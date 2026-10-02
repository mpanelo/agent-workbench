use std::path::{Path, PathBuf};

use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use crate::AppState;
use crate::interaction::Interaction;
use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
    #[default]
    Attention,
    Work,
    Sessions,
}

impl View {
    pub fn is_item_view(self) -> bool {
        self != Self::Sessions
    }
}

pub(crate) fn render_work(
    frame: &mut Frame<'_>,
    state: &AppState,
    scroll: &mut u16,
    interaction: &mut Interaction,
) {
    render_items(frame, state, scroll, interaction, View::Work);
}

pub(crate) fn render_attention(
    frame: &mut Frame<'_>,
    state: &AppState,
    scroll: &mut u16,
    interaction: &mut Interaction,
) {
    render_items(frame, state, scroll, interaction, View::Attention);
}

fn render_items(
    frame: &mut Frame<'_>,
    state: &AppState,
    scroll: &mut u16,
    interaction: &mut Interaction,
    view: View,
) {
    theme::paint(frame);
    let [header, body, notice, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(if interaction.message.is_some() { 2 } else { 0 }),
        Constraint::Length(if interaction.draft.is_some() { 3 } else { 0 }),
        Constraint::Length(if interaction.draft.is_none() { 2 } else { 1 }),
    ])
    .areas(frame.area());
    let count = match &state.work_items {
        None => "…".into(),
        Some(Err(_)) => "unavailable".into(),
        Some(Ok(_)) => state.attention.len().to_string(),
    };
    let title = if view == View::Attention {
        format!("AGENT WORKBENCH — ATTENTION — {count}")
    } else {
        format!("AGENT WORKBENCH — WORK • attention: {count} (a)")
    };
    frame.render_widget(Paragraph::new(title).style(theme::header()), header);
    let mut lines = Vec::new();
    let mut selected_row = None;
    if let Some(Err(error)) = &state.discovery {
        lines.push(Line::styled(
            format!("Discovery unavailable: {}", visible(error)),
            theme::error(),
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
                theme::error(),
            ));
            lines.push(Line::from(
                "The state file has not been changed. Retrying automatically.",
            ));
        }
        Some(Ok(items)) if items.is_empty() => {
            lines.push(Line::from("No work items registered."));
            lines.push(Line::from(
                "Press s, select a pane with j/k, then r to register it here.",
            ));
        }
        Some(Ok(items)) => {
            if view == View::Attention {
                let unknown = items
                    .iter()
                    .filter(|item| item.status == workbench_core::AgentStatus::Unknown)
                    .count();
                if state.attention.is_empty() {
                    lines.push(Line::from("No known items need attention."));
                }
                if unknown > 0 {
                    lines.push(Line::from(format!("{unknown} UNKNOWN item(s) are unclassified, not assumed idle. Press w to inspect all items.")));
                }
                if state.attention.is_empty() || unknown > 0 {
                    lines.push(Line::from(""));
                }
            }
            for state in state.items_for(view) {
                let item = &state.item;
                let selected = interaction.selected_id.as_ref() == Some(&item.id);
                if selected {
                    selected_row = Some(lines.len());
                }
                let mut heading = vec![Span::styled(
                    format!(
                        "{}{}  ",
                        if selected { "> " } else { "  " },
                        visible(&item.id),
                    ),
                    theme::accent(),
                )];
                if view == View::Attention {
                    heading.push(Span::styled(
                        format!("{}  ", work_kind_label(item.kind)),
                        theme::work_kind(item.kind),
                    ));
                }
                heading.push(Span::styled(
                    state.status.to_string(),
                    theme::status(state.status),
                ));
                lines.push(selectable_line(Line::from(heading), selected, body.width));
                if view == View::Work {
                    lines.push(Line::styled(
                        format!("  {}", visible(&item.title)),
                        theme::text().add_modifier(ratatui::style::Modifier::BOLD),
                    ));
                    lines.push(Line::styled(
                        format!("  Status: {}", visible(&state.status_detail)),
                        theme::muted(),
                    ));
                }
                if (view == View::Attention || selected)
                    && state.status == workbench_core::AgentStatus::WaitingForInput
                {
                    lines.push(Line::styled("  Agent requests input:", theme::notice()));
                    if let Some(prompt) = &state.attention_prompt {
                        push_prompt(&mut lines, prompt, body.width);
                    } else {
                        lines.push(Line::from(format!("  {}", visible(&state.status_detail))));
                    }
                }
                if view == View::Work {
                    lines.push(metadata(
                        "Type",
                        work_kind_label(item.kind),
                        theme::work_kind(item.kind),
                    ));
                    let warning = match state.pane {
                        workbench_core::PaneAvailability::Present => None,
                        workbench_core::PaneAvailability::Missing => Some("  Agent pane missing."),
                        workbench_core::PaneAvailability::Unavailable => {
                            Some("  Agent pane unavailable.")
                        }
                    };
                    if let Some(warning) = warning {
                        lines.push(Line::styled(warning, theme::notice()));
                    }
                }
                if view == View::Work {
                    lines.push(metadata(
                        "Repository",
                        &display_path(&item.repository),
                        theme::path(),
                    ));
                }
                if view == View::Work {
                    lines.push(metadata(
                        "Workspace",
                        &display_path(&item.workspace),
                        theme::path(),
                    ));
                }
                if view == View::Work
                    && let Some(branch) = &item.branch
                {
                    lines.push(metadata("Branch", branch, Style::default().fg(theme::PINK)));
                }
                if view == View::Work
                    || state.status == workbench_core::AgentStatus::WaitingForInput
                {
                    lines.push(Line::from(""));
                }
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
            Paragraph::new(visible(message))
                .style(theme::notice())
                .wrap(Wrap { trim: false }),
            notice,
        );
    }
    if let Some(draft) = &interaction.draft {
        let block = Block::bordered()
            .title(format!("Reply to {}", visible(&draft.item_id)))
            .style(theme::panel())
            .border_style(theme::border(true))
            .title_style(theme::accent());
        let inner = block.inner(composer);
        frame.render_widget(block, composer);
        let width = Line::from(draft.text.as_str())
            .width()
            .min(u16::MAX as usize) as u16;
        let offset = width.saturating_sub(inner.width.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(draft.text.as_str())
                .style(theme::panel())
                .scroll((0, offset)),
            inner,
        );
        if !interaction.sending && inner.width > 0 && inner.height > 0 {
            frame.set_cursor_position((inner.x + width.saturating_sub(offset), inner.y));
        }
    }
    frame.render_widget(
        theme::footer(if interaction.draft.is_some() {
            "Enter: send | Esc/Ctrl-C: cancel | Backspace: edit | Ctrl-u: clear"
        } else if view == View::Work {
            "e: edit description | n: rename ID | u: unregister\nj/k | Enter: open | r: reply | d: review | Ctrl+d/u: scroll | a/w/s | q: quit"
        } else {
            "x: acknowledge finished turn\nj/k | Enter: open | r: reply | d: review | Ctrl+d/u: scroll | a/w/s | q: quit"
        }),
        footer,
    );
}

/// Compact card labels only; registration, CLI flags, and persisted kinds keep
/// their full domain names.
fn work_kind_label(kind: workbench_core::WorkItemKind) -> &'static str {
    match kind {
        workbench_core::WorkItemKind::Implementation => "Build",
        workbench_core::WorkItemKind::ExternalReview => "Review",
    }
}

fn metadata(label: &str, value: &str, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label}: "), theme::label()),
        Span::styled(visible(value), style),
    ])
}

// Paint the full selected row, not just its glyphs. Span foregrounds retain
// their status/type colors over the shared selection background.
fn selectable_line(mut line: Line<'static>, selected: bool, width: u16) -> Line<'static> {
    if selected {
        line.spans.push(Span::raw(
            " ".repeat(usize::from(width).saturating_sub(line.width())),
        ));
        line.style(theme::selection().fg(theme::TEXT))
    } else {
        line.style(theme::text())
    }
}

// Wrap bounded prompt previews by display width while retaining exact rendered
// row counts for selection/scrolling. Escaping happens before width measurement.
fn push_prompt(lines: &mut Vec<Line<'static>>, text: &str, width: u16) {
    let limit = usize::from(width).max(3);
    for line in text.lines() {
        let style = theme::prompt_line(line);
        let mut row = String::from("  ");
        let mut columns = 2;
        for ch in visible(line).chars() {
            let ch_width = Line::from(ch.to_string()).width();
            if columns + ch_width > limit && columns > 2 {
                lines.push(Line::styled(row, style));
                row = String::from("  ");
                columns = 2;
            }
            row.push(ch);
            columns += ch_width;
        }
        lines.push(Line::styled(row, style));
    }
}

pub(crate) fn render_sessions(
    frame: &mut Frame<'_>,
    state: &AppState,
    scroll: &mut u16,
    interaction: &mut Interaction,
) {
    theme::paint(frame);
    let [header, body, notice, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(if interaction.message.is_some() { 2 } else { 0 }),
        Constraint::Length(2),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(if interaction.show_all_panes {
            "AGENT WORKBENCH — SESSIONS (all panes)"
        } else {
            "AGENT WORKBENCH — SESSIONS (agents only)"
        })
        .style(theme::header()),
        header,
    );
    let mut lines = Vec::<Line<'static>>::new();
    let mut selected_row = None;
    match &state.discovery {
        None => lines.push(Line::from("Discovering sessions…")),
        Some(Err(error)) => {
            *scroll = 0;
            frame.render_widget(
                Paragraph::new(format!(
                    "Discovery unavailable\n\n{}\n\nRetrying automatically every 2 seconds…",
                    visible(error)
                ))
                .style(theme::error())
                .wrap(Wrap { trim: false }),
                body,
            );
        }
        Some(Ok(snapshot)) if snapshot.sessions.is_empty() => {
            lines.push(Line::from("No tmux sessions found."));
        }
        Some(Ok(snapshot)) => {
            let snapshot = interaction.sessions_snapshot(snapshot);
            if snapshot.sessions.is_empty() {
                lines.push(Line::from("No recognized coding-agent panes found."));
                lines.push(Line::from(
                    "Press f to show all panes, including wrappers and unknown agents.",
                ));
            }
            for session in &snapshot.sessions {
                lines.push(Line::styled(
                    format!(" {}", visible(&session.name)),
                    theme::accent().fg(theme::PINK),
                ));
                for window in &session.windows {
                    lines.push(Line::styled(
                        format!("   {}", visible(window_label(&window.name))),
                        theme::accent().fg(theme::BLUE),
                    ));
                    for pane in &window.panes {
                        let selected =
                            interaction.selected_pane.as_deref() == Some(pane.id.as_str());
                        if selected && selected_row.is_none() {
                            selected_row = Some(lines.len());
                        }
                        let registered = state
                            .items()
                            .iter()
                            .filter(|item| item.item.pane_id == pane.id)
                            .map(|item| visible(&item.item.id))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let command = pane.current_command.as_deref().unwrap_or("unavailable");
                        let mut row = vec![
                            Span::raw(if selected { "  > " } else { "    " }),
                            Span::styled(" ", ratatui::style::Style::default().fg(theme::TEAL)),
                            Span::raw(visible(command)),
                        ];
                        if !registered.is_empty() {
                            row.push(Span::styled(
                                format!("  [Registered: {registered}]"),
                                ratatui::style::Style::default().fg(theme::GREEN),
                            ));
                        }
                        let title =
                            session_pane_title(&pane.title, command, &session.name, &window.name);
                        if !title.is_empty() {
                            row.push(Span::raw(format!(" — {}", visible(title))));
                        }
                        lines.push(selectable_line(Line::from(row), selected, body.width));
                        lines.push(Line::styled(
                            format!(
                                "      {}",
                                pane.working_directory
                                    .as_ref()
                                    .map(|path| visible(&display_path(path)))
                                    .unwrap_or_else(|| "Directory unavailable".to_owned())
                            ),
                            theme::path(),
                        ));
                    }
                }
                lines.push(Line::from(""));
            }
        }
    }
    if !matches!(state.discovery, Some(Err(_))) {
        let max_scroll = lines
            .len()
            .saturating_sub(body.height as usize)
            .min(u16::MAX as usize) as u16;
        if interaction.reveal_pane {
            if let Some(row) = selected_row {
                let row = row.min(u16::MAX as usize) as u16;
                if row < *scroll {
                    *scroll = row;
                }
                let end = row.saturating_add(1);
                if end >= scroll.saturating_add(body.height) {
                    *scroll = end.saturating_sub(body.height.saturating_sub(1));
                    // With a one-row viewport, keep the selectable row visible.
                    *scroll = (*scroll).min(row);
                }
            }
            interaction.reveal_pane = false;
        }
        *scroll = (*scroll).min(max_scroll);
        frame.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), body);
    }
    if let Some(message) = &interaction.message {
        frame.render_widget(
            Paragraph::new(visible(message))
                .style(theme::notice())
                .wrap(Wrap { trim: false }),
            notice,
        );
    }
    frame.render_widget(
        theme::footer(if interaction.show_all_panes {
            "f: show agents only\nj/k: panes | Enter/r: register | Ctrl+d/u: scroll | a/w/s | q: quit"
        } else {
            "f: show all panes\nj/k: panes | Enter/r: register | Ctrl+d/u: scroll | a/w/s | q: quit"
        }),
        footer,
    );
}

/// Replace a pre-existing branch glyph with our window marker, display-only.
fn window_label(name: &str) -> &str {
    name.trim().strip_prefix(" ").unwrap_or(name.trim()).trim()
}

/// Suppress only exact ancestor names and delimited trailing context. Do not
/// substring-replace names inside meaningful descriptions or merge real panes.
fn session_pane_title<'a>(title: &'a str, command: &str, session: &str, window: &str) -> &'a str {
    let context = [session.trim(), window.trim(), window_label(window)];
    let mut title = title.trim();
    while let Some((description, suffix)) = title.rsplit_once(" | ") {
        if !context.contains(&suffix.trim()) {
            break;
        }
        title = description.trim();
    }
    if title == command || context.contains(&title) {
        ""
    } else {
        title
    }
}

/// Abbreviate metadata paths for display only; stored paths remain absolute.
pub(crate) fn display_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    abbreviate_home(path, home.as_deref())
}

fn abbreviate_home(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && home.is_absolute()
        && home.parent().is_some()
        && let Ok(relative) = path.strip_prefix(home)
    {
        if relative.as_os_str().is_empty() {
            return "~".into();
        }
        return Path::new("~").join(relative).to_string_lossy().into_owned();
    }
    path.to_string_lossy().into_owned()
}

// Make control characters visible without letting names/titles distort the layout.
pub(crate) fn visible(text: &str) -> String {
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
    use crate::DiscoveryState;
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{
        AgentStatus, Pane, PaneAvailability, Session, Snapshot, Window, WorkItem, WorkItemKind,
        WorkItemState,
    };

    use super::*;

    fn screen(state: DiscoveryState, width: u16, height: u16, scroll: &mut u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let state = AppState {
            discovery: state,
            ..AppState::default()
        };
        let mut interaction = Interaction {
            show_all_panes: true,
            ..Interaction::default()
        };
        if let Some(snapshot) = state.snapshot() {
            interaction.sync_panes(snapshot);
        }
        // This helper tests explicit scroll clamping, not selection navigation.
        interaction.reveal_pane = false;
        terminal
            .draw(|frame| render_sessions(frame, &state, scroll, &mut interaction))
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
    fn sessions_defaults_to_agents_and_all_mode_recovers_hidden_panes() {
        let mut snapshot = snapshot();
        let mut shell = snapshot.sessions[0].windows[0].panes[0].clone();
        shell.id = "%15".into();
        shell.title = "Shell named codex".into();
        shell.current_command = Some("fish".into());
        let mut shell_window = snapshot.sessions[0].windows[0].clone();
        shell_window.name = "hidden-shell-window".into();
        shell_window.panes = vec![shell.clone()];
        snapshot.sessions[0].windows.push(shell_window.clone());
        snapshot.sessions.push(Session {
            id: "$2".into(),
            name: "hidden-shell-session".into(),
            windows: vec![shell_window],
        });
        snapshot.sessions[0].windows[0].panes.push(shell);
        let mut state = AppState::from_refresh(Ok(snapshot.clone()), Ok(vec![]));
        let mut interaction = Interaction::default();
        let draw = |state: &AppState, interaction: &mut Interaction| {
            if let Some(snapshot) = state.snapshot() {
                interaction.sync_panes(snapshot);
            }
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            terminal
                .draw(|frame| render_sessions(frame, state, &mut 0, interaction))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        let text = draw(&state, &mut interaction);
        assert!(text.contains("SESSIONS (agents only)"));
        assert!(text.contains("codex — Agent"));
        assert!(text.contains("f: show all panes"));
        for hidden in ["fish", "hidden-shell-window", "hidden-shell-session"] {
            assert!(!text.contains(hidden), "{text}");
        }
        interaction.show_all_panes = true;
        let text = draw(&state, &mut interaction);
        for expected in [
            "SESSIONS (all panes)",
            "fish — Shell named codex",
            "hidden-shell-window",
            "hidden-shell-session",
            "f: show agents only",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        interaction.show_all_panes = false;
        snapshot.sessions[0].windows[0].panes.remove(0);
        state.discovery = Some(Ok(snapshot));
        let text = draw(&state, &mut interaction);
        assert!(text.contains("No recognized coding-agent panes found."));
        assert!(text.contains("Press f to show all panes"));
        assert!(interaction.selected_pane.is_none());
    }

    #[test]
    fn sessions_remove_repeated_ancestor_context_but_keep_meaningful_titles() {
        let window = " nvim-v0-12";
        for (title, expected) in [
            (
                "Assess Neovim migration effort | nvim-v0-12",
                "Assess Neovim migration effort",
            ),
            (
                "Assess Neovim migration effort |  nvim-v0-12",
                "Assess Neovim migration effort",
            ),
            (
                "Assess Neovim migration effort | dotfiles | nvim-v0-12",
                "Assess Neovim migration effort",
            ),
            (" nvim-v0-12 ", ""),
            (" nvim-v0-12", ""),
            ("dotfiles", ""),
            ("codex", ""),
            ("", ""),
            (
                "Discuss nvim-v0-12 migration",
                "Discuss nvim-v0-12 migration",
            ),
            ("Task | nvim-v0-12-extra", "Task | nvim-v0-12-extra"),
            ("Task | unrelated", "Task | unrelated"),
            (
                "Task | nvim-v0-12 | unrelated",
                "Task | nvim-v0-12 | unrelated",
            ),
        ] {
            assert_eq!(
                session_pane_title(title, "codex", "dotfiles", window),
                expected,
                "{title}"
            );
        }
        let mut snapshot = snapshot();
        snapshot.sessions[0].name = "dotfiles".into();
        snapshot.sessions[0].windows[0].name = window.into();
        snapshot.sessions[0].windows[0].panes[0].title =
            "Assess Neovim migration effort | nvim-v0-12".into();
        let original = snapshot.clone();
        let text = screen(Some(Ok(snapshot.clone())), 100, 12, &mut 0);
        assert_eq!(text.matches("nvim-v0-12").count(), 1, "{text}");
        assert!(text.contains(" dotfiles"));
        assert!(text.contains(" nvim-v0-12"));
        assert!(text.contains(">  codex — Assess Neovim migration effort"));
        assert!(!text.contains(""));
        assert_eq!(snapshot, original);
    }

    #[test]
    fn compact_sessions_omit_repeated_titles_and_keep_duplicate_panes_selectable() {
        let mut snapshot = snapshot();
        let pane = &mut snapshot.sessions[0].windows[0].panes[0];
        pane.title = "codex".into();
        let mut second = pane.clone();
        second.id = "%15".into();
        second.title.clear();
        snapshot.sessions[0].windows[0].panes.push(second);
        let text = screen(Some(Ok(snapshot.clone())), 80, 16, &mut 0);
        assert_eq!(text.matches("codex").count(), 2);
        assert!(!text.contains("codex —"));
        assert!(!text.contains("%14"));
        assert!(!text.contains("%15"));

        let state = AppState::from_refresh(Ok(snapshot.clone()), Ok(vec![]));
        let mut interaction = Interaction::default();
        interaction.sync_panes(&snapshot);
        interaction.move_pane_selection(&snapshot, 1);
        assert_eq!(interaction.selected_pane.as_deref(), Some("%15"));
        for (width, height) in [(40, 4), (1, 1), (0, 1)] {
            interaction.reveal_pane = true;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_sessions(frame, &state, &mut 0, &mut interaction))
                .unwrap();
            if width == 40 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains(">  codex"), "{text}");
            }
            assert_eq!(interaction.selected_pane.as_deref(), Some("%15"));
        }
    }

    #[test]
    fn sessions_show_registration_and_keep_selected_panes_visible() {
        let mut snapshot = snapshot();
        let mut later = snapshot.sessions[0].windows[0].panes[0].clone();
        later.id = "%15".into();
        later.title = "Later agent".into();
        for index in 0..8 {
            let mut middle = later.clone();
            middle.id = format!("%{}", index + 100);
            middle.title = format!("Middle agent {index}");
            snapshot.sessions[0].windows[0].panes.push(middle);
        }
        snapshot.sessions[0].windows[0].panes.push(later);
        let state = AppState::from_refresh(
            Ok(snapshot),
            Ok(vec![registered(
                PaneAvailability::Present,
                WorkItemKind::Implementation,
            )]),
        );
        let mut interaction = Interaction {
            selected_pane: Some("%15".into()),
            reveal_pane: true,
            ..Interaction::default()
        };
        let mut scroll = 0;
        let mut terminal = Terminal::new(TestBackend::new(100, 8)).unwrap();
        terminal
            .draw(|frame| render_sessions(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains(">  codex — Later agent"));
        assert!(text.contains("/work/my repo"));
        assert_eq!(interaction.selected_pane.as_deref(), Some("%15"));
        assert!(scroll > 0);
        let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
        interaction.selected_pane = Some("%14".into());
        interaction.reveal_pane = true;
        terminal
            .draw(|frame| render_sessions(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Registered: ABC-123"));
        assert!(!text.contains("w: work items"));
        theme::assert_text_style(
            terminal.backend().buffer(),
            "[Registered: ABC-123]",
            theme::GREEN,
            theme::SURFACE,
        );
        assert!(text.contains("Enter/r: register"));
    }

    fn work_screen(state: &AppState, width: u16, height: u16, scroll: &mut u16) -> String {
        item_screen(state, width, height, scroll, View::Work)
    }

    fn attention_screen(state: &AppState, width: u16, height: u16, scroll: &mut u16) -> String {
        item_screen(state, width, height, scroll, View::Attention)
    }

    fn item_screen(
        state: &AppState,
        width: u16,
        height: u16,
        scroll: &mut u16,
        view: View,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut interaction = Interaction::default();
        interaction.sync(state.items_for(view));
        terminal
            .draw(|frame| render_items(frame, state, scroll, &mut interaction, view))
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
            status_detail: "Unsupported foreground command.".into(),
            attention_prompt: None,
            completion_fingerprint: None,
        }
    }

    #[test]
    fn home_paths_are_abbreviated_only_at_directory_boundaries() {
        let home = Some(Path::new("/Users/person"));
        for (path, expected) in [
            ("/Users/person", "~"),
            ("/Users/person/", "~"),
            ("/Users/person/workspace/my repo", "~/workspace/my repo"),
            ("/Users/person/λ🙂", "~/λ🙂"),
            ("/Users/person-other/repo", "/Users/person-other/repo"),
            ("/Users/another/repo", "/Users/another/repo"),
            ("relative/path", "relative/path"),
        ] {
            assert_eq!(abbreviate_home(Path::new(path), home), expected);
        }
        for home in [
            None,
            Some(Path::new("")),
            Some(Path::new("relative")),
            Some(Path::new("/")),
        ] {
            assert_eq!(
                abbreviate_home(Path::new("/Users/person/repo"), home),
                "/Users/person/repo"
            );
        }
        assert_eq!(
            abbreviate_home(
                Path::new("/custom/home/repo"),
                Some(Path::new("/custom/home/"))
            ),
            "~/repo"
        );
    }

    #[test]
    fn shortcut_footers_have_separators_and_fit_an_80_column_terminal() {
        let state = AppState::default();
        for view in [View::Attention, View::Work] {
            let text = item_screen(&state, 80, 10, &mut 0, view);
            let maintenance = "e: edit description | n: rename ID | u: unregister";
            if view == View::Work {
                assert_eq!(text.lines().nth(8).unwrap().trim_end(), maintenance);
            } else {
                assert!(!text.contains(maintenance));
                assert_eq!(
                    text.lines().nth(8).unwrap().trim_end(),
                    "x: acknowledge finished turn"
                );
            }
            assert_eq!(
                text.lines().last().unwrap().trim_end(),
                "j/k | Enter: open | r: reply | d: review | Ctrl+d/u: scroll | a/w/s | q: quit"
            );
        }
        let text = screen(None, 80, 10, &mut 0);
        assert_eq!(
            text.lines().last().unwrap().trim_end(),
            "j/k: panes | Enter/r: register | Ctrl+d/u: scroll | a/w/s | q: quit"
        );
    }

    #[test]
    fn attention_shows_only_queue_items_with_prompt_context_and_compact_completion() {
        let mut waiting = registered(PaneAvailability::Present, WorkItemKind::Implementation);
        waiting.status = AgentStatus::WaitingForInput;
        waiting.attention_prompt = Some("Would you like to run the following command?\n$ cargo test -- λ🙂\n\nOptions:\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again (p)\n  3. No, and tell Codex what to do differently (esc)".into());
        let mut complete = registered(PaneAvailability::Present, WorkItemKind::ExternalReview);
        complete.item.id = "PR #1842".into();
        complete.status = AgentStatus::Complete;
        let mut hidden = registered(PaneAvailability::Present, WorkItemKind::Implementation);
        hidden.item.id = "HIDDEN_UNKNOWN".into();
        let mut running = hidden.clone();
        running.item.id = "HIDDEN_RUNNING".into();
        running.status = AgentStatus::Running;
        let state =
            AppState::from_refresh(Ok(snapshot()), Ok(vec![waiting, complete, hidden, running]));
        let text = attention_screen(&state, 120, 30, &mut 0);
        for expected in [
            "ATTENTION — 2",
            "ABC-123  Build  WAITING_FOR_INPUT",
            "Agent requests input:",
            "$ cargo test -- λ🙂",
            "Options:",
            "› 1. Yes, proceed (y)",
            "2. Yes, and don't ask again (p)",
            "3. No, and tell Codex what to do differently (esc)",
            "PR #1842  Review  TURN FINISHED",
            "1 UNKNOWN item(s)",
            "Enter: open",
            "r: reply",
            "Ctrl+d/u: scroll",
        ] {
            assert!(text.contains(expected), "missing {expected:?}: {text}");
        }
        assert!(!text.contains("HIDDEN_UNKNOWN"));
        assert!(!text.contains("HIDDEN_RUNNING"));
        for metadata in [
            "Fix retries",
            "Type:",
            "Workspace:",
            "Repository:",
            "Branch:",
        ] {
            assert!(!text.contains(metadata), "{text}");
        }
        assert!(!text.contains("Agent turn completed."), "{text}");
        assert!(
            !text.contains("Overall task completion is unverified."),
            "{text}"
        );
        assert!(!text.contains("Pane:"), "{text}");
        assert!(!text.contains("%14"), "{text}");
        assert!(!text.contains("(present)"), "{text}");
        assert!(
            state
                .attention
                .iter()
                .all(|item| item.item.pane_id == "%14")
        );
        let work = work_screen(&state, 120, 35, &mut 0);
        assert!(work.contains("attention: 2 (a)"));
        assert!(work.contains("Options:"));
        assert!(work.contains("› 1. Yes, proceed (y)"));
        assert!(attention_screen(&state, 80, 24, &mut 0).contains("q: quit"));
    }

    #[test]
    fn full_reason_and_command_render_and_remain_accessible_by_page_scrolling() {
        let mut waiting = registered(PaneAvailability::Present, WorkItemKind::Implementation);
        waiting.status = AgentStatus::WaitingForInput;
        waiting.attention_prompt = Some(format!(
            "Would you like to run the following command?\n\nEnvironment: local\n\nReason: {}Shared metadata requires write access. END-OF-REASON\n\n$ git -c commit.gpgsign=false commit -m \"Remove obsolete Packer installation\"\n\nOptions:\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again (p)\n     Permission applies only to this command prefix.\n  3. No, and tell Codex what to do differently (esc)",
            "Preserve signing settings for future commits. λ🙂 ".repeat(20)
        ));
        let state = AppState::from_refresh(Ok(snapshot()), Ok(vec![waiting]));
        let text = attention_screen(&state, 80, 45, &mut 0);
        let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(compact.contains("END-OF-REASON"), "{text}");
        assert!(
            compact
                .contains("$git-ccommit.gpgsign=falsecommit-m\"RemoveobsoletePackerinstallation\""),
            "{text}"
        );
        assert!(!text.contains('…'));
        assert!(text.contains("Ctrl+d/u: scroll"));

        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut interaction = Interaction::default();
        interaction.sync(&state.attention);
        let mut scroll = 0;
        let mut pages = String::new();
        let mut view = View::Attention;
        for _ in 0..8 {
            terminal
                .draw(|frame| render_attention(frame, &state, &mut scroll, &mut interaction))
                .unwrap();
            pages.extend(
                terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol()),
            );
            crate::navigate(
                crossterm::event::KeyCode::PageDown,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                10,
            );
        }
        assert!(pages.contains("END-OF-REASON"));
        assert!(pages.contains("git -c commit.gpgsign=false"));
        assert!(pages.contains("Packer installation"));
        assert!(pages.contains("Options:"));
        assert!(pages.contains("› 1. Yes, proceed (y)"));
        assert!(pages.contains("3. No, and tell Codex what to do differently (esc)"));
        assert!(pages.contains("Permission applies only to this command prefix."));
        assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
    }

    #[test]
    fn work_shows_options_only_for_the_selected_waiting_item_and_keeps_reply_editor() {
        let states = ["A", "B"].into_iter().map(|id| {
            let mut state = registered(PaneAvailability::Present, WorkItemKind::Implementation);
            state.item.id = id.into();
            state.status = AgentStatus::WaitingForInput;
            state.attention_prompt = Some(format!("Question for {id}\n\nOptions:\n› 1. Yes, approve {id} (y)\n  2. No, reject {id} (esc)"));
            state
        }).collect();
        let state = AppState::from_refresh(Ok(snapshot()), Ok(states));
        let mut interaction = Interaction::default();
        interaction.sync(state.items_for(View::Work));
        interaction.move_selection(state.items_for(View::Work), 1);
        interaction.begin_reply(state.items_for(View::Work));
        interaction.draft.as_mut().unwrap().append("y").unwrap();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
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
        for expected in [
            "> B  WAITING_FOR_INPUT",
            "Question for B",
            "Options:",
            "Yes, approve B (y)",
            "No, reject B (esc)",
            "Reply to B",
            "Enter: send",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(!text.contains("Question for A"));
        assert!(!text.contains("approve A"));
        assert_eq!(interaction.draft.as_ref().unwrap().text, "y");
        for (width, height) in [(45, 12), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
        }
    }

    #[test]
    fn work_metadata_uses_distinct_label_value_and_type_colors_without_changing_text() {
        for (kind, label, color) in [
            (WorkItemKind::Implementation, "Build", theme::BLUE),
            (WorkItemKind::ExternalReview, "Review", theme::PINK),
        ] {
            let state = AppState::from_refresh(
                Ok(snapshot()),
                Ok(vec![registered(PaneAvailability::Present, kind)]),
            );
            let mut interaction = Interaction::default();
            interaction.sync(state.items());
            let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
            terminal
                .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
            let buffer = terminal.backend().buffer();
            for (text, fg, bg) in [
                ("Type:", theme::LAVENDER, theme::BASE),
                (label, color, theme::BASE),
                ("Repository:", theme::LAVENDER, theme::BASE),
                ("/work/repo", theme::SKY, theme::BASE),
                ("Workspace:", theme::LAVENDER, theme::BASE),
                ("/work/ABC-123", theme::SKY, theme::BASE),
                ("Branch:", theme::LAVENDER, theme::BASE),
                ("fix/retries", theme::PINK, theme::BASE),
                ("Fix retries", theme::TEXT, theme::BASE),
                ("UNKNOWN", theme::PEACH, theme::SURFACE),
            ] {
                theme::assert_text_style(buffer, text, fg, bg);
            }
            assert_eq!(buffer[(119, 0)].bg, theme::MAUVE);
            assert_eq!(buffer[(119, 1)].bg, theme::SURFACE);
            assert_eq!(buffer[(119, 2)].bg, theme::BASE);
            let line = metadata("Branch", "feature/λ\x1b", Style::default().fg(theme::PINK));
            let rendered: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(rendered, "  Branch: feature/λ\\u{1b}");
        }
    }

    #[test]
    fn item_views_keep_status_colors_under_selection_and_style_prompt_options() {
        let mut waiting = registered(PaneAvailability::Present, WorkItemKind::Implementation);
        waiting.status = AgentStatus::WaitingForInput;
        waiting.attention_prompt = Some("Question\nReason: permission needed\n$ cargo test\n\nOptions:\n› 1. Yes, proceed (y)\n  2. No, cancel (esc)".into());
        let mut complete = waiting.clone();
        complete.item.id = "DONE".into();
        complete.item.kind = WorkItemKind::ExternalReview;
        complete.status = AgentStatus::Complete;
        complete.attention_prompt = None;
        let state = AppState::from_refresh(Ok(snapshot()), Ok(vec![waiting, complete]));
        let mut interaction = Interaction::default();
        interaction.sync(&state.attention);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| render_attention(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let buffer = terminal.backend().buffer();
        for (text, fg, bg) in [
            ("AGENT WORKBENCH", theme::MANTLE, theme::MAUVE),
            ("> ABC-123", theme::TEAL, theme::SURFACE),
            ("WAITING_FOR_INPUT", theme::YELLOW, theme::SURFACE),
            ("TURN FINISHED", theme::GREEN, theme::BASE),
            ("Options:", theme::TEAL, theme::BASE),
            ("› 1. Yes", theme::PEACH, theme::SURFACE),
            ("$ cargo test", theme::TEAL, theme::BASE),
            ("Reason:", theme::PEACH, theme::BASE),
            ("Build", theme::BLUE, theme::SURFACE),
            ("Review", theme::PINK, theme::BASE),
        ] {
            theme::assert_text_style(buffer, text, fg, bg);
        }
        assert!(
            buffer
                .content()
                .iter()
                .all(|cell| matches!(cell.bg, ratatui::style::Color::Rgb(_, _, _)))
        );
    }

    #[test]
    fn sessions_selection_metadata_and_errors_use_shared_theme() {
        let state = AppState::from_refresh(Ok(snapshot()), Ok(vec![]));
        let mut interaction = Interaction::default();
        interaction.sync_panes(&snapshot());
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| render_sessions(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let buffer = terminal.backend().buffer();
        theme::assert_text_style(buffer, "AGENT WORKBENCH", theme::MANTLE, theme::MAUVE);
        theme::assert_text_style(buffer, "codex", theme::TEXT, theme::SURFACE);
        theme::assert_text_style(buffer, " main", theme::PINK, theme::BASE);
        theme::assert_text_style(buffer, " auth", theme::BLUE, theme::BASE);
        theme::assert_text_style(buffer, "", theme::TEAL, theme::SURFACE);
        theme::assert_text_style(buffer, "/work/my repo", theme::SKY, theme::BASE);
        let failed = AppState::from_refresh(Err("test discovery error".into()), Ok(vec![]));
        terminal
            .draw(|frame| render_sessions(frame, &failed, &mut 0, &mut interaction))
            .unwrap();
        theme::assert_text_style(
            terminal.backend().buffer(),
            "Discovery unavailable",
            theme::RED,
            theme::BASE,
        );
    }

    #[test]
    fn attention_loading_empty_unknown_and_errors_do_not_claim_every_agent_is_idle() {
        assert!(
            attention_screen(&AppState::default(), 120, 12, &mut 0).contains("Loading work items")
        );
        let empty = AppState::from_refresh(Ok(Snapshot::default()), Ok(vec![]));
        assert!(attention_screen(&empty, 120, 12, &mut 0).contains("No work items registered"));
        let unknown = AppState::from_refresh(
            Err("tmux unreachable".into()),
            Ok(vec![registered(
                PaneAvailability::Unavailable,
                WorkItemKind::Implementation,
            )]),
        );
        let text = attention_screen(&unknown, 120, 15, &mut 0);
        for expected in [
            "ATTENTION — 0",
            "Discovery unavailable",
            "No known items need attention",
            "unclassified, not assumed idle",
            "Press w",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        let error =
            AppState::from_refresh(Ok(snapshot()), Err("unsupported schema version".into()));
        let text = attention_screen(&error, 120, 12, &mut 0);
        assert!(text.contains("ATTENTION — unavailable"));
        assert!(text.contains("unsupported schema version"));
        assert!(!text.contains("No known items need attention"));
        let mut scroll = u16::MAX;
        attention_screen(&error, 1, 1, &mut scroll);
    }

    #[test]
    fn prompt_wrapping_preserves_unicode_and_escapes_controls_without_breaking_scroll_rows() {
        let prompt = "λ🙂1234567890界text";
        let mut lines = Vec::new();
        push_prompt(&mut lines, prompt, 12);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.width() <= 12));
        let joined: String = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .trim_start()
                    .to_owned()
            })
            .collect();
        assert_eq!(joined, prompt);
        let mut lines = Vec::new();
        push_prompt(&mut lines, "one\x1btwo", 120);
        assert_eq!(lines[0].spans[0].content, "  one\\u{1b}two");
        push_prompt(&mut lines, "🙂", 1);
    }

    #[test]
    fn attention_selection_and_reply_editor_work_in_small_viewports() {
        let states = ["A", "B", "C"]
            .into_iter()
            .map(|id| {
                let mut state = registered(PaneAvailability::Present, WorkItemKind::Implementation);
                state.item.id = id.into();
                state.status = AgentStatus::WaitingForInput;
                state.attention_prompt =
                    Some("Would you like to run the following command?\n$ cargo test".into());
                state
            })
            .collect();
        let state = AppState::from_refresh(Ok(snapshot()), Ok(states));
        let mut interaction = Interaction::default();
        interaction.sync(&state.attention);
        interaction.move_selection(&state.attention, 2);
        interaction.begin_reply(&state.attention);
        interaction
            .draft
            .as_mut()
            .unwrap()
            .append("yes λ🙂")
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut scroll = 0;
        terminal
            .draw(|frame| render_attention(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "> C  Build  WAITING_FOR_INPUT",
            "Reply to C",
            "yes λ🙂",
            "Enter: send",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(scroll > 0);
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();
        terminal
            .draw(|frame| render_attention(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
    }

    #[test]
    fn work_view_renders_each_observed_status_and_explanation() {
        for status in [
            AgentStatus::Running,
            AgentStatus::WaitingForInput,
            AgentStatus::Idle,
            AgentStatus::Complete,
            AgentStatus::Unknown,
        ] {
            let mut item = registered(PaneAvailability::Present, WorkItemKind::Implementation);
            item.status = status;
            item.status_detail = "Observed locally; task completion unverified.".into();
            let state = AppState {
                discovery: None,
                work_items: Some(Ok(vec![item])),
                ..AppState::default()
            };
            let text = work_screen(&state, 100, 15, &mut 0);
            assert!(text.contains(&status.to_string()), "{text}");
            assert!(
                text.contains("Observed locally; task completion unverified."),
                "{text}"
            );
        }
    }

    #[test]
    fn finished_attention_items_fit_one_row_and_keep_work_details_and_storage_unchanged() {
        for (kind, label, full_name) in [
            (WorkItemKind::Implementation, "Build", "Implementation"),
            (WorkItemKind::ExternalReview, "Review", "External Review"),
        ] {
            let mut finished = registered(PaneAvailability::Present, kind);
            finished.status = AgentStatus::Complete;
            let original = finished.clone();
            let state = AppState::from_refresh(Ok(snapshot()), Ok(vec![finished]));
            // Header + one content row + two shortcut rows.
            let text = attention_screen(&state, 80, 4, &mut 0);
            let rows: Vec<_> = text.lines().map(str::trim_end).collect();
            assert_eq!(rows[1], format!("> ABC-123  {label}  TURN FINISHED"));
            assert_eq!(rows[2], "x: acknowledge finished turn");
            assert_eq!(state.items(), std::slice::from_ref(&original));
            assert_eq!(state.items()[0].item.kind.to_string(), full_name);
            let work = work_screen(&state, 100, 12, &mut 0);
            assert!(work.contains(&format!("Type: {label}")), "{work}");
            for detail in [
                "Fix retries",
                "Workspace: /work/ABC-123",
                "Repository: /work/repo",
                "Branch: fix/retries",
            ] {
                assert!(work.contains(detail), "{work}");
            }
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
                    ..AppState::default()
                };
                let text = work_screen(&state, 120, 15, &mut 0);
                for expected in [
                    "WORK",
                    "ABC-123  UNKNOWN",
                    "Fix retries",
                    "/work/repo",
                    "/work/ABC-123",
                    "Branch: fix/retries",
                    work_kind_label(kind),
                ] {
                    assert!(text.contains(expected), "missing {expected:?} in {text}");
                }
                assert!(!text.contains("%14"), "{text}");
                assert!(!text.contains("Pane:"), "{text}");
                match pane {
                    PaneAvailability::Present => assert!(!text.contains("Agent pane"), "{text}"),
                    PaneAvailability::Missing => {
                        assert!(text.contains("Agent pane missing."), "{text}")
                    }
                    PaneAvailability::Unavailable => {
                        assert!(text.contains("Agent pane unavailable."), "{text}")
                    }
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
            ..AppState::default()
        };
        let text = work_screen(&state, 120, 15, &mut 0);
        assert!(text.contains("Discovery unavailable"));
        assert!(text.contains("ABC-123  UNKNOWN"));
        assert!(text.contains("Agent pane unavailable."));
        assert!(!text.contains("%14"));
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
        assert!(text.contains("r to register"));
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
        assert!(
            text.contains("Enter: send | Esc/Ctrl-C: cancel | Backspace: edit | Ctrl-u: clear")
        );
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
    }

    #[test]
    fn displays_human_context_without_tmux_ids_indices_or_metadata_labels() {
        let text = screen(Some(Ok(snapshot())), 100, 12, &mut 0);
        for expected in [
            "SESSIONS",
            " main",
            "   auth",
            ">  codex — Agent",
            "/work/my repo",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        for hidden in [
            "$1", "@2", "%14", "pane 0", "2  auth", "command:", "title:", "cwd:",
        ] {
            assert!(!text.contains(hidden), "unexpected {hidden:?} in {text}");
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
        assert!(text.contains(">  unavailable"));
        assert!(text.contains("Directory unavailable"));
        assert!(text.contains("one\\ntwo\\tthree\\u{1b}"));
    }

    #[test]
    fn clamps_scroll_when_snapshot_shrinks_and_handles_small_terminals() {
        let mut scroll = u16::MAX;
        let text = screen(Some(Ok(snapshot())), 100, 5, &mut scroll);
        assert_eq!(scroll, 3);
        assert!(text.contains("/work/my repo"));
        screen(Some(Ok(Snapshot::default())), 10, 2, &mut scroll);
        assert_eq!(scroll, 1);
        screen(Some(Ok(snapshot())), 1, 1, &mut scroll);
    }
}
