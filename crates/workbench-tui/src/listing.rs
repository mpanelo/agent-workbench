//! One ordered list for registrations and optional, unregistered agent panes.
use workbench_core::{Pane, WorkItemState};

use crate::{AppState, interaction::Interaction};

pub(crate) enum Row<'a> {
    Work(&'a WorkItemState),
    Session {
        session: String,
        window: String,
        pane: Pane,
    },
}

impl Row<'_> {
    pub fn selected(&self, interaction: &Interaction) -> bool {
        match self {
            Self::Work(entry) => {
                interaction.selected_pane.is_none()
                    && interaction.selected_id.as_ref() == Some(&entry.item.id)
            }
            Self::Session { pane, .. } => interaction.selected_pane.as_ref() == Some(&pane.id),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Work(entry) => &entry.item.id,
            Self::Session { window, .. } => window,
        }
    }

    fn select(&self, interaction: &mut Interaction) {
        match self {
            Self::Work(entry) => {
                interaction.selected_id = Some(entry.item.id.clone());
                interaction.selected_pane = None;
            }
            Self::Session { pane, .. } => {
                interaction.selected_id = None;
                interaction.selected_pane = Some(pane.id.clone());
            }
        }
        interaction.reveal_selection = true;
    }
}

impl AppState {
    pub fn rows(&self, show_sessions: bool) -> Vec<Row<'_>> {
        let mut rows: Vec<_> = self.items().iter().map(Row::Work).collect();
        // Without a loaded registry, we cannot establish which panes are unregistered.
        if show_sessions
            && matches!(self.work_items, Some(Ok(_)))
            && let Some(snapshot) = self.snapshot()
        {
            let mut seen: std::collections::HashSet<_> = self
                .items()
                .iter()
                .map(|entry| entry.item.pane_id.clone())
                .collect();
            for session in snapshot.coding_agent_panes().sessions {
                for window in session.windows {
                    for pane in window.panes {
                        if seen.insert(pane.id.clone()) {
                            rows.push(Row::Session {
                                session: session.name.clone(),
                                window: window.name.clone(),
                                pane,
                            });
                        }
                    }
                }
            }
        }
        rows
    }

    pub fn move_row(&self, interaction: &mut Interaction, delta: isize) {
        if !matches!(self.work_items, Some(Ok(_))) {
            return;
        }
        if !interaction.show_sessions && interaction.selected_pane.is_none() {
            interaction.move_selection(self.items(), delta);
            return;
        }
        self.sync_selection(interaction);
        let rows = self.rows(interaction.show_sessions);
        if rows.is_empty() {
            return;
        }
        let index = rows
            .iter()
            .position(|row| row.selected(interaction))
            .unwrap_or(0);
        rows[index.saturating_add_signed(delta).min(rows.len() - 1)].select(interaction);
    }

    pub fn sync_selection(&self, interaction: &mut Interaction) {
        // Failed loads do not prove removal. Suspend actions through items(),
        // but keep identity until a successful registry/discovery refresh.
        if !matches!(self.work_items, Some(Ok(_))) {
            return;
        }
        if interaction.selected_pane.is_none() {
            interaction.sync(self.items());
        } else if let Some(bar) = &mut interaction.response_bar {
            bar.observe(self.items());
        }
        if let Some(pane) = &interaction.selected_pane
            && let Some(entry) = self
                .items()
                .iter()
                .find(|entry| entry.item.pane_id == *pane)
        {
            Row::Work(entry).select(interaction);
            return;
        }
        let rows = self.rows(interaction.show_sessions);
        if rows.iter().any(|row| row.selected(interaction)) {
            return;
        }
        if interaction.show_sessions
            && interaction.selected_pane.is_some()
            && self.snapshot().is_none()
        {
            return;
        }
        interaction.selected_id = None;
        interaction.selected_pane = None;
        if let Some(row) = rows.first() {
            row.select(interaction);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{navigate, ui};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{
        AgentStatus, PaneAvailability, Session, Snapshot, Window, WorkItem, WorkItemKind,
        WorkspaceAvailability,
    };

    fn fixture() -> AppState {
        let panes = [
            ("%1", "fish"),
            ("%2", "codex"),
            ("%3", "claude"),
            ("%4", "nvim"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (id, command))| Pane {
            id: id.into(),
            index: index as u32,
            title: format!("Task {index}"),
            current_command: Some(command.into()),
            working_directory: Some("/work/repo".into()),
        })
        .collect();
        let snapshot = Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@1".into(),
                    index: 0,
                    name: "feature".into(),
                    panes,
                }],
            }],
        };
        let items = [("B", "%1"), ("A", "%gone")]
            .into_iter()
            .map(|(id, pane)| WorkItemState {
                item: WorkItem {
                    id: id.into(),
                    title: String::new(),
                    repository: "/work/repo".into(),
                    workspace: "/work/repo".into(),
                    branch: Some("main".into()),
                    kind: WorkItemKind::Implementation,
                    pane_id: pane.into(),
                },
                status: AgentStatus::Unknown,
                status_detail: String::new(),
                attention_prompt: None,
                pane: PaneAvailability::Present,
                workspace_availability: WorkspaceAvailability::Present,
                completion_fingerprint: None,
            })
            .collect();
        AppState::from_refresh(Ok(snapshot), Ok(items))
    }

    fn key(code: KeyCode, state: &AppState, interaction: &mut Interaction) -> Option<String> {
        navigate(code, state, &mut 0, interaction)
    }

    #[test]
    fn test_rows() {
        let mut state = fixture();
        assert!(!Interaction::default().show_sessions);
        assert_eq!(
            state.rows(false).iter().map(Row::label).collect::<Vec<_>>(),
            ["B", "A"]
        );
        let snapshot = state.snapshot().unwrap().clone();
        // Linked sessions may expose the same pane more than once.
        state
            .discovery
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .sessions
            .push(snapshot.sessions[0].clone());
        let rows = state.rows(true);
        assert_eq!(rows.len(), 4);
        let ids: Vec<_> = rows
            .iter()
            .map(|row| match row {
                Row::Work(entry) => entry.item.id.as_str(),
                Row::Session { pane, .. } => pane.id.as_str(),
            })
            .collect();
        assert_eq!(ids, ["B", "A", "%2", "%3"]);
        assert_eq!(state.snapshot().unwrap().sessions.len(), 2);
        if let Some(Ok(items)) = &mut state.work_items {
            items[0].item.pane_id = "%2".into();
        }
        assert_eq!(state.rows(true).len(), 3);
    }

    #[test]
    fn test_toggle() {
        let state = fixture();
        let mut interaction = Interaction::default();
        state.sync_selection(&mut interaction);
        key(KeyCode::Down, &state, &mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        key(KeyCode::Tab, &state, &mut interaction);
        assert!(interaction.show_sessions);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        key(KeyCode::Down, &state, &mut interaction);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        assert!(interaction.selected_id.is_none());
        assert!(interaction.selected(state.items()).is_none());
        key(KeyCode::Tab, &state, &mut interaction);
        assert!(!interaction.show_sessions);
        assert!(interaction.selected_pane.is_none());
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        assert_eq!(
            key(KeyCode::Enter, &state, &mut interaction).as_deref(),
            Some("B")
        );
    }

    #[test]
    fn test_session_actions() {
        let state = fixture();
        let mut interaction = Interaction {
            show_sessions: true,
            selected_pane: Some("%2".into()),
            // Even an inconsistent selection cannot route actions to hidden work.
            selected_id: Some("A".into()),
            ..Interaction::default()
        };
        for code in ['e', 'd', 'c', 'x', 'y', 'n'] {
            assert!(key(KeyCode::Char(code), &state, &mut interaction).is_none());
            assert!(interaction.maintenance_requested.is_none());
            assert!(interaction.removal_requested.is_none());
            assert!(interaction.review_requested.is_none());
            assert!(interaction.approval_requested.is_none());
            assert!(interaction.acknowledgement_requested.is_none());
            assert!(interaction.draft.is_none());
        }
        for code in [KeyCode::Enter, KeyCode::Char('r')] {
            assert!(key(code, &state, &mut interaction).is_none());
            assert_eq!(
                interaction.registration_requested.take().as_deref(),
                Some("%2")
            );
        }
        let hints = interaction.help_context().hints();
        assert!(hints.contains("Register: <enter>/r"));
        for hidden in ["Respond:", "Edit:", "Review:", "Clean Up:", "Acknowledge"] {
            assert!(!hints.contains(hidden));
        }
    }

    #[test]
    fn test_work_actions() {
        let state = fixture();
        let mut interaction = Interaction {
            show_sessions: true,
            selected_id: Some("A".into()),
            ..Interaction::default()
        };
        key(KeyCode::Char('d'), &state, &mut interaction);
        assert_eq!(interaction.review_requested.as_deref(), Some("A"));
        key(KeyCode::Char('c'), &state, &mut interaction);
        assert_eq!(interaction.removal_requested.as_ref().unwrap().id, "A");
        key(KeyCode::Char('e'), &state, &mut interaction);
        assert!(matches!(&interaction.maintenance_requested,
            Some(crate::maintenance::Request::Edit(item)) if item.id == "A"));
        key(KeyCode::Char('r'), &state, &mut interaction);
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
        assert!(interaction.registration_requested.is_none());
    }

    #[test]
    fn test_refresh() {
        let mut state = fixture();
        let mut interaction = Interaction {
            show_sessions: true,
            selected_pane: Some("%3".into()),
            ..Interaction::default()
        };
        state.sync_selection(&mut interaction);
        if let Some(Ok(snapshot)) = &mut state.discovery {
            snapshot.sessions[0].windows[0].panes.reverse();
        }
        state.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%3"));
        // The selected pane becoming registered moves selection to its work identity.
        if let Some(Ok(items)) = &mut state.work_items {
            items[1].item.pane_id = "%3".into();
        }
        state.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        assert!(interaction.selected_pane.is_none());
        assert_eq!(state.rows(true).len(), 3);
        if let Some(Ok(items)) = &mut state.work_items {
            items[1].status = AgentStatus::WaitingForInput;
            items[1].attention_prompt = Some("Question?".into());
        }
        state.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
    }

    #[test]
    fn test_disappearance() {
        let mut state = fixture();
        let mut interaction = Interaction {
            show_sessions: true,
            selected_pane: Some("%2".into()),
            ..Interaction::default()
        };
        state.discovery = Some(Err("offline".into()));
        state.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        key(KeyCode::Enter, &state, &mut interaction);
        assert!(interaction.registration_requested.is_none());
        assert!(
            interaction
                .message
                .as_deref()
                .unwrap()
                .contains("unavailable")
        );
        state.discovery = Some(Ok(Snapshot::default()));
        state.sync_selection(&mut interaction);
        assert!(interaction.selected_pane.is_none());
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        state.work_items = Some(Err("broken registry".into()));
        state.move_row(&mut interaction, 1);
        state.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        assert!(state.rows(true).is_empty());
    }

    #[test]
    fn test_keys() {
        let state = fixture();
        let mut interaction = Interaction::default();
        state.sync_selection(&mut interaction);
        for code in ['s', 'w', 'f'] {
            key(KeyCode::Char(code), &state, &mut interaction);
        }
        assert!(!interaction.show_sessions);
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT,
        ] {
            navigate(
                KeyEvent::new(KeyCode::Tab, modifiers),
                &state,
                &mut 0,
                &mut interaction,
            );
            assert!(!interaction.show_sessions);
        }
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            navigate(
                KeyEvent {
                    kind,
                    ..KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
                },
                &state,
                &mut 0,
                &mut interaction,
            );
            assert!(!interaction.show_sessions);
        }
        interaction.begin_reply(state.items());
        key(KeyCode::Tab, &state, &mut interaction);
        assert!(!interaction.show_sessions);
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "B");
    }

    #[test]
    fn test_empty() {
        let mut state = fixture();
        state.work_items = Some(Ok(Vec::new()));
        let mut interaction = Interaction::default();
        state.sync_selection(&mut interaction);
        assert!(interaction.selected_pane.is_none());
        key(KeyCode::Tab, &state, &mut interaction);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        key(KeyCode::Down, &state, &mut interaction);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%3"));
        key(KeyCode::Tab, &state, &mut interaction);
        assert!(interaction.selected_pane.is_none());
        assert!(interaction.selected_id.is_none());
    }

    #[test]
    fn test_reveal() {
        let mut state = fixture();
        if let Some(Ok(snapshot)) = &mut state.discovery {
            let window = &mut snapshot.sessions[0].windows[0];
            let mut pane = window.panes[1].clone();
            for index in 10..20 {
                pane.id = format!("%{index}");
                window.panes.push(pane.clone());
            }
        }
        let mut interaction = Interaction {
            show_sessions: true,
            selected_pane: Some("%19".into()),
            ..Interaction::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|frame| ui::render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        assert!(interaction.work_list_offset > 0);
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .chunks(40)
                .any(|row| row[0].symbol() == ">")
        );
        state.sync_selection(&mut interaction);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%19"));
        key(KeyCode::Tab, &state, &mut interaction);
        terminal
            .draw(|frame| ui::render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        assert_eq!(interaction.work_list_offset, 0);
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
    }

    #[test]
    fn test_unavailable() {
        for discovery in [None, Some(Err("tmux\x1b unavailable".into()))] {
            let state = AppState {
                discovery,
                work_items: Some(Ok(Vec::new())),
                ..AppState::default()
            };
            let mut interaction = Interaction {
                show_sessions: true,
                ..Interaction::default()
            };
            let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
            terminal
                .draw(|frame| ui::render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if state.discovery.is_none() {
                assert!(text.contains("Discovering sessions"));
            } else {
                assert!(text.contains("Discovery unavailable: tmux\\u{1b} unavailable"));
            }
            assert!(!text.chars().any(char::is_control));
        }
    }

    #[test]
    fn test_render() {
        let mut state = fixture();
        if let Some(Ok(snapshot)) = &mut state.discovery {
            snapshot.sessions[0].name = "main\x1b[2J".into();
            let window = &mut snapshot.sessions[0].windows[0];
            window.name = "界🙂feature".into();
            window.panes[1].title = "Question\nnext\tline\x1b".into();
        }
        let mut interaction = Interaction {
            show_sessions: true,
            selected_pane: Some("%2".into()),
            ..Interaction::default()
        };
        for (width, height) in [(140, 30), (40, 12), (20, 6), (1, 1), (0, 0)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| ui::render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
            let cells = terminal.backend().buffer().content();
            assert!(
                cells
                    .iter()
                    .all(|cell| !cell.symbol().chars().any(char::is_control))
            );
            let text: String = cells.iter().map(|cell| cell.symbol()).collect();
            if width == 140 {
                for expected in [
                    "UNREGISTERED",
                    "main\\u{1b}[2J",
                    "Question\\nnext\\tline\\u{1b}",
                    "Register: <enter>/r",
                ] {
                    assert!(text.contains(expected), "missing {expected}: {text}");
                }
                assert_eq!(text.matches("UNREGISTERED").count(), 2);
                let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
                assert!(compact.contains("界🙂feature"));
                assert!(!text.contains("Pane preview"));
                assert!(!text.contains("Respond: r"));
            }
            assert_eq!(interaction.selected_pane.as_deref(), Some("%2"));
        }
    }
}
