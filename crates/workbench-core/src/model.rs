use std::path::{Path, PathBuf};

/// A complete discovery result; each refresh replaces the previous snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub sessions: Vec<Session>,
}

impl Snapshot {
    /// A view of recognized foreground agents, without altering raw discovery.
    /// Empty windows/sessions are omitted; linked windows keep their hierarchy.
    pub fn coding_agent_panes(&self) -> Self {
        let mut filtered = self.clone();
        for session in &mut filtered.sessions {
            for window in &mut session.windows {
                window.panes.retain(Pane::is_coding_agent);
            }
            session.windows.retain(|window| !window.panes.is_empty());
        }
        filtered
            .sessions
            .retain(|session| !session.windows.is_empty());
        filtered
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// Stable tmux session ID, such as `$1`.
    pub id: String,
    pub name: String,
    pub windows: Vec<Window>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// Stable tmux window ID, such as `@2`; may be linked to multiple sessions.
    pub id: String,
    pub index: u32,
    pub name: String,
    pub panes: Vec<Pane>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    /// Stable tmux pane ID, such as `%14`.
    pub id: String,
    pub index: u32,
    pub title: String,
    pub current_command: Option<String>,
    pub working_directory: Option<PathBuf>,
}

impl Pane {
    /// Conservative command-name recognition, not an agent-status detector.
    /// Titles and generic runtimes cannot reliably identify a running agent.
    pub fn is_coding_agent(&self) -> bool {
        self.current_command
            .as_deref()
            .and_then(|command| Path::new(command).file_name())
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                matches!(
                    name,
                    "codex" | "claude" | "gemini" | "opencode" | "aider" | "goose" | "amp"
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, command: Option<&str>) -> Pane {
        Pane {
            id: id.into(),
            index: 0,
            title: "codex claude".into(),
            current_command: command.map(str::to_owned),
            working_directory: None,
        }
    }

    #[test]
    fn recognizes_only_exact_agent_executables_not_titles_or_runtimes() {
        for command in [
            "codex",
            "claude",
            "gemini",
            "opencode",
            "aider",
            "goose",
            "amp",
            "/opt/bin/codex",
            "/path with spaces/claude",
        ] {
            assert!(pane("%1", Some(command)).is_coding_agent(), "{command}");
        }
        for command in [
            None,
            Some(""),
            Some("fish"),
            Some("node"),
            Some("python"),
            Some("vim"),
            Some("workbench"),
            Some("my-codex"),
            Some("codex-helper"),
            Some("claude --help"),
            Some("2.1.0"),
        ] {
            assert!(!pane("%1", command).is_coding_agent(), "{command:?}");
        }
    }

    #[test]
    fn agent_view_prunes_empty_hierarchy_preserves_ids_order_and_raw_discovery() {
        let window = Window {
            id: "@1".into(),
            index: 3,
            name: "task".into(),
            panes: vec![
                pane("%1", Some("fish")),
                pane("%2", Some("claude")),
                pane("%3", Some("codex")),
            ],
        };
        let empty = Window {
            id: "@2".into(),
            panes: vec![pane("%4", None)],
            ..window.clone()
        };
        let snapshot = Snapshot {
            sessions: vec![
                Session {
                    id: "$1".into(),
                    name: "main".into(),
                    windows: vec![empty.clone(), window.clone()],
                },
                Session {
                    id: "$2".into(),
                    name: "linked".into(),
                    windows: vec![window],
                },
                Session {
                    id: "$3".into(),
                    name: "shells".into(),
                    windows: vec![empty],
                },
            ],
        };
        let original = snapshot.clone();
        let filtered = snapshot.coding_agent_panes();
        assert_eq!(snapshot, original);
        assert_eq!(filtered.sessions.len(), 2);
        for session in &filtered.sessions {
            assert_eq!(session.windows.len(), 1);
            let window = &session.windows[0];
            assert_eq!(window.id, "@1");
            assert_eq!(window.index, 3);
            assert_eq!(
                window
                    .panes
                    .iter()
                    .map(|pane| pane.id.as_str())
                    .collect::<Vec<_>>(),
                ["%2", "%3"]
            );
        }
        assert_eq!(
            Snapshot::default().coding_agent_panes(),
            Snapshot::default()
        );
    }
}
