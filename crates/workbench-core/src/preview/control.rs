//! A selected-pane notification stream, not a terminal emulator. Raw output is
//! discarded; only bounded capture-pane snapshots leave this adapter.
use std::{collections::HashSet, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    time::{Instant, sleep, sleep_until, timeout},
};

use super::{FORMAT, MAX_CAPTURE, PanePreview, PreviewError, invalid, parse_capture};
use crate::{DiscoveryError, Engine, WorkItem, tmux::COMMAND_TIMEOUT};

const REFRESH: Duration = Duration::from_millis(100);
const MAX_TRAFFIC: usize = 4 * 1024 * 1024;
const MAX_PANES: usize = 1024;

struct Retry(Duration);

impl Default for Retry {
    fn default() -> Self {
        Self(Duration::from_millis(200))
    }
}

impl Retry {
    fn next(&mut self, uptime: Duration) -> Duration {
        if uptime >= Duration::from_secs(5) {
            *self = Self::default();
        }
        let delay = self.0;
        self.0 = (delay * 2).min(Duration::from_secs(3));
        delay
    }
}

impl Engine {
    /// Stream recent snapshots when the selected pane changes. This operation
    /// reconnects after transport errors with bounded backoff. Dropping it kills
    /// its own client, never the pane/server. Registration errors end the stream.
    /// Notifications never establish agent status or authorize input.
    pub async fn stream_preview(
        &self,
        expected: &WorkItem,
        mut update: impl FnMut(Result<PanePreview, PreviewError>),
    ) -> Result<(), PreviewError> {
        let mut retry = Retry::default();
        loop {
            let started = Instant::now();
            let result = self
                .preview_session(expected, |capture| update(Ok(capture)))
                .await;
            let error = match result {
                Err(error @ (PreviewError::Changed | PreviewError::State(_))) => return Err(error),
                Err(error) => error,
                Ok(()) => PreviewError::Transport(invalid("Preview stream ended.")),
            };
            update(Err(error));
            sleep(retry.next(started.elapsed())).await;
        }
    }

    async fn preview_session(
        &self,
        expected: &WorkItem,
        mut update: impl FnMut(PanePreview),
    ) -> Result<(), PreviewError> {
        let check = || {
            self.work_items()
                .map_err(PreviewError::State)?
                .iter()
                .find(|item| *item == expected)
                .map(|_| ())
                .ok_or(PreviewError::Changed)
        };
        check()?;
        crate::tmux::validate_id(&expected.pane_id, '%', 0).map_err(PreviewError::Transport)?;
        let session = self
            .tmux
            .query(&[
                "display-message",
                "-p",
                "-t",
                &expected.pane_id,
                "#{session_id}",
            ])
            .await
            .map_err(PreviewError::Transport)?;
        let session = session.trim();
        crate::tmux::validate_id(session, '$', 0).map_err(PreviewError::Transport)?;
        let mut command = self.tmux.command();
        command
            .args([
                "-C",
                "attach-session",
                "-E",
                "-f",
                "read-only,ignore-size,active-pane,no-output,pause-after=1",
                "-t",
                session,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .map_err(|e| PreviewError::Transport(DiscoveryError::Unavailable(e)))?;
        let mut client = Control::new(child, session).map_err(PreviewError::Transport)?;
        // Attach itself produces a reply. Never change client dimensions,
        // active window/pane, session options, environment, or input.
        timeout(COMMAND_TIMEOUT, client.reply(1, &expected.pane_id))
            .await
            .map_err(|_| PreviewError::Transport(DiscoveryError::TimedOut))??;
        timeout(COMMAND_TIMEOUT, client.filter(&expected.pane_id, true))
            .await
            .map_err(|_| PreviewError::Transport(DiscoveryError::TimedOut))??;
        loop {
            check()?;
            let started = Instant::now();
            let capture = timeout(COMMAND_TIMEOUT, client.capture(&expected.pane_id))
                .await
                .map_err(|_| PreviewError::Transport(DiscoveryError::TimedOut))??;
            check()?;
            update(capture);
            // Coalesce output while capped. Reading resumes before the
            // next request and snapshots include all intervening output.
            client.changed(&expected.pane_id).await?;
            sleep_until(started + REFRESH).await;
        }
    }
}

struct Control {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    traffic: usize,
    since: Instant,
    dirty: bool,
    session: String,
    paused: HashSet<String>,
    rescan: bool,
}

impl Control {
    fn new(mut child: Child, session: &str) -> Result<Self, DiscoveryError> {
        Ok(Self {
            input: child
                .stdin
                .take()
                .ok_or_else(|| invalid("Preview input unavailable."))?,
            output: BufReader::new(
                child
                    .stdout
                    .take()
                    .ok_or_else(|| invalid("Preview output unavailable."))?,
            ),
            _child: child,
            traffic: 0,
            since: Instant::now(),
            dirty: false,
            session: session.into(),
            paused: HashSet::new(),
            rescan: false,
        })
    }

    async fn line(&mut self) -> Result<Vec<u8>, DiscoveryError> {
        let mut line = Vec::new();
        loop {
            let bytes = self
                .output
                .fill_buf()
                .await
                .map_err(DiscoveryError::Unavailable)?;
            if bytes.is_empty() {
                return Err(invalid("Preview connection closed."));
            }
            let end = bytes.iter().position(|b| *b == b'\n').map(|i| i + 1);
            let count = end.unwrap_or(bytes.len());
            if line.len() + count > MAX_CAPTURE {
                return Err(invalid("Preview protocol line exceeded its limit."));
            }
            line.extend_from_slice(&bytes[..count]);
            self.output.consume(count);
            if self.since.elapsed() >= Duration::from_secs(1) {
                self.since = Instant::now();
                self.traffic = 0;
            }
            self.traffic += count;
            if self.traffic > MAX_TRAFFIC {
                return Err(invalid("Preview traffic exceeded its limit."));
            }
            if end.is_some() {
                return Ok(line);
            }
        }
    }

    async fn reply(&mut self, count: usize, pane: &str) -> Result<Vec<u8>, PreviewError> {
        let mut result = Vec::new();
        let mut block: Option<Vec<u8>> = None;
        let mut completed = 0;
        let mut block_start = 0;
        loop {
            let line = self.line().await.map_err(PreviewError::Transport)?;
            if let Some(guard) = &block {
                if line.strip_prefix(b"%end ") == Some(guard.as_slice()) {
                    block = None;
                    completed += 1;
                    if completed == count {
                        return Ok(result);
                    }
                } else if line.strip_prefix(b"%error ") == Some(guard.as_slice()) {
                    let message: String = String::from_utf8_lossy(&result[block_start..])
                        .chars()
                        .take(300)
                        .collect();
                    return Err(PreviewError::Transport(invalid(format!(
                        "Preview command failed: {}",
                        message.trim()
                    ))));
                } else {
                    if result.len() + line.len() > MAX_CAPTURE {
                        return Err(PreviewError::Transport(invalid(
                            "Pane preview exceeded its capture limit.",
                        )));
                    }
                    result.extend_from_slice(&line);
                }
            } else if let Some(guard) = line.strip_prefix(b"%begin ") {
                if !valid_guard(guard) {
                    return Err(PreviewError::Transport(invalid(
                        "Invalid preview reply guard.",
                    )));
                }
                block = Some(guard.into());
                block_start = result.len();
            } else {
                self.dirty |= self.notice(&line, pane).map_err(PreviewError::Transport)?;
            }
        }
    }

    async fn capture(&mut self, pane: &str) -> Result<PanePreview, PreviewError> {
        if self.rescan {
            self.filter(pane, false).await?;
        }
        // Both interpolations are validated pane IDs or a constant with no
        // quotes/newlines. No agent text or user-supplied shell text is executed.
        let command = format!(
            "display-message -p -t {pane} '{FORMAT}' ; capture-pane -p -e -S -200 -t {pane} ; display-message -p -t {pane} '{FORMAT}' ; display-message -p -t {pane} '#{{session_id}}'\n"
        );
        self.dirty = false;
        self.input
            .write_all(command.as_bytes())
            .await
            .map_err(|e| PreviewError::Transport(DiscoveryError::Unavailable(e)))?;
        let bytes = self.reply(3, pane).await?;
        let session = self.reply(1, pane).await?;
        if session.strip_suffix(b"\n") != Some(self.session.as_bytes()) {
            return Err(PreviewError::Transport(invalid(
                "Preview pane moved to another session.",
            )));
        }
        parse_capture(&bytes, pane).map_err(PreviewError::Transport)
    }

    async fn changed(&mut self, pane: &str) -> Result<(), PreviewError> {
        if std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        loop {
            let line = self.line().await.map_err(PreviewError::Transport)?;
            if self.notice(&line, pane).map_err(PreviewError::Transport)? {
                return Ok(());
            }
        }
    }

    async fn command(&mut self, command: &str, pane: &str) -> Result<Vec<u8>, PreviewError> {
        self.input
            .write_all(command.as_bytes())
            .await
            .map_err(|e| PreviewError::Transport(DiscoveryError::Unavailable(e)))?;
        self.reply(1, pane).await
    }

    async fn filter(&mut self, pane: &str, enable: bool) -> Result<(), PreviewError> {
        // Pause, never turn off: off can stop producer reads when all control
        // clients turn a pane off. Pause only discards delivery to this client.
        self.rescan = false;
        let command = format!("list-panes -s -t '{}' -F '#{{pane_id}}'\n", self.session);
        let bytes = self.command(&command, pane).await?;
        let panes = parse_panes(&bytes).map_err(PreviewError::Transport)?;
        if !panes.iter().any(|id| id == pane) {
            return Err(PreviewError::Transport(invalid(
                "Selected pane left the preview session.",
            )));
        }
        self.paused.retain(|id| panes.contains(id));
        // Changing no-output resets tmux's pane offsets, including paused
        // state. Enable and pause in the same command, in that order, before
        // the server can dispatch more pane output.
        let mut command = String::from("refresh-client");
        if enable {
            self.paused.clear();
            command.push_str(" -f '!no-output'");
        }
        for id in &panes {
            if id != pane && !self.paused.contains(id) {
                command.push_str(&format!(" -A '{id}:pause'"));
            }
        }
        if command != "refresh-client" {
            command.push('\n');
            self.command(&command, pane).await?;
            self.paused
                .extend(panes.into_iter().filter(|id| id != pane));
        }
        Ok(())
    }

    fn notice(&mut self, line: &[u8], pane: &str) -> Result<bool, DiscoveryError> {
        let mut fields = line.split(|b| *b == b' ' || *b == b'\n');
        let kind = fields.next().unwrap_or_default();
        let target = fields.next().unwrap_or_default();
        if matches!(kind, b"%output" | b"%extended-output") && target != pane.as_bytes()
            || matches!(
                kind,
                b"%layout-change"
                    | b"%window-add"
                    | b"%window-close"
                    | b"%unlinked-window-close"
                    | b"%window-pane-changed"
                    | b"%sessions-changed"
            )
        {
            self.rescan = true;
        }
        Ok(notification(line, pane)? || self.rescan)
    }
}

fn parse_panes(bytes: &[u8]) -> Result<Vec<String>, DiscoveryError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Invalid preview pane list."))?;
    let mut panes = Vec::new();
    for id in text.lines() {
        crate::tmux::validate_id(id, '%', 0)?;
        if panes.len() == MAX_PANES {
            return Err(invalid("Preview session has too many panes."));
        }
        panes.push(id.to_owned());
    }
    Ok(panes)
}

fn valid_guard(bytes: &[u8]) -> bool {
    bytes.strip_suffix(b"\n").is_some_and(|bytes| {
        let fields: Vec<_> = bytes.split(|b| *b == b' ').collect();
        fields.len() == 3
            && fields
                .iter()
                .all(|field| !field.is_empty() && field.iter().all(u8::is_ascii_digit))
    })
}

fn notification(line: &[u8], pane: &str) -> Result<bool, DiscoveryError> {
    let mut fields = line.split(|b| *b == b' ' || *b == b'\n');
    let kind = fields.next().unwrap_or_default();
    let target = fields.next().unwrap_or_default();
    if kind == b"%exit" || kind == b"%pause" && target == pane.as_bytes() {
        return Err(invalid("Preview connection stopped or fell behind."));
    }
    Ok(matches!(
        kind,
        b"%output" | b"%extended-output" | b"%pane-mode-changed"
    ) && target == pane.as_bytes()
        || matches!(
            kind,
            b"%layout-change"
                | b"%window-close"
                | b"%unlinked-window-close"
                | b"%window-pane-changed"
                | b"%sessions-changed"
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fixture(bytes: &[u8]) -> (tempfile::TempDir, Control) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("protocol");
        std::fs::write(&path, bytes).unwrap();
        let child = tokio::process::Command::new("/bin/cat")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        (directory, Control::new(child, "$4").unwrap())
    }

    #[cfg(unix)]
    fn setup() -> (tempfile::TempDir, Engine, WorkItem, String) {
        use std::{fs, os::unix::fs::PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("tmux");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf '%s\n' "$@" >> "$0.args"
case "$*" in
  *display-message*) printf '$4\n'; exit 0 ;;
esac
printf '%%begin 1 1 0\n%%end 1 1 0\n'
while IFS= read -r command; do
  printf '%s\n' "$command" >> "$0.commands"
  case "$command" in
    list-panes*) printf '%%begin 1 8 1\n%%14\n%%1\n%%end 1 8 1\n' ;;
    refresh-client*) printf '%%begin 1 9 1\n%%end 1 9 1\n' ;;
    *) while [ -e "$0.wait" ]; do sleep 0.01; done; exec cat "$0.reply" ;;
  esac
done
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let metadata = "\x1e1:2\x1f/socket\x1f%14\x1f123\x1f0\x1f0\x1f80\x1f24\x1e";
        let reply = format!(
            "%begin 1 2 1\n{metadata}\n%end 1 2 1\n%begin 1 3 1\nsnapshot\n%end 1 3 1\n%begin 1 4 1\n{metadata}\n%end 1 4 1\n%begin 1 5 1\n$4\n%end 1 5 1\n"
        );
        fs::write(directory.path().join("tmux.reply"), &reply).unwrap();
        let mut engine = Engine::new(directory.path().join("state.json"));
        engine.tmux.executable = executable.into_os_string();
        let item = WorkItem {
            id: "A".into(),
            title: String::new(),
            repository: directory.path().into(),
            workspace: directory.path().into(),
            branch: None,
            kind: crate::WorkItemKind::Implementation,
            pane_id: "%14".into(),
        };
        engine.register_work_item(item.clone()).unwrap();
        (directory, engine, item, reply)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_command() {
        use std::fs;
        let (directory, engine, item, reply) = setup();
        let before = fs::read(engine.store.state_path()).unwrap();
        let mut snapshots = Vec::new();
        assert!(
            engine
                .preview_session(&item, |capture| snapshots.push(capture))
                .await
                .is_err()
        );
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].text, "snapshot\n");
        let args = fs::read_to_string(directory.path().join("tmux.args")).unwrap();
        assert!(args.contains("-N\n-C\nattach-session\n-E\n-f\nread-only,ignore-size,active-pane,no-output,pause-after=1\n-t\n$4\n"));
        let commands = fs::read_to_string(directory.path().join("tmux.commands")).unwrap();
        assert!(commands.starts_with(
            "list-panes -s -t '$4' -F '#{pane_id}'\nrefresh-client -f '!no-output' -A '%1:pause'\n"
        ));
        assert!(!commands.contains("%14:pause"));
        for forbidden in ["send-keys", "select-pane", "select-window", ":off", " -C "] {
            assert!(!commands.contains(forbidden), "{forbidden}");
        }
        assert!(commands.ends_with(&format!(
                "display-message -p -t %14 '{FORMAT}' ; capture-pane -p -e -S -200 -t %14 ; display-message -p -t %14 '{FORMAT}' ; display-message -p -t %14 '#{{session_id}}'\n"
        )));
        assert_eq!(fs::read(engine.store.state_path()).unwrap(), before);
        fs::write(
            directory.path().join("tmux.reply"),
            reply.replace("$4\n", "$5\n"),
        )
        .unwrap();
        assert!(
            engine
                .preview_session(&item, |_| panic!("moved pane published"))
                .await
                .unwrap_err()
                .to_string()
                .contains("moved")
        );
        let before = fs::read(directory.path().join("tmux.args")).unwrap();
        let stale = WorkItem {
            title: "Changed".into(),
            ..item
        };
        assert!(matches!(
            engine
                .stream_preview(&stale, |_| panic!("stale snapshot"))
                .await,
            Err(PreviewError::Changed)
        ));
        assert_eq!(
            fs::read(directory.path().join("tmux.args")).unwrap(),
            before
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_recheck() {
        use std::fs;
        let (directory, engine, item, _reply) = setup();
        let barrier = directory.path().join("tmux.wait");
        fs::write(&barrier, "").unwrap();
        let change = async {
            timeout(Duration::from_secs(3), async {
                loop {
                    let commands = fs::read_to_string(directory.path().join("tmux.commands"))
                        .unwrap_or_default();
                    if commands.contains("capture-pane") {
                        break;
                    }
                    sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            engine
                .update_work_item_description(&item, "Changed")
                .unwrap();
            fs::remove_file(&barrier).unwrap();
        };
        let (result, ()) = tokio::join!(
            engine.stream_preview(&item, |_| panic!("stale capture published")),
            change
        );
        assert!(matches!(result, Err(PreviewError::Changed)));
    }

    #[test]
    fn test_retry() {
        let mut retry = Retry::default();
        for millis in [200, 400, 800, 1600, 3000, 3000] {
            assert_eq!(retry.next(Duration::ZERO), Duration::from_millis(millis));
        }
        assert_eq!(
            retry.next(Duration::from_secs(5)),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn test_panes() {
        assert_eq!(parse_panes(b"%14\n%1\n").unwrap(), ["%14", "%1"]);
        for bytes in [
            b"%1;kill-server\n".as_slice(),
            b"\n",
            b"%1\nbad\n",
            b"%1\n\xff\n",
        ] {
            assert!(parse_panes(bytes).is_err());
        }
        assert!(parse_panes("%1\n".repeat(MAX_PANES + 1).as_bytes()).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_rescan() {
        let (_directory, mut client) = fixture(b"");
        assert!(!client.notice(b"%pause %1\n", "%14").unwrap());
        assert!(!client.rescan);
        assert!(client.notice(b"%window-add @2\n", "%14").unwrap());
        assert!(client.rescan);
        client.rescan = false;
        assert!(client.notice(b"%output %2 ignored\n", "%14").unwrap());
        assert!(client.rescan);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_replies() {
        let bytes = b"%extended-output %1 0 : ignored\\033\n%begin 1 2 1\nmetadata\n%end 1 2 1\n%extended-output %14 0 : discard\xff\n%begin 1 3 1\n%end 999 9 9\n\x1b[32mcontent\x1b[0m\n%end 1 3 1\n%begin 1 4 1\nmetadata\n%end 1 4 1\n";
        let (_directory, mut client) = fixture(bytes);
        assert_eq!(
            client.reply(3, "%14").await.unwrap(),
            b"metadata\n%end 999 9 9\n\x1b[32mcontent\x1b[0m\nmetadata\n"
        );
        // A change between reply blocks must survive until the next capture.
        assert!(client.dirty);
        assert!(client.changed("%14").await.is_ok());
        assert!(!client.dirty);
        assert!(client.changed("%14").await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_protocol_errors() {
        for bytes in [
            b"%begin 1 2 1\nfailed\n%error 1 2 1\n".as_slice(),
            b"%begin bad\n",
            b"%begin 1 2 1\nunterminated\n",
            b"%exit server gone\n",
            b"%pause %14\n",
        ] {
            let (_directory, mut client) = fixture(bytes);
            assert!(client.reply(1, "%14").await.is_err(), "{bytes:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_limits() {
        let oversized = vec![b'x'; MAX_CAPTURE + 1];
        let (_directory, mut client) = fixture(&oversized);
        assert!(
            client
                .line()
                .await
                .unwrap_err()
                .to_string()
                .contains("line exceeded")
        );

        let row = "x".repeat(MAX_CAPTURE / 2);
        let reply = format!("%begin 1 2 1\n{row}\n{row}\n%end 1 2 1\n");
        let (_directory, mut client) = fixture(reply.as_bytes());
        assert!(
            client
                .reply(1, "%14")
                .await
                .unwrap_err()
                .to_string()
                .contains("capture limit")
        );

        let (_directory, mut client) = fixture(b"%output %14 data\n");
        client.traffic = MAX_TRAFFIC;
        assert!(
            client
                .line()
                .await
                .unwrap_err()
                .to_string()
                .contains("traffic exceeded")
        );
    }

    #[test]
    fn test_notifications() {
        for text in [
            b"%output %14 raw\\033 bytes\n".as_slice(),
            b"%extended-output %14 0 : raw\n",
            b"%pane-mode-changed %14\n",
            b"%layout-change @2 data\n",
            b"%unlinked-window-close @2\n",
            b"%window-pane-changed @2 %14\n",
        ] {
            assert!(notification(text, "%14").unwrap());
        }
        assert!(!notification(b"%output %1 ignore\n", "%14").unwrap());
        assert!(!notification(b"%output %140 ignore\n", "%14").unwrap());
        assert!(notification(b"%pause %14\n", "%14").is_err());
        assert!(notification(b"%exit\n", "%14").is_err());
        assert!(!notification(b"%pause %1\n", "%14").unwrap());
    }

    #[test]
    fn test_guards() {
        assert!(valid_guard(b"123 4 1\n"));
        for text in [
            b"\n".as_slice(),
            b"1 2\n",
            b"1  2 3\n",
            b"1 2 ;kill-server\n",
            b"1 2 3",
        ] {
            assert!(!valid_guard(text));
        }
    }
}
