//! Bounded, read-only pane captures. No transcripts or preview persistence.
use std::{error::Error, fmt, process::Stdio};

use tokio::{io::AsyncReadExt, time::timeout};

use crate::tmux::{COMMAND_TIMEOUT, TmuxClient, ensure_success, validate_id};
use crate::{DiscoveryError, Engine, WorkItem, WorkItemError};

mod control;

const MAX_CAPTURE: usize = 256 * 1024;
const FORMAT: &str = concat!(
    "\x1e#{pid}:#{start_time}\x1f#{socket_path}\x1f#{pane_id}\x1f",
    "#{pane_pid}\x1f#{pane_dead}\x1f#{pane_in_mode}\x1f",
    "#{pane_width}\x1f#{pane_height}\x1e"
);

/// Recent terminal output, not a complete agent response. ANSI styles are data
/// for the presentation layer to decode, never bytes to write to a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanePreview {
    pub text: String,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug)]
pub enum PreviewError {
    State(WorkItemError),
    Transport(DiscoveryError),
    Changed,
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(error) => error.fmt(f),
            Self::Transport(error) => error.fmt(f),
            Self::Changed => f.write_str("Work item changed or was removed; preview discarded."),
        }
    }
}

impl Error for PreviewError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::Changed => None,
        }
    }
}

impl Engine {
    /// Capture an unchanged registration without focusing, resizing, or sending
    /// input to its pane. Only recent screen/history text is read, never files.
    pub async fn preview_work_item(
        &self,
        expected: &WorkItem,
    ) -> Result<PanePreview, PreviewError> {
        let check = || {
            self.work_items()
                .map_err(PreviewError::State)?
                .iter()
                .find(|item| item.id == expected.id)
                .filter(|item| *item == expected)
                .map(|_| ())
                .ok_or(PreviewError::Changed)
        };
        check()?;
        let preview = self
            .tmux
            .capture_preview(&expected.pane_id)
            .await
            .map_err(PreviewError::Transport)?;
        check()?;
        Ok(preview)
    }
}

impl TmuxClient {
    async fn capture_preview(&self, pane: &str) -> Result<PanePreview, DiscoveryError> {
        validate_id(pane, '%', 0)?;
        let mut command = self.command();
        command
            .args([
                "display-message",
                "-p",
                "-t",
                pane,
                FORMAT,
                ";",
                "capture-pane",
                "-p",
                "-e",
                "-S",
                "-200",
                "-t",
                pane,
                ";",
                "display-message",
                "-p",
                "-t",
                pane,
                FORMAT,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        timeout(COMMAND_TIMEOUT, async {
            let mut child = command.spawn().map_err(DiscoveryError::Unavailable)?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| invalid("Preview stdout unavailable."))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| invalid("Preview stderr unavailable."))?;
            let (stdout, stderr, status) =
                tokio::try_join!(read(stdout, MAX_CAPTURE), read(stderr, 16 * 1024), async {
                    child.wait().await.map_err(DiscoveryError::Unavailable)
                })?;
            let output = std::process::Output {
                stdout,
                stderr,
                status,
            };
            ensure_success(&output)?;
            parse_capture(&output.stdout, pane)
        })
        .await
        .map_err(|_| DiscoveryError::TimedOut)?
    }
}

async fn read(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, DiscoveryError> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(DiscoveryError::Unavailable)?;
    if bytes.len() > limit {
        return Err(invalid(
            "Pane preview exceeded its capture limit. Open the agent pane to inspect it.",
        ));
    }
    Ok(bytes)
}

fn invalid(reason: impl Into<String>) -> DiscoveryError {
    DiscoveryError::InvalidOutput {
        record: 0,
        reason: reason.into(),
    }
}

fn parse_capture(bytes: &[u8], pane: &str) -> Result<PanePreview, DiscoveryError> {
    if bytes.len() > MAX_CAPTURE {
        return Err(invalid("Pane preview exceeded its capture limit."));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Preview is not UTF-8."))?;
    let fields: Vec<_> = text.split('\x1e').collect();
    let ["", before, screen, after, "\n"] = fields.as_slice() else {
        return Err(invalid("Incomplete pane preview."));
    };
    if before != after {
        return Err(invalid(
            "Pane identity or size changed during preview capture.",
        ));
    }
    let fields: Vec<_> = before.split('\x1f').collect();
    let [
        server,
        socket,
        id,
        pid,
        dead @ ("0" | "1"),
        mode @ ("0" | "1"),
        width,
        height,
    ] = fields.as_slice()
    else {
        return Err(invalid("Invalid pane preview metadata."));
    };
    if id != &pane
        || server.is_empty()
        || socket.is_empty()
        || pid.parse::<u32>().ok().filter(|pid| *pid > 0).is_none()
    {
        return Err(invalid("Invalid pane preview identity."));
    }
    if *dead == "1" {
        return Err(invalid("Agent pane has exited; preview unavailable."));
    }
    if *mode == "1" {
        return Err(invalid("Pane is in tmux copy mode; preview paused."));
    }
    let size = |value: &str| {
        value
            .parse::<u16>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("Invalid pane preview dimensions."))
    };
    Ok(PanePreview {
        text: screen.strip_prefix('\n').unwrap_or(screen).to_string(),
        width: size(width)?,
        height: size(height)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, time::Duration};

    const META: &str = "123:456\x1f/tmp/private.sock\x1f%14\x1f789\x1f0\x1f0\x1f100\x1f30";

    fn frame(before: &str, text: &str, after: &str) -> String {
        format!("\x1e{before}\x1e\n{text}\x1e{after}\x1e\n")
    }

    #[test]
    fn test_capture_parse() {
        let text = "\x1b[32mλ🙂 done\x1b[0m\n";
        let preview = parse_capture(frame(META, text, META).as_bytes(), "%14").unwrap();
        assert_eq!(
            preview,
            PanePreview {
                text: text.into(),
                width: 100,
                height: 30
            }
        );
    }

    #[test]
    fn test_capture_guards() {
        for after in [
            META.replace("789", "790"),
            META.replace("100", "101"),
            META.replace("123:456", "123:999"),
        ] {
            assert!(parse_capture(frame(META, "text", &after).as_bytes(), "%14").is_err());
        }
        for metadata in [
            META.replace("%14", "%15"),
            META.replace("\x1f0\x1f0\x1f", "\x1f1\x1f0\x1f"),
            META.replace("\x1f0\x1f0\x1f", "\x1f0\x1f1\x1f"),
            META.replace("100", "0"),
            META.replace("100", "bad"),
            META.replace("789", "0"),
            META.replace("/tmp/private.sock", ""),
        ] {
            assert!(parse_capture(frame(&metadata, "text", &metadata).as_bytes(), "%14").is_err());
        }
        for text in [
            Vec::new(),
            b"unframed".to_vec(),
            vec![0xff],
            frame(META, "bad\x1econtent", META).into_bytes(),
            vec![b'x'; MAX_CAPTURE + 1],
        ] {
            assert!(parse_capture(&text, "%14").is_err());
        }
    }

    #[tokio::test]
    async fn test_capture_bounds() {
        assert_eq!(read(&b"1234"[..], 4).await.unwrap(), b"1234");
        assert!(read(&b"12345"[..], 4).await.is_err());
    }

    #[cfg(unix)]
    fn fixture() -> (tempfile::TempDir, Engine, WorkItem) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-tmux");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -eu
base=$(dirname "$0")
printf '%s\n' "$@" > "$base/log"
touch "$base/started"
while [ -e "$base/wait" ]; do sleep 0.01; done
if [ -e "$base/fail" ]; then printf 'pane disappeared\n' >&2; exit 7; fi
exec cat "$base/capture"
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            directory.path().join("capture"),
            frame(META, "agent output\n", META),
        )
        .unwrap();
        let mut engine = Engine::new(directory.path().join("state.json"));
        engine.tmux.executable = executable.into_os_string();
        let item = WorkItem {
            id: "A".into(),
            title: "Task".into(),
            repository: directory.path().into(),
            workspace: directory.path().into(),
            branch: None,
            kind: crate::WorkItemKind::Implementation,
            pane_id: "%14".into(),
        };
        engine.register_work_item(item.clone()).unwrap();
        (directory, engine, item)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_capture_command() {
        let (directory, engine, item) = fixture();
        let original = fs::read(engine.store.state_path()).unwrap();
        assert_eq!(
            engine.preview_work_item(&item).await.unwrap().text,
            "agent output\n"
        );
        let log = fs::read_to_string(directory.path().join("log")).unwrap();
        let args: Vec<_> = log.lines().collect();
        assert_eq!(
            args,
            [
                "-N",
                "display-message",
                "-p",
                "-t",
                "%14",
                FORMAT,
                ";",
                "capture-pane",
                "-p",
                "-e",
                "-S",
                "-200",
                "-t",
                "%14",
                ";",
                "display-message",
                "-p",
                "-t",
                "%14",
                FORMAT
            ]
        );
        assert!(!log.contains("send-keys") && !log.contains("select-pane"));
        assert_eq!(fs::read(engine.store.state_path()).unwrap(), original);
        let log_before = log;
        assert!(
            engine
                .tmux
                .capture_preview("%14;kill-server")
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("log")).unwrap(),
            log_before
        );
        fs::write(directory.path().join("fail"), "").unwrap();
        assert!(
            engine
                .preview_work_item(&item)
                .await
                .unwrap_err()
                .to_string()
                .contains("pane disappeared")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_stale_item() {
        let (directory, engine, mut item) = fixture();
        item.title = "Changed".into();
        assert!(matches!(
            engine.preview_work_item(&item).await,
            Err(PreviewError::Changed)
        ));
        assert!(!directory.path().join("log").exists());
        assert!(matches!(
            engine
                .stream_preview(&item, |_| panic!("stale preview published"))
                .await,
            Err(PreviewError::Changed)
        ));
        let item = engine.work_items().unwrap().remove(0);
        engine.unregister_work_item(&item).unwrap();
        assert!(matches!(
            engine.preview_work_item(&item).await,
            Err(PreviewError::Changed)
        ));
        assert!(!directory.path().join("log").exists());
    }

    #[cfg(unix)]
    async fn started(directory: &Path) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !directory.join("started").exists() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_capture_recheck() {
        let (directory, engine, item) = fixture();
        fs::write(directory.path().join("wait"), "").unwrap();
        let capturing = engine.preview_work_item(&item);
        tokio::pin!(capturing);
        tokio::select! {
            result = &mut capturing => panic!("capture returned before barrier: {result:?}"),
            _ = started(directory.path()) => {}
        }
        engine
            .update_work_item_description(&item, "Changed")
            .unwrap();
        fs::remove_file(directory.path().join("wait")).unwrap();
        assert!(matches!(capturing.await, Err(PreviewError::Changed)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_capture_timeout() {
        let (directory, engine, item) = fixture();
        fs::write(directory.path().join("wait"), "").unwrap();
        assert!(matches!(
            engine.preview_work_item(&item).await,
            Err(PreviewError::Transport(DiscoveryError::TimedOut))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires tmux; creates and cleans up only its own isolated server"]
    async fn test_tmux_preview() {
        struct Server(std::path::PathBuf);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = std::process::Command::new("tmux")
                    .arg("-S")
                    .arg(&self.0)
                    .arg("kill-server")
                    .output();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("preview.sock");
        let output = std::process::Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "preview",
                "-x",
                "100",
                "-y",
                "30",
                "printf '\\033[38;2;1;2;3mpreview-marker\\033[0m\\n'; exec sleep 30",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _server = Server(socket.clone());
        let mut engine = Engine::new(directory.path().join("state.json"));
        engine.tmux.socket = Some(socket);
        let pane = engine.discover().await.unwrap().sessions[0].windows[0].panes[0]
            .id
            .clone();
        let item = WorkItem {
            id: "A".into(),
            title: "Preview".into(),
            repository: directory.path().into(),
            workspace: directory.path().into(),
            branch: None,
            kind: crate::WorkItemKind::Implementation,
            pane_id: pane.clone(),
        };
        engine.register_work_item(item.clone()).unwrap();
        engine
            .tmux
            .execute(&[
                "new-window".into(),
                "-t".into(),
                "preview".into(),
                "-n".into(),
                "active".into(),
                "exec sleep 30".into(),
            ])
            .await
            .unwrap();
        let focus = [
            "display-message",
            "-p",
            "-t",
            "preview",
            "#{window_id}:#{pane_id}",
        ];
        let sizes = [
            "list-panes",
            "-a",
            "-F",
            "#{pane_id}:#{pane_width}:#{pane_height}",
        ];
        let before = (
            engine.tmux.query(&focus).await.unwrap(),
            engine.tmux.query(&sizes).await.unwrap(),
        );
        let clients = ["list-clients", "-F", "#{client_name}"];
        let clients_before = engine.tmux.query(&clients).await.unwrap();
        let state = fs::read(engine.store.state_path()).unwrap();
        let capture = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let capture = engine.preview_work_item(&item).await.unwrap();
                if capture.text.contains("preview-marker") {
                    break capture;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(capture.text.contains("\x1b[38;2;1;2;3m"));
        assert_eq!((capture.width, capture.height), (100, 30));
        assert_eq!(engine.tmux.query(&focus).await.unwrap(), before.0);
        assert_eq!(engine.tmux.query(&sizes).await.unwrap(), before.1);
        assert_eq!(fs::read(engine.store.state_path()).unwrap(), state);
        let mut streamed = None;
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            engine.stream_preview(&item, |capture| {
                streamed = Some(capture);
            }),
        )
        .await;
        assert!(result.is_err(), "stream stopped: {result:?}");
        let streamed = streamed.expect("stream did not deliver an initial snapshot");
        assert!(streamed.text.contains("preview-marker"));
        assert_eq!((streamed.width, streamed.height), (100, 30));
        assert_eq!(engine.tmux.query(&focus).await.unwrap(), before.0);
        assert_eq!(engine.tmux.query(&sizes).await.unwrap(), before.1);
        assert_eq!(fs::read(engine.store.state_path()).unwrap(), state);
        // Cancellation must detach/reap only the observer, leaving the session.
        tokio::time::timeout(Duration::from_secs(3), async {
            while engine.tmux.query(&clients).await.unwrap() != clients_before {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(engine.stream_preview(&item, |_| {}), async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                engine
                    .tmux
                    .execute(&["kill-pane".into(), "-t".into(), pane])
                    .await
                    .unwrap();
            })
            .0
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(engine.preview_work_item(&item).await.is_err());
        assert_eq!(engine.work_items().unwrap(), [item]);
    }
}
