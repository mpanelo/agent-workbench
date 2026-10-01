use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Paragraph, Wrap},
};

use crate::DiscoveryState;

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
        Paragraph::new("↑/↓ PgUp/PgDn: scroll  Home: top  q/Esc/Ctrl-C: quit  • refresh: 2s"),
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
    use workbench_core::{Pane, Session, Snapshot, Window};

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
