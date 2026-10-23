//! A selected-pane notification stream, not a terminal emulator. Raw output is
//! discarded; only bounded capture-pane snapshots leave this adapter.
use std::{process::Stdio, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    time::{Instant, sleep_until, timeout},
};

use super::{FORMAT, MAX_CAPTURE, PanePreview, PreviewError, invalid, parse_capture};
use crate::{DiscoveryError, Engine, WorkItem, tmux::COMMAND_TIMEOUT};

const REFRESH: Duration = Duration::from_millis(100);
const MAX_TRAFFIC: usize = 4 * 1024 * 1024;

impl Engine {
    /// Stream recent snapshots when the selected pane changes. This operation
    /// runs until cancelled or disconnected. Dropping it kills its own client,
    /// never the pane/server. A caller may fall back to preview_work_item.
    /// Notifications never establish agent status or authorize input.
    pub async fn stream_preview(
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
                "read-only,ignore-size,active-pane,pause-after=1",
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
        let run = async {
            // Attach itself produces a reply. Never change client dimensions,
            // active window/pane, session options, environment, or input.
            timeout(COMMAND_TIMEOUT, client.reply(1, &expected.pane_id))
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
        };
        // Startup and capture reads have deadlines; an idle notification wait
        // intentionally has none. Caller cancellation still drops the client.
        run.await
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
                    return Err(PreviewError::Transport(invalid("Preview command failed.")));
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
            } else {
                self.dirty |= notification(&line, pane).map_err(PreviewError::Transport)?;
            }
        }
    }

    async fn capture(&mut self, pane: &str) -> Result<PanePreview, PreviewError> {
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
            if notification(&line, pane).map_err(PreviewError::Transport)? {
                return Ok(());
            }
        }
    }
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
    #[tokio::test]
    async fn test_stream_command() {
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
IFS= read -r command
printf '%s\n' "$command" > "$0.commands"
exec cat "$0.reply"
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
        let before = fs::read(engine.store.state_path()).unwrap();
        let mut snapshots = Vec::new();
        assert!(
            engine
                .stream_preview(&item, |capture| snapshots.push(capture))
                .await
                .is_err()
        );
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].text, "snapshot\n");
        let args = fs::read_to_string(directory.path().join("tmux.args")).unwrap();
        assert!(args.contains("-N\n-C\nattach-session\n-E\n-f\nread-only,ignore-size,active-pane,pause-after=1\n-t\n$4\n"));
        let commands = fs::read_to_string(directory.path().join("tmux.commands")).unwrap();
        assert_eq!(
            commands,
            format!(
                "display-message -p -t %14 '{FORMAT}' ; capture-pane -p -e -S -200 -t %14 ; display-message -p -t %14 '{FORMAT}' ; display-message -p -t %14 '#{{session_id}}'\n"
            )
        );
        assert_eq!(fs::read(engine.store.state_path()).unwrap(), before);
        fs::write(
            directory.path().join("tmux.reply"),
            reply.replace("$4\n", "$5\n"),
        )
        .unwrap();
        assert!(
            engine
                .stream_preview(&item, |_| panic!("moved pane published"))
                .await
                .unwrap_err()
                .to_string()
                .contains("moved")
        );
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
