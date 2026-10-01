use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};

use crate::interaction::Interaction;
use crate::{AppState, DiscoveryState};

pub(crate) enum View {
    Work,
    Sessions,
}

pub(crate) fn render_work(
    frame: &mut Frame<'_>,
    state: &AppState,
    scroll: &mut u16,
    interaction: &mut Interaction,
) {
    let [header, body, notice, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(if interaction.message.is_some() { 2 } else { 0 }),
        Constraint::Length(if interaction.draft.is_some() { 3 } else { 0 }),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new("AGENT WORKBENCH — WORK").style(Style::default().fg(Color::Cyan)),
        header,
    );
    let mut lines = Vec::new();
    let mut selected_row = None;
    if let Some(Err(error)) = &state.discovery {
        lines.push(Line::styled(
            format!("Discovery unavailable: {}", visible(error)),
            Style::default().fg(Color::Red),
        ));
        lines.push(Line::from(
            "Saved items remain registered. Retrying automatically every 2 seconds.",
        ));
        lines.push(Line::from(""));
    }
    match &state.work_items {
        None => lines.push(Line::from("Loading work items…")),
        Some(Err(error)) => {
            lines.push(Line::styled(
                format!("Work-item state unavailable: {}", visible(error)),
                Style::default().fg(Color::Red),
            ));
            lines.push(Line::from(
                "The state file has not been changed. Retrying automatically.",
            ));
        }
        Some(Ok(items)) if items.is_empty() => {
            lines.push(Line::from("No work items registered."));
            lines.push(Line::from(
                "Register from another terminal with `workbench register` (see --help).",
            ));
        }
        Some(Ok(items)) => {
            for state in items {
                let item = &state.item;
                let selected = interaction.selected_id.as_ref() == Some(&item.id);
                if selected {
                    selected_row = Some(lines.len());
                }
                lines.push(Line::styled(
                    format!(
                        "{}{}  {}",
                        if selected { "> " } else { "  " },
                        visible(&item.id),
                        state.status
                    ),
                    Style::default().fg(if selected { Color::Yellow } else { Color::Cyan }),
                ));
                lines.push(Line::from(format!("  {}", visible(&item.title))));
                lines.push(Line::from(format!(
                    "  Type: {}  Pane: {} ({})",
                    item.kind,
                    visible(&item.pane_id),
                    state.pane
                )));
                lines.push(Line::from(format!(
                    "  Repository: {}",
                    visible(&item.repository.to_string_lossy())
                )));
                lines.push(Line::from(format!(
                    "  Workspace: {}",
                    visible(&item.workspace.to_string_lossy())
                )));
                if let Some(branch) = &item.branch {
                    lines.push(Line::from(format!("  Branch: {}", visible(branch))));
                }
                lines.push(Line::from(""));
            }
        }
    }
    let max_scroll = lines
        .len()
        .saturating_sub(body.height as usize)
        .min(u16::MAX as usize) as u16;
    if interaction.reveal_selection {
        if let Some(row) = selected_row {
            let row = row.min(u16::MAX as usize) as u16;
            if row < *scroll {
                *scroll = row;
            }
            if row >= scroll.saturating_add(body.height) {
                *scroll = row.saturating_sub(body.height.saturating_sub(1));
            }
        }
        interaction.reveal_selection = false;
    }
    *scroll = (*scroll).min(max_scroll);
    frame.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), body);
    if let Some(message) = &interaction.message {
        frame.render_widget(
            Paragraph::new(visible(message)).wrap(Wrap { trim: false }),
            notice,
        );
    }
    if let Some(draft) = &interaction.draft {
        let block = Block::bordered().title(format!("Reply to {}", visible(&draft.item_id)));
        let inner = block.inner(composer);
        frame.render_widget(block, composer);
        let width = Line::from(draft.text.as_str())
            .width()
            .min(u16::MAX as usize) as u16;
        let offset = width.saturating_sub(inner.width.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(draft.text.as_str()).scroll((0, offset)),
            inner,
        );
        if !interaction.sending && inner.width > 0 && inner.height > 0 {
            frame.set_cursor_position((inner.x + width.saturating_sub(offset), inner.y));
        }
    }
    frame.render_widget(
        Paragraph::new(if interaction.draft.is_some() {
            "Enter: send  Esc/Ctrl-C: cancel  Backspace: edit  Ctrl-u: clear"
        } else {
            "j/k ↑/↓: select  Enter: open  r: reply  Tab: attention  s: sessions  q: quit"
        }),
        footer,
    );
}

pub(crate) fn render(frame: &mut Frame<'_>, state: &DiscoveryState, scroll: &mut u16) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new("AGENT WORKBENCH — SESSIONS").style(Style::default().fg(Color::Cyan)),
        header,
    );
    let mut lines = Vec::<Line<'static>>::new();
    match state {
        None => lines.push(Line::from("Discovering sessions…")),
        Some(Err(error)) => {
            *scroll = 0;
            frame.render_widget(
                Paragraph::new(format!(
                    "Discovery unavailable\n\n{}\n\nRetrying automatically every 2 seconds…",
                    visible(error)
                ))
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: false }),
                body,
            );
        }
        Some(Ok(snapshot)) if snapshot.sessions.is_empty() => {
            lines.push(Line::from("No tmux sessions found."));
        }
        Some(Ok(snapshot)) => {
            for session in &snapshot.sessions {
                lines.push(Line::styled(
                    format!("{}  ({})", visible(&session.name), session.id),
                    Style::default().fg(Color::Cyan),
                ));
                for window in &session.windows {
                    lines.push(Line::from(format!(
                        "  {}  {}  ({})",
                        window.index,
                        visible(&window.name),
                        window.id
                    )));
                    for pane in &window.panes {
                        lines.push(Line::from(format!(
                            "    {}  pane {}  command: {}  title: {}",
                            pane.id,
                            pane.index,
                            visible(pane.current_command.as_deref().unwrap_or("unavailable")),
                            visible(&pane.title),
                        )));
                        lines.push(Line::styled(
                            format!(
                                "      cwd: {}",
                                pane.working_directory
                                    .as_ref()
                                    .map(|path| visible(&path.to_string_lossy()))
                                    .unwrap_or_else(|| "unavailable".to_owned())
                            ),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
                lines.push(Line::from(""));
            }
        }
    }
    if !matches!(state, Some(Err(_))) {
        let max_scroll = lines
            .len()
            .saturating_sub(body.height as usize)
            .min(u16::MAX as usize) as u16;
        *scroll = (*scroll).min(max_scroll);
        frame.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), body);
    }
    frame.render_widget(
        Paragraph::new(
            "w: work  s: sessions  ↑/↓ PgUp/PgDn: scroll  q/Esc/Ctrl-C: quit  • refresh: 2s",
        ),
        footer,
    );
}

// Make control characters visible without letting names/titles distort the layout.
fn visible(text: &str) -> String {
    text.chars()
        .flat_map(|ch| {
            if ch.is_control() {
                ch.escape_default().collect::<Vec<_>>()
            } else {
                vec![ch]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{
        AgentStatus, Pane, PaneAvailability, Session, Snapshot, Window, WorkItem, WorkItemKind,
        WorkItemState,
    };

    use super::*;

    fn screen(state: DiscoveryState, width: u16, height: u16, scroll: &mut u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, &state, scroll))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            sessions: vec![Session {
                id: "$1".into(),
                name: "main".into(),
                windows: vec![Window {
                    id: "@2".into(),
                    index: 2,
                    name: "auth".into(),
                    panes: vec![Pane {
                        id: "%14".into(),
                        index: 0,
                        title: "Agent".into(),
                        current_command: Some("codex".into()),
                        working_directory: Some("/work/my repo".into()),
                    }],
                }],
            }],
        }
    }

    fn work_screen(state: &AppState, width: u16, height: u16, scroll: &mut u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        terminal
            .draw(|frame| render_work(frame, state, scroll, &mut interaction))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn registered(pane: PaneAvailability, kind: WorkItemKind) -> WorkItemState {
        WorkItemState {
            item: WorkItem {
                id: "ABC-123".into(),
                title: "Fix retries".into(),
                repository: "/work/repo".into(),
                workspace: "/work/ABC-123".into(),
                branch: Some("fix/retries".into()),
                kind,
                pane_id: "%14".into(),
            },
            status: AgentStatus::Unknown,
            pane,
        }
    }

    #[test]
    fn work_view_shows_saved_metadata_and_unknown_status_for_both_types() {
        for kind in [WorkItemKind::Implementation, WorkItemKind::ExternalReview] {
            for pane in [
                PaneAvailability::Present,
                PaneAvailability::Missing,
                PaneAvailability::Unavailable,
            ] {
                let state = AppState {
                    discovery: Some(Ok(snapshot())),
                    work_items: Some(Ok(vec![registered(pane, kind)])),
                };
                let text = work_screen(&state, 120, 15, &mut 0);
                for expected in [
                    "WORK",
                    "ABC-123  UNKNOWN",
                    "Fix retries",
                    "%14",
                    "/work/repo",
                    "/work/ABC-123",
                    "Branch: fix/retries",
                    &kind.to_string(),
                    &format!("({pane})"),
                ] {
                    assert!(text.contains(expected), "missing {expected:?} in {text}");
                }
            }
        }
    }

    #[test]
    fn discovery_failure_keeps_work_items_visible_and_state_errors_are_actionable() {
        let state = AppState {
            discovery: Some(Err("tmux was not found".into())),
            work_items: Some(Ok(vec![registered(
                PaneAvailability::Unavailable,
                WorkItemKind::Implementation,
            )])),
        };
        let text = work_screen(&state, 120, 15, &mut 0);
        assert!(text.contains("Discovery unavailable"));
        assert!(text.contains("ABC-123  UNKNOWN"));
        assert!(text.contains("%14 (unavailable)"));
        let state = AppState {
            work_items: Some(Err("unsupported schema version 2".into())),
            ..AppState::default()
        };
        let text = work_screen(&state, 120, 15, &mut 0);
        assert!(text.contains("unsupported schema version 2"));
        assert!(text.contains("file has not been changed"));
    }

    #[test]
    fn work_view_handles_loading_empty_and_small_screens() {
        assert!(work_screen(&AppState::default(), 80, 10, &mut 0).contains("Loading work items"));
        let state = AppState {
            work_items: Some(Ok(Vec::new())),
            ..AppState::default()
        };
        let mut scroll = 100;
        let text = work_screen(&state, 100, 10, &mut scroll);
        assert!(text.contains("No work items registered"));
        assert!(text.contains("workbench register"));
        assert_eq!(scroll, 0);
        work_screen(&state, 1, 1, &mut scroll);
    }

    #[test]
    fn selecting_a_later_item_keeps_its_highlight_visible_in_a_small_viewport() {
        let mut items = Vec::new();
        for id in ["A", "B", "C"] {
            let mut state = registered(PaneAvailability::Present, WorkItemKind::Implementation);
            state.item.id = id.into();
            items.push(state);
        }
        let state = AppState {
            work_items: Some(Ok(items)),
            ..AppState::default()
        };
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        interaction.move_selection(state.items(), 2);
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        let mut scroll = 0;
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("> C  UNKNOWN"));
        assert!(scroll > 0);
    }

    #[test]
    fn reply_editor_renders_captured_target_and_unicode_on_small_screens() {
        let state = AppState {
            work_items: Some(Ok(vec![registered(
                PaneAvailability::Present,
                WorkItemKind::Implementation,
            )])),
            ..AppState::default()
        };
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        interaction.begin_reply(state.items());
        interaction
            .draft
            .as_mut()
            .unwrap()
            .append("yes λ🙂")
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 15)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Reply to ABC-123"));
        assert!(text.contains("yes λ🙂"));
        assert!(text.contains("Enter: send"));
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
    }

    #[test]
    fn displays_hierarchy_and_all_pane_metadata() {
        let text = screen(Some(Ok(snapshot())), 100, 12, &mut 0);
        for expected in [
            "SESSIONS",
            "main  ($1)",
            "2  auth  (@2)",
            "%14",
            "pane 0",
            "command: codex",
            "title: Agent",
            "cwd: /work/my repo",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
    }

    #[test]
    fn displays_loading_empty_and_error_states() {
        assert!(screen(None, 80, 10, &mut 0).contains("Discovering sessions"));
        assert!(
            screen(Some(Ok(Snapshot::default())), 80, 10, &mut 0)
                .contains("No tmux sessions found")
        );
        let mut scroll = 20;
        let text = screen(
            Some(Err("tmux was not found. Install tmux on PATH.".into())),
            80,
            10,
            &mut scroll,
        );
        assert!(text.contains("tmux was not found"));
        assert!(text.contains("Retrying automatically"));
        assert_eq!(scroll, 0);
    }

    #[test]
    fn displays_optional_metadata_as_unavailable_and_escapes_controls() {
        let mut snapshot = snapshot();
        let pane = &mut snapshot.sessions[0].windows[0].panes[0];
        pane.current_command = None;
        pane.working_directory = None;
        pane.title = "one\ntwo\tthree\x1b".into();
        let text = screen(Some(Ok(snapshot)), 100, 12, &mut 0);
        assert!(text.contains("command: unavailable"));
        assert!(text.contains("cwd: unavailable"));
        assert!(text.contains("one\\ntwo\\tthree\\u{1b}"));
    }

    #[test]
    fn clamps_scroll_when_snapshot_shrinks_and_handles_small_terminals() {
        let mut scroll = u16::MAX;
        let text = screen(Some(Ok(snapshot())), 100, 4, &mut scroll);
        assert_eq!(scroll, 3);
        assert!(text.contains("cwd: /work/my repo"));
        screen(Some(Ok(Snapshot::default())), 10, 2, &mut scroll);
        assert_eq!(scroll, 1);
        screen(Some(Ok(snapshot())), 1, 1, &mut scroll);
    }
}
