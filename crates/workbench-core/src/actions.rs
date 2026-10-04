use std::{
    error::Error,
    fmt,
    io::{self, IsTerminal},
    process::Stdio,
    time::Duration,
};

use crate::{DiscoveryError, Engine, Snapshot, WorkItem, WorkItemError};

pub const MAX_INPUT_BYTES: usize = 4096;

// Coding-agent composers can classify rapid character input as a paste and
// treat an immediate Enter as a newline. Allow that burst to settle before
// sending the one submission key (Codex 0.159.3 suppresses Enter for 120 ms).
const REPLY_SETTLE_DELAY: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum ActionError {
    State(WorkItemError),
    Transport(DiscoveryError),
    UnknownWorkItem(String),
    MissingPane { item: String, pane: String },
    ChangedWorkItem(String),
    InvalidInput(String),
    NotInteractive,
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(error) => error.fmt(f),
            Self::Transport(error) => error.fmt(f),
            Self::UnknownWorkItem(id) => write!(f, "Work item {id:?} is no longer registered."),
            Self::MissingPane { item, pane } => write!(
                f,
                "Pane {pane} for {item:?} is missing. Its registration is preserved."
            ),
            Self::InvalidInput(reason) => write!(f, "Cannot send input: {reason}"),
            Self::ChangedWorkItem(id) => write!(
                f,
                "Work item {id:?} changed after reply text was delivered. Enter was not sent; inspect the original pane before retrying."
            ),
            Self::NotInteractive => {
                write!(f, "Opening a work item requires an interactive terminal.")
            }
        }
    }
}

impl Error for ActionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}

impl From<WorkItemError> for ActionError {
    fn from(error: WorkItemError) -> Self {
        Self::State(error)
    }
}

impl From<DiscoveryError> for ActionError {
    fn from(error: DiscoveryError) -> Self {
        Self::Transport(error)
    }
}

/// Single-line replies are submitted explicitly; control keys are not accepted as text.
pub fn validate_agent_input(input: &str) -> Result<(), ActionError> {
    if input.trim().is_empty() {
        return Err(ActionError::InvalidInput("enter a nonempty reply".into()));
    }
    if input.len() > MAX_INPUT_BYTES {
        return Err(ActionError::InvalidInput(format!(
            "reply exceeds {MAX_INPUT_BYTES} UTF-8 bytes"
        )));
    }
    if input.chars().any(char::is_control) {
        return Err(ActionError::InvalidInput(
            "use a single line with no control characters".into(),
        ));
    }
    Ok(())
}

impl Engine {
    pub(crate) fn registered_item(&self, id: &str) -> Result<WorkItem, ActionError> {
        self.work_items()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| ActionError::UnknownWorkItem(id.to_owned()))
    }

    /// Send literal UTF-8 text followed by one Enter, without changing pane focus.
    /// Never retries automatically: a failed command may already have delivered input.
    pub async fn send_agent_input(&self, id: &str, input: &str) -> Result<(), ActionError> {
        validate_agent_input(input)?;
        let item = self.registered_item(id)?;
        let snapshot = self.discover().await?;
        target_for(&item, &snapshot)?;
        self.tmux.execute(&input_args(&item.pane_id, input)).await?;
        tokio::time::sleep(REPLY_SETTLE_DELAY).await;
        // Do not silently send Enter to a remapped/replaced registration after
        // the pause. A partial text delivery is not retried or rolled back.
        if self.registered_item(id)? != item {
            return Err(ActionError::ChangedWorkItem(id.into()));
        }
        target_for(&item, &self.discover().await?)?;
        self.tmux.execute(&submit_args(&item.pane_id)).await?;
        Ok(())
    }

    /// Switch the current tmux client, or attach this terminal until the user detaches.
    /// The caller must suspend terminal rendering/raw mode before this operation.
    pub async fn focus_work_item(&self, id: &str) -> Result<(), ActionError> {
        let item = self.registered_item(id)?;
        let snapshot = self.discover().await?;
        let target = target_for(&item, &snapshot)?;
        if inside_tmux() {
            self.tmux.execute(&focus_args(&target, true)).await?;
        } else {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                return Err(ActionError::NotInteractive);
            }
            let mut command = self.tmux.command();
            command
                .args(focus_args(&target, false))
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .kill_on_drop(true);
            // An interactive attachment lasts until detach, so has no command timeout.
            let status = command
                .status()
                .await
                .map_err(DiscoveryError::Unavailable)?;
            if !status.success() {
                return Err(DiscoveryError::CommandFailed {
                    code: status.code(),
                    message: "Could not attach to the mapped pane; check the pane and terminal"
                        .into(),
                }
                .into());
            }
        }
        Ok(())
    }
}

fn inside_tmux() -> bool {
    std::env::var_os("TMUX").is_some_and(|value| !value.is_empty())
}

pub(crate) fn target_for(item: &WorkItem, snapshot: &Snapshot) -> Result<String, ActionError> {
    for session in &snapshot.sessions {
        for window in &session.windows {
            if window.panes.iter().any(|pane| pane.id == item.pane_id) {
                return Ok(format!("{}:{}.{}", session.id, window.id, item.pane_id));
            }
        }
    }
    Err(ActionError::MissingPane {
        item: item.id.clone(),
        pane: item.pane_id.clone(),
    })
}

fn focus_args(target: &str, inside: bool) -> Vec<String> {
    vec![
        if inside {
            "switch-client"
        } else {
            "attach-session"
        }
        .into(),
        "-E".into(),
        "-t".into(),
        target.into(),
    ]
}

fn input_args(pane: &str, input: &str) -> Vec<String> {
    // tmux parses semicolons in argv. Encode literal UTF-8 bytes so reply text
    // cannot become tmux commands, flags, formats, or key names.
    let mut args = vec!["send-keys".into(), "-H".into(), "-t".into(), pane.into()];
    args.extend(input.as_bytes().iter().map(|byte| format!("{byte:02x}")));
    args
}

fn submit_args(pane: &str) -> Vec<String> {
    vec!["send-keys".into(), "-t".into(), pane.into(), "Enter".into()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pane, Session, Window, WorkItemKind};

    fn item() -> WorkItem {
        WorkItem {
            id: "ABC-123".into(),
            title: "Fix retry".into(),
            repository: "/work/repo".into(),
            workspace: "/work/task".into(),
            branch: None,
            kind: WorkItemKind::Implementation,
            pane_id: "%14".into(),
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@2".into(),
                    index: 7,
                    name: "auth".into(),
                    panes: vec![Pane {
                        id: "%14".into(),
                        index: 1,
                        title: "Agent".into(),
                        current_command: None,
                        working_directory: None,
                    }],
                }],
            }],
        }
    }

    #[test]
    fn resolves_exact_ids_and_reports_disappeared_panes() {
        assert_eq!(target_for(&item(), &snapshot()).unwrap(), "$1:@2.%14");
        assert!(matches!(
            target_for(&item(), &Snapshot::default()),
            Err(ActionError::MissingPane { .. })
        ));
        assert_eq!(
            focus_args("$1:@2.%14", true),
            ["switch-client", "-E", "-t", "$1:@2.%14"]
        );
        assert_eq!(
            focus_args("$1:@2.%14", false),
            ["attach-session", "-E", "-t", "$1:@2.%14"]
        );
    }

    #[test]
    fn encodes_reply_bytes_literally_and_submits_exactly_once() {
        for input in [
            ";",
            "Enter",
            "-R",
            "#{pane_id}",
            "C-c ; kill-server",
            "'$(echo x)' \\ λ🙂",
        ] {
            let args = input_args("%14", input);
            assert_eq!(&args[..4], ["send-keys", "-H", "-t", "%14"]);
            let decoded: Vec<u8> = args[4..]
                .iter()
                .map(|value| u8::from_str_radix(value, 16).unwrap())
                .collect();
            assert_eq!(decoded, input.as_bytes());
            assert_eq!(submit_args("%14"), ["send-keys", "-t", "%14", "Enter"]);
        }
    }

    #[test]
    fn rejects_empty_multiline_control_and_oversized_replies() {
        for input in ["", "  ", "yes\nno", "\r", "\t", "\x1b", "\0"] {
            assert!(validate_agent_input(input).is_err());
        }
        assert!(validate_agent_input(&"a".repeat(MAX_INPUT_BYTES + 1)).is_err());
        assert!(validate_agent_input(&"a".repeat(MAX_INPUT_BYTES)).is_ok());
        assert!(validate_agent_input(" yes, preserve λ backwards compatibility ").is_ok());
    }

    #[tokio::test]
    async fn unknown_registration_fails_before_attempting_a_transport_action() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("state.json"));
        assert!(matches!(
            engine.send_agent_input("missing", "yes").await,
            Err(ActionError::UnknownWorkItem(_))
        ));
        assert!(matches!(
            engine.focus_work_item("missing").await,
            Err(ActionError::UnknownWorkItem(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn transport_failure_and_stale_snapshot_preserve_the_registration_and_do_not_retry() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-tmux");
        let output = directory.path().join("snapshot");
        let log = directory.path().join("log");
        let script = format!(
            "#!/bin/sh\nset -eu\nprintf 'INVOKE\\n' >> '{log}'\nif [ \"$2\" = list-panes ]; then cat '{output}'; else printf 'pane vanished\\n' >&2; exit 1; fi\n",
            log = log.display(),
            output = output.display()
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            &output,
            "$1\x1fmain\x1f@2\x1f7\x1fauth\x1f%14\x1f1\x1fAgent\x1fcodex\x1f/work/task\x1e\n",
        )
        .unwrap();
        let mut engine = Engine::new(directory.path().join("state.json"));
        engine.tmux.executable = executable.into_os_string();
        engine.register_work_item(item()).unwrap();
        let error = engine.send_agent_input("ABC-123", "yes").await.unwrap_err();
        assert!(error.to_string().contains("pane vanished"));
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
        assert_eq!(engine.work_items().unwrap(), [item()]);
        fs::write(&output, "").unwrap();
        assert!(matches!(
            engine.send_agent_input("ABC-123", "yes").await,
            Err(ActionError::MissingPane { .. })
        ));
        assert!(matches!(
            engine.focus_work_item("ABC-123").await,
            Err(ActionError::MissingPane { .. })
        ));
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 4);
        assert_eq!(engine.work_items().unwrap(), [item()]);
    }

    #[cfg(unix)]
    struct ReplyFixture {
        directory: tempfile::TempDir,
        engine: Engine,
    }

    #[cfg(unix)]
    impl ReplyFixture {
        fn new() -> Self {
            use std::{fs, os::unix::fs::PermissionsExt};
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("fake-tmux");
            fs::write(
                &executable,
                format!(
                    r#"#!/bin/sh
set -eu
base='{}'
[ "$1" = '-N' ] && shift
printf 'CALL\n' >> "$base/log"
case "$1" in
list-panes)
  [ ! -e "$base/discovery-failure" ] || exit 31
  exec /bin/cat "$base/snapshot" ;;
send-keys)
  if [ "$2" = '-H' ]; then
    printf '%s\n' "$@" > "$base/text"
    [ ! -e "$base/text-failure" ] || exit 32
  else
    printf '%s\n' "$@" >> "$base/submit"
    [ ! -e "$base/submit-failure" ] || exit 33
  fi ;;
*) exit 99 ;;
esac
"#,
                    directory.path().display()
                ),
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(
                directory.path().join("snapshot"),
                "$1\x1fmain\x1f@2\x1f7\x1fauth\x1f%14\x1f1\x1fAgent\x1fcodex\x1f/work/task\x1e\n",
            )
            .unwrap();
            let mut engine = Engine::new(directory.path().join("state.json"));
            engine.tmux.executable = executable.into_os_string();
            engine.register_work_item(item()).unwrap();
            Self { directory, engine }
        }
        fn path(&self, name: &str) -> std::path::PathBuf {
            self.directory.path().join(name)
        }
        async fn text_delivered(&self) {
            tokio::time::timeout(Duration::from_secs(3), async {
                while !self.path("text").exists() {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .expect("text command was not reached");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reply_text_and_one_enter_are_separate_commands_outside_the_paste_window() {
        use std::fs;
        let fixture = ReplyFixture::new();
        let input = "yes λ🙂 Enter ; kill-server";
        fixture
            .engine
            .send_agent_input("ABC-123", input)
            .await
            .unwrap();
        let text = fs::read_to_string(fixture.path("text")).unwrap();
        let expected = input_args("%14", input).join("\n") + "\n";
        assert_eq!(text, expected);
        assert_eq!(
            fs::read_to_string(fixture.path("submit")).unwrap(),
            "send-keys\n-t\n%14\nEnter\n"
        );
        let text_at = fs::metadata(fixture.path("text"))
            .unwrap()
            .modified()
            .unwrap();
        let submit_at = fs::metadata(fixture.path("submit"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(submit_at.duration_since(text_at).unwrap() >= REPLY_SETTLE_DELAY);
        assert_eq!(
            fs::read_to_string(fixture.path("log"))
                .unwrap()
                .lines()
                .count(),
            4
        );
        assert_eq!(fixture.engine.work_items().unwrap(), [item()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_changed_registration_or_disappearing_pane_after_text_delivery_never_gets_enter() {
        use std::fs;
        for change in ["edit", "remap", "unregister", "pane", "discovery"] {
            let fixture = ReplyFixture::new();
            let sending = fixture.engine.send_agent_input("ABC-123", "continue");
            tokio::pin!(sending);
            tokio::select! {
                result = &mut sending => panic!("send finished before text observation: {result:?}"),
                _ = fixture.text_delivered() => {},
            }
            assert!(!fixture.path("submit").exists());
            match change {
                "edit" => {
                    fixture
                        .engine
                        .update_work_item_description(&item(), "Changed")
                        .unwrap();
                }
                "remap" => {
                    fixture.engine.unregister_work_item(&item()).unwrap();
                    let mut remapped = item();
                    remapped.pane_id = "%15".into();
                    fixture.engine.register_work_item(remapped).unwrap();
                }
                "unregister" => {
                    fixture.engine.unregister_work_item(&item()).unwrap();
                }
                "pane" => fs::write(fixture.path("snapshot"), "").unwrap(),
                _ => fs::write(fixture.path("discovery-failure"), "").unwrap(),
            }
            let failure = sending.await.unwrap_err();
            match change {
                "edit" | "remap" => assert!(matches!(failure, ActionError::ChangedWorkItem(_))),
                "unregister" => assert!(matches!(failure, ActionError::UnknownWorkItem(_))),
                "pane" => assert!(matches!(failure, ActionError::MissingPane { .. })),
                _ => assert!(matches!(failure, ActionError::Transport(_))),
            }
            assert!(!fixture.path("submit").exists(), "{change}");
            assert_eq!(
                fs::read_to_string(fixture.path("text"))
                    .unwrap()
                    .lines()
                    .filter(|line| *line == "send-keys")
                    .count(),
                1
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn text_and_enter_failures_preserve_state_and_never_retry_input() {
        use std::fs;
        for stage in ["text", "submit"] {
            let fixture = ReplyFixture::new();
            fs::write(fixture.path(&format!("{stage}-failure")), "").unwrap();
            assert!(
                fixture
                    .engine
                    .send_agent_input("ABC-123", "continue")
                    .await
                    .is_err()
            );
            assert_eq!(fixture.engine.work_items().unwrap(), [item()]);
            if stage == "text" {
                assert!(!fixture.path("submit").exists());
                assert_eq!(
                    fs::read_to_string(fixture.path("log"))
                        .unwrap()
                        .lines()
                        .count(),
                    2
                );
            } else {
                assert_eq!(
                    fs::read_to_string(fixture.path("submit")).unwrap(),
                    "send-keys\n-t\n%14\nEnter\n"
                );
                assert_eq!(
                    fs::read_to_string(fixture.path("log"))
                        .unwrap()
                        .lines()
                        .count(),
                    4
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires tmux; creates and cleans up only its own isolated test server"]
    async fn isolated_tmux_input_is_literal_and_missing_panes_are_safe() {
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
        let socket = directory.path().join("tmux.sock");
        let started = std::process::Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "m3-test",
                "-x",
                "200",
                "-y",
                "40",
                "cat",
            ])
            .output()
            .unwrap();
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let _server = Server(socket.clone());
        let mut engine = Engine::new(directory.path().join("work-items.json"));
        engine.tmux.socket = Some(socket.clone());
        let snapshot = engine.discover().await.unwrap();
        let pane_id = snapshot.sessions[0].windows[0].panes[0].id.clone();
        let mut registered = item();
        registered.pane_id = pane_id.clone();
        engine.register_work_item(registered.clone()).unwrap();
        let reply = "yes λ🙂 Enter C-c -R #{pane_id} ; kill-server";
        engine.send_agent_input("ABC-123", reply).await.unwrap();
        let mut received = false;
        for _ in 0..50 {
            let captured = std::process::Command::new("tmux")
                .arg("-S")
                .arg(&socket)
                .args(["capture-pane", "-p", "-t", &pane_id])
                .output()
                .unwrap();
            // Canonical-mode cat echoes a submitted line back after the tty's
            // input echo. One visible copy proves typing, not submission.
            if String::from_utf8_lossy(&captured.stdout)
                .lines()
                .filter(|line| line.trim_end() == reply)
                .count()
                == 2
            {
                received = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            received,
            "literal Unicode reply was not submitted exactly once"
        );
        let created = std::process::Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["new-window", "-d", "-t", "m3-test", "cat"])
            .output()
            .unwrap();
        assert!(created.status.success());
        let killed = std::process::Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["kill-pane", "-t", &pane_id])
            .output()
            .unwrap();
        assert!(killed.status.success());
        assert!(matches!(
            engine.send_agent_input("ABC-123", "yes").await,
            Err(ActionError::MissingPane { .. })
        ));
        assert!(matches!(
            engine.focus_work_item("ABC-123").await,
            Err(ActionError::MissingPane { .. })
        ));
        assert_eq!(engine.work_items().unwrap(), [registered]);
    }
}
