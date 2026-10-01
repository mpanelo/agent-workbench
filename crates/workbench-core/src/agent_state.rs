//! Conservative, ephemeral terminal observations; no transcript discovery.

use std::collections::{HashMap, HashSet};

use tokio::task::JoinSet;

use crate::tmux::PaneObservation;
use crate::{AgentStatus, Engine, PaneAvailability, Snapshot, WorkItemError, WorkItemState};

const MAX_CONCURRENT_CAPTURES: usize = 4;

impl Engine {
    /// Resolve registrations and infer status from live, read-only pane observations.
    /// Every refresh replaces previous evidence. An individual capture failure does
    /// not fail the list or remove the registration. Dynamic status is never saved.
    pub async fn observe_work_item_states(
        &self,
        snapshot: Option<&Snapshot>,
    ) -> Result<Vec<WorkItemState>, WorkItemError> {
        let mut states = self.work_item_states(snapshot)?;
        let Some(snapshot) = snapshot else {
            return Ok(states);
        };
        let panes: HashMap<_, _> = snapshot
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
            .map(|pane| (pane.id.as_str(), pane))
            .collect();
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for state in &mut states {
            if state.pane != PaneAvailability::Present {
                continue;
            }
            let command = panes
                .get(state.item.pane_id.as_str())
                .and_then(|pane| pane.current_command.as_deref());
            if !supported(command) {
                state.status_detail = "Unsupported or unavailable foreground command.".into();
            } else if seen.insert(state.item.pane_id.clone()) {
                targets.push(state.item.pane_id.clone());
            }
        }
        let mut captures = JoinSet::new();
        let mut observations = HashMap::new();
        let mut targets = targets.into_iter();
        loop {
            while captures.len() < MAX_CONCURRENT_CAPTURES {
                let Some(id) = targets.next() else { break };
                let tmux = self.tmux.clone();
                captures.spawn(async move {
                    let result = tmux.observe_pane(&id).await;
                    (id, result)
                });
            }
            let Some(result) = captures.join_next().await else {
                break;
            };
            if let Ok((id, result)) = result {
                let detected = match result {
                    Ok(observation) => infer(&observation),
                    Err(_) => (
                        AgentStatus::Unknown,
                        "Pane capture failed or changed; retrying next refresh.",
                    ),
                };
                observations.insert(id, detected);
            }
        }
        for state in &mut states {
            if let Some((status, detail)) = observations.get(&state.item.pane_id) {
                state.status = *status;
                state.status_detail = (*detail).into();
            }
        }
        Ok(states)
    }
}

fn supported(command: Option<&str>) -> bool {
    command.is_some_and(|command| {
        std::path::Path::new(command)
            .file_name()
            .is_some_and(|name| name == "codex")
    })
}

fn infer(observation: &PaneObservation) -> (AgentStatus, &'static str) {
    use AgentStatus::*;
    if observation.dead || observation.in_mode || !supported(Some(&observation.command)) {
        return (
            Unknown,
            "Pane is dead, in copy/view mode, or no longer running a supported agent.",
        );
    }
    // tmux capture-pane without -e supplies plain visible text, not ANSI bytes.
    // Unexpected controls are inconclusive rather than silently stripped.
    if observation
        .screen
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return (Unknown, "Pane contains unexpected control characters.");
    }
    let lines: Vec<_> = observation
        .screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let Some(last) = lines.last() else {
        return (Unknown, "Pane has no recognizable agent UI.");
    };
    // Require a current dialog footer, a known title, and a selected Yes/No
    // choice. A quoted/historical approval question alone is not evidence.
    let confirm = matches!(
        *last,
        "Press enter to confirm or esc to cancel"
            | "Press Enter to confirm or Esc to cancel"
            | "Press enter to confirm or select"
            | "Press Enter to confirm or select"
    );
    if confirm {
        let tail = &lines[lines.len().saturating_sub(40)..];
        let title = tail.iter().any(|line| {
            matches!(
                *line,
                "Would you like to run the following command?"
                    | "Would you like to make the following edits?"
                    | "Would you like to grant these permissions?"
                    | "Would you like to send input to the existing terminal?"
            )
        });
        let choice = tail.iter().any(|line| {
            line.strip_prefix("› ")
                .and_then(|choice| choice.split_once(". "))
                .is_some_and(|(number, text)| {
                    number.parse::<u8>().is_ok()
                        && (text.starts_with("Yes, ") || text.starts_with("No, "))
                })
        });
        if title && choice {
            return (
                WaitingForInput,
                "Codex approval dialog is awaiting a decision.",
            );
        }
        return (Unknown, "Unrecognized confirmation dialog.");
    }
    // The ready composer is at the bottom, immediately above 1–3 footer rows.
    // No search through scrollback or arbitrary prose for 'done'/'waiting'.
    if !last.ends_with("? for shortcuts") {
        return (
            Unknown,
            "Codex screen is inconclusive (possibly a menu or truncated pane).",
        );
    }
    let prompt = lines
        .iter()
        .rposition(|line| *line == "›" || line.starts_with("› "));
    let Some(prompt) = prompt.filter(|index| (1..=3).contains(&(lines.len() - index - 1))) else {
        return (Unknown, "Codex composer is not visible.");
    };
    let mut previous = lines[..prompt].iter().rev();
    let mut activity = previous.next().copied();
    if activity.is_some_and(|line| line.starts_with("└ Tip: ")) {
        activity = previous.next().copied();
    }
    if activity.is_some_and(running_indicator) {
        return (Running, "Codex displays an active interruptible turn.");
    }
    if lines[..prompt]
        .iter()
        .rev()
        .take(5)
        .any(|line| line.contains("to interrupt"))
    {
        return (Unknown, "Codex activity indicator is unrecognized.");
    }
    if activity.is_some_and(completion_indicator) {
        return (
            Complete,
            "Codex turn finished; overall task completion is unverified.",
        );
    }
    (
        Idle,
        "Codex composer is ready; no explicit input request or completion marker detected.",
    )
}

fn running_indicator(line: &str) -> bool {
    let Some((label, rest)) = line.split_once(" (") else {
        return false;
    };
    matches!(
        label,
        "• Working" | "• Thinking" | "• Running" | "• Searching"
    ) && rest
        .strip_suffix(" • esc to interrupt)")
        .is_some_and(duration)
}

fn completion_indicator(line: &str) -> bool {
    let line = line.trim_matches('─').trim();
    line.strip_prefix("Worked for ").is_some_and(|rest| {
        duration(
            rest.split(" • ")
                .next()
                .unwrap_or(rest)
                .trim_end_matches('─')
                .trim(),
        )
    })
}

fn duration(text: &str) -> bool {
    !text.trim().is_empty()
        && text.split_whitespace().all(|part| {
            let Some(number) = part.strip_suffix(['h', 'm', 's']) else {
                return false;
            };
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOTER: &str = "\n› Ask Codex to do anything\n\n  GPT-6.1-Sol high · ~/work · Task\n  ← for agents · ? for shortcuts\n";
    const APPROVAL: &str = "Would you like to run the following command?\n\n$ cargo test\n\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)\n\nPress enter to confirm or esc to cancel\n";

    fn observation(screen: &str) -> PaneObservation {
        PaneObservation {
            command: "codex".into(),
            dead: false,
            in_mode: false,
            screen: screen.into(),
        }
    }

    #[test]
    fn common_codex_approvals_require_live_dialog_chrome() {
        for title in [
            "Would you like to run the following command?",
            "Would you like to make the following edits?",
            "Would you like to grant these permissions?",
            "Would you like to send input to the existing terminal?",
        ] {
            let screen = APPROVAL.replace("Would you like to run the following command?", title);
            assert_eq!(infer(&observation(&screen)).0, AgentStatus::WaitingForInput);
            assert_eq!(
                infer(&observation(&screen.replace(
                    "Yes, proceed (y)",
                    "No, and tell Codex what to do differently (esc)"
                )))
                .0,
                AgentStatus::WaitingForInput
            );
            assert_eq!(
                infer(&observation(
                    &screen.replace("or esc to cancel", "or select")
                ))
                .0,
                AgentStatus::WaitingForInput
            );
        }
        for screen in [
            "Would you like to run the following command?",
            "› 1. Yes, proceed (y)\nPress enter to confirm or esc to cancel",
            "Would you like to run the following command?\nPress enter to confirm or esc to cancel",
        ] {
            assert_eq!(infer(&observation(screen)).0, AgentStatus::Unknown);
        }
    }

    #[test]
    fn current_bottom_ui_wins_over_historical_approval_or_completion() {
        let screen = format!(
            "{APPROVAL}\nWorked for 18s • 7:08 PM\n• Working (5m 19s • esc to interrupt)\n└ Tip: Use /export.\n{FOOTER}"
        );
        assert_eq!(infer(&observation(&screen)).0, AgentStatus::Running);
        let screen = format!("{APPROVAL}\nWorked for 18s • 7:08 PM\n{FOOTER}");
        assert_eq!(infer(&observation(&screen)).0, AgentStatus::Complete);
        let screen = format!("{APPROVAL}\n• New output λ🙂\n{FOOTER}");
        assert_eq!(infer(&observation(&screen)).0, AgentStatus::Idle);
    }

    #[test]
    fn empty_ready_composer_and_legacy_context_footer_are_idle() {
        assert_eq!(infer(&observation(FOOTER)).0, AgentStatus::Idle);
        assert_eq!(
            infer(&observation("›\n  93% context left · ? for shortcuts\n")).0,
            AgentStatus::Idle
        );
        assert_eq!(
            infer(&observation(&format!(
                "─ Worked for 1m 5s ──────\n{FOOTER}"
            )))
            .0,
            AgentStatus::Complete
        );
        assert!(!completion_indicator("Worked for a while"));
        assert!(!running_indicator("• Working (soon • esc to interrupt)"));
        assert!(!running_indicator("• Working (  • esc to interrupt)"));
    }

    #[test]
    fn unsupported_dead_copy_mode_and_ambiguous_screens_are_unknown() {
        for screen in [
            "",
            "done",
            "Would you like to continue?",
            "• Working (6s • esc to interrupt)",
            "›\nMenu\n",
            "\x1b[32m›\n? for shortcuts",
        ] {
            assert_eq!(infer(&observation(screen)).0, AgentStatus::Unknown);
        }
        for command in ["fish", "node", "claude", "my-codex", ""] {
            let mut pane = observation(APPROVAL);
            pane.command = command.into();
            assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        }
        let mut pane = observation(APPROVAL);
        pane.in_mode = true;
        assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        pane.in_mode = false;
        pane.dead = true;
        assert_eq!(infer(&pane).0, AgentStatus::Unknown);
        assert!(supported(Some("/opt/bin/codex")));
    }

    #[test]
    fn only_waiting_and_complete_need_attention_and_all_labels_are_stable() {
        for (status, label, attention) in [
            (AgentStatus::Running, "RUNNING", false),
            (AgentStatus::WaitingForInput, "WAITING_FOR_INPUT", true),
            (AgentStatus::Idle, "IDLE", false),
            (AgentStatus::Complete, "COMPLETE", true),
            (AgentStatus::Unknown, "UNKNOWN", false),
        ] {
            assert_eq!(status.to_string(), label);
            assert_eq!(status.needs_attention(), attention);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refresh_replaces_evidence_deduplicates_panes_and_isolates_failures() {
        use crate::{Pane, Session, Window, WorkItem, WorkItemKind};
        use std::{fs, os::unix::fs::PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-tmux");
        let log = directory.path().join("log");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> '{}'
if [ \"$2\" != display-message ]; then exit 2; fi
cat '{}/'$5
",
                log.display(),
                directory.path().display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("state.json");
        let mut engine = Engine::new(&path);
        engine.tmux.executable = executable.into_os_string();
        let items: Vec<_> = [
            ("A", "%14"),
            ("B", "%14"),
            ("C", "%15"),
            ("D", "%99"),
            ("E", "%16"),
        ]
        .into_iter()
        .map(|(id, pane_id)| WorkItem {
            id: id.into(),
            title: "Task".into(),
            repository: "/work".into(),
            workspace: "/work".into(),
            branch: None,
            kind: WorkItemKind::Implementation,
            pane_id: pane_id.into(),
        })
        .collect();
        // Use an existing registry fixture; persistence locking is tested separately.
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version": 1, "work_items": items})).unwrap(),
        )
        .unwrap();
        let stored = fs::read(&path).unwrap();
        let snapshot = Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 0,
                    name: "work".into(),
                    panes: [("%14", "codex"), ("%15", "fish"), ("%16", "codex")]
                        .into_iter()
                        .map(|(id, command)| Pane {
                            id: id.into(),
                            index: 0,
                            title: "Agent".into(),
                            current_command: Some(command.into()),
                            working_directory: None,
                        })
                        .collect(),
                }],
            }],
        };
        let fixture = |command: &str, screen: &str| {
            format!("\x1e{command}\x1f0\x1f0\x1e\n{screen}\x1e{command}\x1f0\x1f0\x1e\n")
        };
        for (screen, expected) in [
            (APPROVAL.into(), AgentStatus::WaitingForInput),
            (
                format!("• Working (2s • esc to interrupt)\n{FOOTER}"),
                AgentStatus::Running,
            ),
            (format!("Worked for 2s\n{FOOTER}"), AgentStatus::Complete),
            (FOOTER.into(), AgentStatus::Idle),
            ("unrecognized menu".into(), AgentStatus::Unknown),
        ] {
            fs::write(directory.path().join("%14"), fixture("codex", &screen)).unwrap();
            let states = engine
                .observe_work_item_states(Some(&snapshot))
                .await
                .unwrap();
            assert_eq!(states[0].status, expected);
            assert_eq!(states[1].status, expected);
            assert_eq!(states[2].status, AgentStatus::Unknown);
            assert_eq!(states[3].pane, PaneAvailability::Missing);
            assert_eq!(states[4].status, AgentStatus::Unknown);
            assert!(states[4].status_detail.contains("capture failed"));
        }
        let calls = fs::read_to_string(&log).unwrap();
        // One capture for the shared pane and one failed capture per refresh.
        assert_eq!(calls.lines().count(), 10);
        for call in calls.lines() {
            assert!(call.contains("capture-pane -p -J -t"));
            assert!(!call.contains("send-keys"));
            assert!(!call.contains("%15"));
        }
        // A shell replacing Codex invalidates even a screen that looks like approval.
        fs::write(directory.path().join("%14"), fixture("fish", APPROVAL)).unwrap();
        assert_eq!(
            engine
                .observe_work_item_states(Some(&snapshot))
                .await
                .unwrap()[0]
                .status,
            AgentStatus::Unknown
        );
        let calls_before = fs::read_to_string(&log).unwrap();
        for snapshot in [None, Some(&Snapshot::default())] {
            let states = engine.observe_work_item_states(snapshot).await.unwrap();
            assert!(
                states
                    .iter()
                    .all(|state| state.status == AgentStatus::Unknown)
            );
        }
        assert_eq!(fs::read_to_string(&log).unwrap(), calls_before);
        assert_eq!(fs::read(&path).unwrap(), stored);
    }

    #[tokio::test]
    async fn observation_with_no_items_does_not_need_a_transport() {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = Engine::new(directory.path().join("not-created.json"));
        engine.tmux.executable = "nonexistent-tmux".into();
        assert!(
            engine
                .observe_work_item_states(Some(&Snapshot::default()))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!directory.path().join("not-created.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires tmux; replays Codex UI fixtures on its own isolated server"]
    async fn isolated_tmux_observations_handle_copy_mode_and_disappearance() {
        use crate::{WorkItem, WorkItemKind};
        use std::{fs, process::Command};
        struct Server(std::path::PathBuf);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = Command::new("tmux")
                    .arg("-S")
                    .arg(&self.0)
                    .arg("kill-server")
                    .output();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("tmux.sock");
        // An inert process named codex replays fixtures; no agent/API is invoked.
        // Compile a helper rather than copying a platform-signed system binary.
        let executable = directory.path().join("codex");
        let source = directory.path().join("fixture.rs");
        fs::write(
            &source,
            r#"fn main() {
            let screen = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
            print!("\x1b[2J\x1b[H{}", screen);
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            loop { std::thread::park(); }
        }"#,
        )
        .unwrap();
        let compiled = Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let fixture = directory.path().join("screen");
        fs::write(&fixture, APPROVAL).unwrap();
        let started = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "m4-test",
                "-x",
                "120",
                "-y",
                "40",
            ])
            .arg(&executable)
            .arg(&fixture)
            .output()
            .unwrap();
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let _server = Server(socket.clone());
        let mut engine = Engine::new(directory.path().join("state.json"));
        engine.tmux.socket = Some(socket);
        let snapshot = engine.discover().await.unwrap();
        let id = snapshot.sessions[0].windows[0].panes[0].id.clone();
        engine
            .register_work_item(WorkItem {
                id: "M4".into(),
                title: "Observe".into(),
                repository: "/work".into(),
                workspace: "/work".into(),
                branch: None,
                kind: WorkItemKind::Implementation,
                pane_id: id.clone(),
            })
            .unwrap();
        for (screen, expected) in [
            (APPROVAL.into(), AgentStatus::WaitingForInput),
            (
                format!("• Working (3s • esc to interrupt)\n{FOOTER}"),
                AgentStatus::Running,
            ),
            (format!("Worked for 3s\n{FOOTER}"), AgentStatus::Complete),
            (FOOTER.into(), AgentStatus::Idle),
        ] {
            fs::write(&fixture, screen).unwrap();
            engine
                .tmux
                .execute(&[
                    "respawn-pane".into(),
                    "-k".into(),
                    "-t".into(),
                    id.clone(),
                    executable.to_string_lossy().into_owned(),
                    fixture.to_string_lossy().into_owned(),
                ])
                .await
                .unwrap();
            let mut observed = AgentStatus::Unknown;
            for _ in 0..50 {
                let snapshot = engine.discover().await.unwrap();
                observed = engine
                    .observe_work_item_states(Some(&snapshot))
                    .await
                    .unwrap()[0]
                    .status;
                if observed == expected {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert_eq!(observed, expected);
        }
        engine
            .tmux
            .execute(&["copy-mode".into(), "-t".into(), id.clone()])
            .await
            .unwrap();
        let snapshot = engine.discover().await.unwrap();
        assert_eq!(
            engine
                .observe_work_item_states(Some(&snapshot))
                .await
                .unwrap()[0]
                .status,
            AgentStatus::Unknown
        );
        engine
            .tmux
            .execute(&[
                "new-window".into(),
                "-d".into(),
                "-t".into(),
                "m4-test".into(),
                "cat".into(),
            ])
            .await
            .unwrap();
        engine
            .tmux
            .execute(&["kill-pane".into(), "-t".into(), id])
            .await
            .unwrap();
        let snapshot = engine.discover().await.unwrap();
        let states = engine
            .observe_work_item_states(Some(&snapshot))
            .await
            .unwrap();
        assert_eq!(states[0].pane, PaneAvailability::Missing);
        assert_eq!(states[0].status, AgentStatus::Unknown);
        assert_eq!(engine.work_items().unwrap().len(), 1);
    }
}
