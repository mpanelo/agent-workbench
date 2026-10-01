use std::{
    collections::BTreeMap,
    error::Error,
    fmt, io,
    process::{Output, Stdio},
    time::Duration,
};

use tokio::{process::Command, time::timeout};

use crate::{Pane, Session, Snapshot, Window};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const FIELD_SEPARATOR: char = '\u{1f}';
const RECORD_SEPARATOR: char = '\u{1e}';
// Real control-character separators, not backslash escapes interpreted by a shell.
// This preserves spaces, tabs, newlines, and Unicode in titles and paths.
const PANE_FORMAT: &str = concat!(
    "#{session_id}\x1f#{session_name}\x1f#{window_id}\x1f",
    "#{window_index}\x1f#{window_name}\x1f#{pane_id}\x1f",
    "#{pane_index}\x1f#{pane_title}\x1f#{pane_current_command}\x1f",
    "#{pane_current_path}\x1e"
);

#[derive(Debug)]
pub enum DiscoveryError {
    Unavailable(io::Error),
    CommandFailed { code: Option<i32>, message: String },
    TimedOut,
    InvalidOutput { record: usize, reason: String },
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) if error.kind() == io::ErrorKind::NotFound => {
                write!(
                    f,
                    "tmux was not found. Install tmux and make it available on PATH."
                )
            }
            Self::Unavailable(error) => write!(f, "Could not run tmux: {error}"),
            Self::CommandFailed { code, message } => {
                write!(
                    f,
                    "Tmux discovery failed (exit {code:?}): {message}. Check that a tmux server with a session is running."
                )
            }
            Self::TimedOut => write!(
                f,
                "Tmux discovery timed out after 3 seconds. Check that the tmux server is responding."
            ),
            Self::InvalidOutput { record, reason } => {
                write!(
                    f,
                    "Could not read tmux metadata at record {record}: {reason}"
                )
            }
        }
    }
}

impl Error for DiscoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(error) => Some(error),
            _ => None,
        }
    }
}

pub(crate) async fn discover() -> Result<Snapshot, DiscoveryError> {
    let mut command = Command::new("tmux");
    command.args(["list-panes", "-a", "-F", PANE_FORMAT]);
    interpret_output(run_command(&mut command, COMMAND_TIMEOUT).await?)
}

async fn run_command(command: &mut Command, limit: Duration) -> Result<Output, DiscoveryError> {
    command.stdin(Stdio::null()).kill_on_drop(true);
    timeout(limit, command.output())
        .await
        .map_err(|_| DiscoveryError::TimedOut)?
        .map_err(DiscoveryError::Unavailable)
}

fn interpret_output(output: Output) -> Result<Snapshot, DiscoveryError> {
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(DiscoveryError::CommandFailed {
            code: output.status.code(),
            message: if message.is_empty() {
                "tmux returned no error details".to_owned()
            } else {
                message
            },
        });
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|error| invalid_output(0, format!("metadata is not UTF-8: {error}")))?;
    parse_snapshot(text)
}

fn invalid_output(record: usize, reason: impl Into<String>) -> DiscoveryError {
    DiscoveryError::InvalidOutput {
        record,
        reason: reason.into(),
    }
}

fn validate_id(id: &str, prefix: char, record: usize) -> Result<(), DiscoveryError> {
    if !id.starts_with(prefix) || id[1..].parse::<u64>().is_err() {
        return Err(invalid_output(record, format!("invalid ID {id:?}")));
    }
    Ok(())
}

fn parse_index(value: &str, record: usize) -> Result<u32, DiscoveryError> {
    value
        .parse()
        .map_err(|_| invalid_output(record, format!("invalid index {value:?}")))
}

fn parse_snapshot(text: &str) -> Result<Snapshot, DiscoveryError> {
    if text.is_empty() {
        return Ok(Snapshot::default());
    }
    let records = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .strip_suffix(RECORD_SEPARATOR)
        .ok_or_else(|| invalid_output(1, "missing record terminator"))?;
    let mut sessions = BTreeMap::<String, Session>::new();
    // tmux adds a newline after each formatted record. Remove only that newline,
    // never whitespace inside a metadata field.
    for (offset, raw) in records.split(RECORD_SEPARATOR).enumerate() {
        let record = offset + 1;
        let row = raw.strip_prefix('\n').unwrap_or(raw);
        if row.is_empty() {
            return Err(invalid_output(record, "empty metadata record"));
        }
        let fields: Vec<_> = row.split(FIELD_SEPARATOR).collect();
        let [
            session_id,
            session_name,
            window_id,
            window_index,
            window_name,
            pane_id,
            pane_index,
            title,
            command,
            path,
        ] = fields.as_slice()
        else {
            return Err(invalid_output(
                record,
                format!("expected 10 fields, got {}", fields.len()),
            ));
        };
        validate_id(session_id, '$', record)?;
        validate_id(window_id, '@', record)?;
        validate_id(pane_id, '%', record)?;
        let window_index = parse_index(window_index, record)?;
        let pane = Pane {
            id: (*pane_id).to_owned(),
            index: parse_index(pane_index, record)?,
            title: (*title).to_owned(),
            current_command: (!command.is_empty()).then(|| (*command).to_owned()),
            working_directory: (!path.is_empty()).then(|| (*path).into()),
        };
        let session = sessions
            .entry((*session_id).to_owned())
            .or_insert_with(|| Session {
                id: (*session_id).to_owned(),
                name: (*session_name).to_owned(),
                windows: Vec::new(),
            });
        if session.name != *session_name {
            return Err(invalid_output(record, "conflicting session names"));
        }
        let window_position = session
            .windows
            .iter()
            .position(|window| window.id == *window_id);
        let window = match window_position {
            Some(position) => &mut session.windows[position],
            None => {
                session.windows.push(Window {
                    id: (*window_id).to_owned(),
                    index: window_index,
                    name: (*window_name).to_owned(),
                    panes: Vec::new(),
                });
                session
                    .windows
                    .last_mut()
                    .ok_or_else(|| invalid_output(record, "missing window"))?
            }
        };
        if window.index != window_index || window.name != *window_name {
            return Err(invalid_output(record, "conflicting window metadata"));
        }
        if window
            .panes
            .iter()
            .any(|existing| existing.id == pane.id || existing.index == pane.index)
        {
            return Err(invalid_output(record, "duplicate pane in window"));
        }
        window.panes.push(pane);
    }
    let mut sessions: Vec<_> = sessions.into_values().collect();
    sessions.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    for session in &mut sessions {
        session.windows.sort_by_key(|window| window.index);
        for window in &mut session.windows {
            window.panes.sort_by_key(|pane| pane.index);
        }
    }
    Ok(Snapshot { sessions })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(fields: [&str; 10]) -> String {
        format!("{}\x1e\n", fields.join("\x1f"))
    }

    fn sample() -> String {
        row([
            "$1",
            "main",
            "@2",
            "2",
            "auth",
            "%14",
            "0",
            "Agent",
            "codex",
            "/work/auth",
        ])
    }

    #[test]
    fn groups_sessions_windows_and_panes_in_display_order() {
        let text = [
            row([
                "$2",
                "reviews",
                "@3",
                "1",
                "PR-1842",
                "%20",
                "0",
                "Review",
                "codex",
                "/reviews/1842",
            ]),
            row([
                "$1",
                "main",
                "@2",
                "2",
                "auth",
                "%15",
                "1",
                "Shell",
                "zsh",
                "/work/auth",
            ]),
            sample(),
            row([
                "$1",
                "main",
                "@1",
                "1",
                "telemetry",
                "%10",
                "0",
                "Retry",
                "codex",
                "/work/telemetry",
            ]),
        ]
        .concat();
        let snapshot = parse_snapshot(&text).unwrap();
        assert_eq!(snapshot.sessions.len(), 2);
        let main = &snapshot.sessions[0];
        assert_eq!((&*main.id, &*main.name), ("$1", "main"));
        assert_eq!(
            main.windows.iter().map(|w| w.index).collect::<Vec<_>>(),
            [1, 2]
        );
        let panes = &main.windows[1].panes;
        assert_eq!(
            panes.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["%14", "%15"]
        );
        assert_eq!(panes[0].current_command.as_deref(), Some("codex"));
        assert_eq!(
            panes[0].working_directory.as_deref(),
            Some(std::path::Path::new("/work/auth"))
        );
    }

    #[test]
    fn preserves_unicode_spaces_tabs_newlines_and_backslashes() {
        let title = "⠴ Agent\tworking\nsecond line \\ #{literal}";
        let path = "/work/my repo\twith tab\nand newline";
        let snapshot = parse_snapshot(&row([
            "$1", "main", "@1", "1", " auth", "%1", "0", title, "codex", path,
        ]))
        .unwrap();
        let window = &snapshot.sessions[0].windows[0];
        assert_eq!(window.name, " auth");
        assert_eq!(window.panes[0].title, title);
        assert_eq!(
            window.panes[0].working_directory.as_deref(),
            Some(std::path::Path::new(path))
        );
    }

    #[test]
    fn absent_optional_metadata_and_empty_title_are_supported() {
        let snapshot = parse_snapshot(&row([
            "$1", "main", "@1", "0", "shell", "%1", "0", "", "", "",
        ]))
        .unwrap();
        let pane = &snapshot.sessions[0].windows[0].panes[0];
        assert!(pane.title.is_empty());
        assert!(pane.current_command.is_none());
        assert!(pane.working_directory.is_none());
    }

    #[test]
    fn linked_windows_remain_visible_in_each_session() {
        let text = [
            sample(),
            row([
                "$2",
                "reviews",
                "@2",
                "7",
                "auth",
                "%14",
                "0",
                "Agent",
                "codex",
                "/work/auth",
            ]),
        ]
        .concat();
        let snapshot = parse_snapshot(&text).unwrap();
        assert_eq!(snapshot.sessions.len(), 2);
        assert_eq!(
            snapshot.sessions[0].windows[0].id,
            snapshot.sessions[1].windows[0].id
        );
        assert_eq!(snapshot.sessions[1].windows[0].index, 7);
    }

    #[test]
    fn fresh_snapshot_drops_disappeared_panes() {
        let initial = parse_snapshot(
            &[
                sample(),
                row([
                    "$1",
                    "main",
                    "@2",
                    "2",
                    "auth",
                    "%15",
                    "1",
                    "Shell",
                    "zsh",
                    "/work/auth",
                ]),
            ]
            .concat(),
        )
        .unwrap();
        let refreshed = parse_snapshot(&sample()).unwrap();
        assert_eq!(initial.sessions[0].windows[0].panes.len(), 2);
        assert_eq!(refreshed.sessions[0].windows[0].panes.len(), 1);
        assert_eq!(parse_snapshot("").unwrap(), Snapshot::default());
    }

    #[test]
    fn rejects_malformed_fields_ids_indices_and_duplicate_panes() {
        for text in [
            "bad\x1e\n".to_owned(),
            sample().replace("$1", "bad"),
            sample().replace("@2", "@"),
            sample().replace("%14", "%oops"),
            sample().replace("\x1f2\x1f", "\x1fnope\x1f"),
            sample().replace("\x1f0\x1f", "\x1f-1\x1f"),
            sample().replace("\x1e", ""),
            [sample(), sample()].concat(),
            [sample(), "\x1e\n".to_owned()].concat(),
            sample().replace("Agent", "Agent\x1fextra"),
        ] {
            assert!(
                matches!(
                    parse_snapshot(&text),
                    Err(DiscoveryError::InvalidOutput { .. })
                ),
                "accepted {text:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn interprets_command_failure_and_invalid_utf8() {
        use std::os::unix::process::ExitStatusExt;
        let output = Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: b"no server running on /tmp/tmux/default\n".to_vec(),
        };
        let error = interpret_output(output).unwrap_err();
        assert!(error.to_string().contains("no server running"));
        assert!(error.to_string().contains("session is running"));
        let output = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: vec![0xff],
            stderr: Vec::new(),
        };
        assert!(matches!(
            interpret_output(output),
            Err(DiscoveryError::InvalidOutput { .. })
        ));
    }

    #[tokio::test]
    async fn missing_executable_returns_actionable_error() {
        let mut command = Command::new("/nonexistent/agent-workbench-test-tmux");
        let error = run_command(&mut command, COMMAND_TIMEOUT)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, DiscoveryError::Unavailable(source) if source.kind() == io::ErrorKind::NotFound)
        );
        assert!(error.to_string().contains("PATH"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn slow_command_times_out() {
        let mut command = Command::new("/bin/sleep");
        command.arg("5");
        let error = run_command(&mut command, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(matches!(error, DiscoveryError::TimedOut));
    }

    #[tokio::test]
    #[ignore = "requires an installed tmux and a running server; read-only smoke test"]
    async fn discovers_live_tmux() {
        let snapshot = crate::Engine.discover().await.unwrap();
        assert!(!snapshot.sessions.is_empty());
        assert!(
            snapshot
                .sessions
                .iter()
                .all(|session| !session.windows.is_empty())
        );
        assert!(
            snapshot
                .sessions
                .iter()
                .flat_map(|session| &session.windows)
                .all(|window| !window.panes.is_empty())
        );
    }
}
