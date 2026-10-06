use std::path::{Path, PathBuf};

use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use crate::AppState;
use crate::interaction::{ApprovalPhase, Interaction};
use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
    #[default]
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
    theme::paint(frame);
    let [header, body, notice, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(if interaction.message.is_some() { 2 } else { 0 }),
        Constraint::Length(if interaction.draft.is_some() { 3 } else { 0 }),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let count = match &state.work_items {
        None => "…".into(),
        Some(Err(_)) => "unavailable".into(),
        Some(Ok(_)) => state.attention.len().to_string(),
    };
    let title = format!("AGENT WORKBENCH — WORK • attention: {count}");
    frame.render_widget(Paragraph::new(title).style(theme::header()), header);
    let mut lines = Vec::new();
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
            // Each item always occupies exactly one row. Prompts, errors and
            // metadata live in a separate viewport and cannot move list rows.
            let list_height = items.len().min(usize::from((body.height / 3).max(1))) as u16;
            let [list, details] =
                Layout::vertical([Constraint::Length(list_height), Constraint::Min(0)]).areas(body);
            if let Some(index) = items
                .iter()
                .position(|state| interaction.selected_id.as_ref() == Some(&state.item.id))
            {
                if index < interaction.work_list_offset {
                    interaction.work_list_offset = index;
                }
                if index >= interaction.work_list_offset + usize::from(list.height) {
                    interaction.work_list_offset =
                        index.saturating_sub(usize::from(list.height.saturating_sub(1)));
                }
            }
            interaction.work_list_offset = interaction
                .work_list_offset
                .min(items.len().saturating_sub(usize::from(list.height)));
            // Measure all registered IDs, not only visible rows, so scrolling
            // and status changes cannot shift the columns. Reserve space for
            // selection, gaps, attention and a feedback glyph. Reserve the same
            // space even without feedback so approval never shifts the columns.
            let available = usize::from(list.width).saturating_sub(10);
            let id_width = items
                .iter()
                .map(|entry| Line::from(visible(&entry.item.id)).width())
                .max()
                .unwrap_or(0)
                .min(
                    available
                        .saturating_sub(WORK_STATUS_WIDTH)
                        .max(available / 3),
                );
            let status_width = WORK_STATUS_WIDTH.min(available.saturating_sub(id_width));
            for entry in items
                .iter()
                .skip(interaction.work_list_offset)
                .take(usize::from(list.height))
            {
                let item = &entry.item;
                let selected = interaction.selected_id.as_ref() == Some(&item.id);
                let mut heading = vec![Span::styled(
                    format!(
                        "{}{}  ",
                        if selected { "> " } else { "  " },
                        work_cell(&visible(&item.id), id_width),
                    ),
                    theme::accent(),
                )];
                heading.push(Span::styled(
                    work_cell(&entry.status.to_string(), status_width),
                    theme::status(entry.status),
                ));
                let attention = state
                    .attention
                    .iter()
                    .any(|attention| attention.item.id == item.id);
                heading.push(Span::styled(
                    if attention { "  !" } else { "   " },
                    theme::notice(),
                ));
                if let Some(feedback) = interaction
                    .approval_feedback
                    .as_ref()
                    .filter(|feedback| feedback.item_id == item.id)
                {
                    heading.push(Span::styled(
                        format!("  {}", feedback.label()),
                        approval_feedback_style(feedback.phase),
                    ));
                }
                lines.push(selectable_line(Line::from(heading), selected, list.width));
            }
            frame.render_widget(Paragraph::new(std::mem::take(&mut lines)), list);
            let selected = interaction.selected(items);
            let selected_id = selected.map(|state| state.item.id.clone());
            if interaction.detail_item_id != selected_id {
                *scroll = 0;
                interaction.detail_item_id = selected_id;
            }
            let feedback = interaction
                .approval_feedback
                .as_ref()
                .filter(|feedback| selected.is_some_and(|state| feedback.item_id == state.item.id));
            let mut title = vec![Span::raw("Details")];
            if let Some(feedback) = feedback {
                title.push(Span::raw(" — "));
                title.push(Span::styled(
                    feedback.label(),
                    approval_feedback_style(feedback.phase),
                ));
            }
            if let Some(state) = selected {
                title.push(Span::raw(format!(" — {}", visible(&state.item.id))));
            }
            let block = Block::bordered()
                .title(Line::from(title))
                .border_style(if feedback.is_some_and(|feedback| feedback.is_pulsing()) {
                    Style::default().fg(theme::GREEN)
                } else {
                    theme::border(true)
                })
                .title_style(theme::accent());
            let inner = block.inner(details);
            frame.render_widget(block, details);
            if let Some(Err(error)) = &state.discovery {
                lines.push(Line::styled(
                    format!("Discovery unavailable: {}", visible(error)),
                    theme::error(),
                ));
                lines.push(Line::from(
                    "Saved items remain registered. Retrying automatically every 2 seconds.",
                ));
            }
            if let Some(state) = selected {
                let item = &state.item;
                lines.push(Line::styled(visible(&item.title), theme::text()));
                lines.push(metadata(
                    "Type",
                    work_kind_label(item.kind),
                    theme::work_kind(item.kind),
                ));
                if state.status != workbench_core::AgentStatus::Complete {
                    lines.push(metadata("Status", &state.status_detail, theme::muted()));
                }
                match state.pane {
                    workbench_core::PaneAvailability::Present => {}
                    workbench_core::PaneAvailability::Missing => {
                        lines.push(Line::styled("  Agent pane missing.", theme::notice()))
                    }
                    workbench_core::PaneAvailability::Unavailable => {
                        lines.push(Line::styled("  Agent pane unavailable.", theme::notice()))
                    }
                }
                lines.push(metadata(
                    "Repository",
                    &display_path(&item.repository),
                    theme::path(),
                ));
                lines.push(metadata(
                    "Workspace",
                    &display_path(&item.workspace),
                    theme::path(),
                ));
                if let Some(branch) = &item.branch {
                    lines.push(metadata("Branch", branch, theme::muted()));
                }
                if state.status == workbench_core::AgentStatus::WaitingForInput {
                    lines.push(Line::from(""));
                    lines.push(Line::styled("  Agent requests input:", theme::notice()));
                    if let Some(prompt) = &state.attention_prompt {
                        push_prompt(&mut lines, prompt, inner.width);
                    } else {
                        lines.push(Line::from(format!("  {}", visible(&state.status_detail))));
                    }
                }
            } else {
                lines.push(Line::from("Select a work item to inspect its details."));
            }
            let max_scroll = lines
                .len()
                .saturating_sub(usize::from(inner.height))
                .min(usize::from(u16::MAX)) as u16;
            *scroll = (*scroll).min(max_scroll);
            frame.render_widget(
                Paragraph::new(std::mem::take(&mut lines)).scroll((*scroll, 0)),
                inner,
            );
        }
    }
    if !lines.is_empty() {
        frame.render_widget(Paragraph::new(lines), body);
        *scroll = 0;
    }
    interaction.reveal_selection = false;
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
        let width = draft.cursor.column(&draft.text);
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
            crate::help::Context::Reply.hints()
        } else if let Some(bar) = &interaction.response_bar {
            crate::help::Context::Response {
                changed: bar.changed,
                can_reject: bar.can_reject(),
            }
            .hints()
        } else {
            crate::help::Context::Work.hints()
        }),
        footer,
    );
}

// Keep attention in a fixed column even when no current item has the longest
// status label. This is presentation sizing, not an agent-state decision.
const WORK_STATUS_WIDTH: usize = 17; // WAITING_FOR_INPUT

fn approval_feedback_style(phase: ApprovalPhase) -> Style {
    match phase {
        ApprovalPhase::Checking => theme::notice(),
        ApprovalPhase::Sent => Style::default().fg(theme::GREEN),
        ApprovalPhase::Failed => theme::error(),
    }
}

fn work_cell(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut cell = if Line::from(text).width() <= width {
        text.to_owned()
    } else {
        let mut end = 0;
        for (index, ch) in text.char_indices() {
            let next = index + ch.len_utf8();
            if Line::from(&text[..next]).width() > width - 1 {
                break;
            }
            end = next;
        }
        format!("{}…", &text[..end])
    };
    cell.push_str(&" ".repeat(width.saturating_sub(Line::from(cell.as_str()).width())));
    cell
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
        Constraint::Length(1),
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
                    theme::accent().fg(theme::MAUVE),
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
        theme::footer(
            crate::help::Context::Sessions {
                all: interaction.show_all_panes,
            }
            .hints(),
        ),
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
        assert!(text.contains("All panes: f"));
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
            "Agents only: f",
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
        assert!(text.contains("Register: <enter>/r"));
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
    fn shortcut_footers_use_one_row_with_overflow_and_help_at_80_columns() {
        let state = AppState::default();
        let text = work_screen(&state, 80, 10, &mut 0);
        assert_eq!(
            text.lines().last().unwrap().trim_end(),
            "Respond: r | Edit: e | Unregister: u | Clean up: c | … | Help: ?"
        );
        assert_eq!(text.lines().nth(8).unwrap().trim_end(), "");
        let text = screen(None, 80, 10, &mut 0);
        assert_eq!(
            text.lines().last().unwrap().trim_end(),
            "Agents only: f | Views: w/s | Quit: q | Select: j/k | … | Help: ?"
        );
    }

    #[test]
    fn unified_work_shows_every_item_attention_flags_and_selected_details() {
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
        let text = work_screen(&state, 320, 30, &mut 0);
        for expected in [
            "WORK • attention: 2",
            "ABC-123         WAITING_FOR_INPUT  !",
            "Agent requests input:",
            "$ cargo test -- λ🙂",
            "Options:",
            "› 1. Yes, proceed (y)",
            "2. Yes, and don't ask again (p)",
            "3. No, and tell Codex what to do differently (esc)",
            "PR #1842        TURN FINISHED      !",
            "HIDDEN_UNKNOWN  UNKNOWN",
            "HIDDEN_RUNNING  RUNNING",
            "Details — ABC-123",
            "Open: <enter>",
            "Respond: r",
            "Help: ?",
        ] {
            assert!(text.contains(expected), "missing {expected:?}: {text}");
        }
        for metadata in [
            "Fix retries",
            "Type:",
            "Workspace:",
            "Repository:",
            "Branch:",
        ] {
            assert!(text.contains(metadata), "{text}");
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
        assert!(work.contains("attention: 2"));
        assert!(work.contains("Options:"));
        assert!(work.contains("› 1. Yes, proceed (y)"));
        let narrow = work_screen(&state, 80, 24, &mut 0);
        for hint in ["Respond: r", "Help: ?"] {
            assert!(narrow.contains(hint));
        }
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
        let text = work_screen(&state, 80, 45, &mut 0);
        let compact: String = text
            .chars()
            .filter(|ch| !ch.is_whitespace() && *ch != '│')
            .collect();
        assert!(compact.contains("END-OF-REASON"), "{text}");
        assert!(
            compact
                .contains("$git-ccommit.gpgsign=falsecommit-m\"RemoveobsoletePackerinstallation\""),
            "{text}"
        );
        // Prompt content is still complete; only the compact footer truncates.
        assert!(!text.lines().take(44).any(|line| line.contains('…')));
        assert!(text.lines().last().unwrap().contains("… | Help: ?"));

        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut interaction = Interaction::default();
        interaction.sync(&state.attention);
        let mut scroll = 0;
        let mut pages = String::new();
        let mut view = View::Work;
        for _ in 0..20 {
            terminal
                .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
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
                crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('d'),
                    crossterm::event::KeyModifiers::CONTROL,
                ),
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
        interaction.sync(state.items());
        interaction.move_selection(state.items(), 1);
        interaction.begin_reply(state.items());
        interaction.draft.as_mut().unwrap().insert("y").unwrap();
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
            "Send: <enter>",
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
                ("Type:", theme::SUBTEXT, theme::BASE),
                (label, color, theme::BASE),
                ("Repository:", theme::SUBTEXT, theme::BASE),
                ("/work/repo", theme::TEXT, theme::BASE),
                ("Workspace:", theme::SUBTEXT, theme::BASE),
                ("/work/ABC-123", theme::TEXT, theme::BASE),
                ("Branch:", theme::SUBTEXT, theme::BASE),
                ("fix/retries", theme::SUBTEXT, theme::BASE),
                ("Fix retries", theme::TEXT, theme::BASE),
                ("UNKNOWN", theme::SUBTEXT, theme::SURFACE),
            ] {
                theme::assert_text_style(buffer, text, fg, bg);
            }
            assert_eq!(buffer[(119, 0)].bg, theme::MANTLE);
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

    fn approval_fixture() -> (AppState, workbench_core::ApprovalRequest) {
        let mut item = registered(PaneAvailability::Present, WorkItemKind::Implementation);
        item.status = AgentStatus::WaitingForInput;
        item.attention_prompt = Some("Would you like to run the following command?\n$ cargo test\n\nOptions:\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)".into());
        let request = workbench_core::ApprovalRequest::capture(
            &item,
            workbench_core::ApprovalDecision::ApproveOnce,
        )
        .unwrap();
        let mut other = item.clone();
        other.item.id = "other".into();
        (
            AppState::from_refresh(Ok(snapshot()), Ok(vec![item, other])),
            request,
        )
    }

    #[test]
    fn response_bar_replaces_only_the_footer_and_leaves_the_request_viewport_intact() {
        let (state, _) = approval_fixture();
        for (width, height) in [(120, 30), (80, 24), (40, 14), (1, 1), (0, 1)] {
            let mut interaction = Interaction::default();
            interaction.sync(state.items());
            let mut scroll = 0;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
                .unwrap();
            let baseline = terminal.backend().buffer().clone();
            interaction.begin_response(state.items());
            terminal
                .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
                .unwrap();
            let buffer = terminal.backend().buffer();
            for y in 0..height - 1 {
                for x in 0..width {
                    assert_eq!(buffer[(x, y)], baseline[(x, y)]);
                }
            }
            if width >= 80 {
                let footer: String = buffer
                    .content()
                    .chunks(usize::from(width))
                    .last()
                    .unwrap()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                for hint in ["Approve once: y", "Reject: n", "Cancel: Esc", "Help: ?"] {
                    assert!(footer.contains(hint), "{footer}");
                }
                assert!(!footer.contains("Unregister"));
            }
            assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
            interaction.response_key(crossterm::event::KeyCode::Esc.into(), state.items());
            terminal
                .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
                .unwrap();
            assert_eq!(terminal.backend().buffer(), &baseline);
        }
    }

    #[test]
    fn changed_response_bar_shows_recovery_in_the_footer_without_overlaying_the_prompt() {
        let (mut state, _) = approval_fixture();
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        interaction.begin_response(state.items());
        if let Some(Ok(items)) = &mut state.work_items {
            items[0].attention_prompt = Some("A new question remains visible".into());
        }
        interaction.sync(state.items());
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
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
        assert!(text.contains("A new question remains visible"), "{text}");
        assert!(
            text.contains("Request changed; cancel and reopen."),
            "{text}"
        );
        assert!(!text.contains("Approve once: y"), "{text}");
    }

    #[test]
    fn approval_feedback_is_inline_green_and_does_not_move_rows_or_fake_running() {
        let (state, request) = approval_fixture();
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut scroll = 0;
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        let baseline = terminal.backend().buffer().clone();
        interaction.begin_approval(&request);
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        theme::assert_text_style(
            terminal.backend().buffer(),
            "… Checking approval",
            theme::YELLOW,
            theme::SURFACE,
        );
        interaction.finish_approval(&request, Ok(()));
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        let confirmed = terminal.backend().buffer();
        theme::assert_text_style(confirmed, "✓ Approval sent", theme::GREEN, theme::SURFACE);
        assert_eq!(confirmed[(0, 3)].fg, theme::GREEN); // selected details border
        for y in [1, 2] {
            // ID, actual status, and attention retain their exact cells.
            for x in 0..31 {
                assert_eq!(confirmed[(x, y)], baseline[(x, y)]);
            }
        }
        assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
        assert_eq!(state.items()[0].status, AgentStatus::WaitingForInput);
        assert_eq!(state.attention.len(), 2);
        assert!(interaction.message.is_none());
        let after_send = std::time::Instant::now();
        assert!(
            interaction.tick_approval_feedback(after_send + std::time::Duration::from_millis(300))
        );
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 3)].fg, theme::TEAL);
        assert!(
            interaction.tick_approval_feedback(after_send + std::time::Duration::from_millis(1500))
        );
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        assert_eq!(terminal.backend().buffer(), &baseline);
    }

    #[test]
    fn approval_feedback_stays_with_its_item_and_fits_narrow_terminals() {
        let (mut state, request) = approval_fixture();
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        interaction.begin_approval(&request);
        interaction.finish_approval(&request, Ok(()));
        interaction.move_selection(state.items(), 1);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let rows: Vec<String> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        assert!(rows[1].contains("✓ Approval sent"));
        assert!(!rows[2].contains("✓"));
        assert!(rows[3].contains("Details — other"));
        assert_eq!(terminal.backend().buffer()[(0, 3)].fg, theme::TEAL);
        if let Some(Ok(items)) = &mut state.work_items {
            items[0].status = AgentStatus::Running;
        }
        interaction.move_selection(state.items(), -1);
        for (width, height) in [(40, 14), (24, 8), (1, 1), (0, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
            if width == 40 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("RUNNING"), "{text}");
                assert!(text.contains("✓"), "{text}");
            }
        }
        interaction.finish_approval(&request, Err(workbench_core::ApprovalError::Changed));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        theme::assert_text_style(
            terminal.backend().buffer(),
            "✗ Approval failed",
            theme::RED,
            theme::SURFACE,
        );
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
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let buffer = terminal.backend().buffer();
        for (text, fg, bg) in [
            ("AGENT WORKBENCH", theme::LAVENDER, theme::MANTLE),
            ("> ABC-123", theme::TEAL, theme::SURFACE),
            ("WAITING_FOR_INPUT", theme::YELLOW, theme::SURFACE),
            ("TURN FINISHED", theme::GREEN, theme::BASE),
            ("Options:", theme::TEAL, theme::BASE),
            ("› 1. Yes", theme::MAUVE, theme::BASE),
            ("$ cargo test", theme::TEAL, theme::BASE),
            ("Reason:", theme::PEACH, theme::BASE),
            ("Build", theme::BLUE, theme::BASE),
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
        theme::assert_text_style(buffer, "AGENT WORKBENCH", theme::LAVENDER, theme::MANTLE);
        theme::assert_text_style(buffer, "codex", theme::TEXT, theme::SURFACE);
        theme::assert_text_style(buffer, " main", theme::MAUVE, theme::BASE);
        theme::assert_text_style(buffer, " auth", theme::BLUE, theme::BASE);
        theme::assert_text_style(buffer, "", theme::TEAL, theme::SURFACE);
        theme::assert_text_style(buffer, "/work/my repo", theme::TEXT, theme::BASE);
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
    fn work_loading_empty_unknown_and_errors_do_not_claim_every_agent_is_idle() {
        assert!(work_screen(&AppState::default(), 120, 12, &mut 0).contains("Loading work items"));
        let empty = AppState::from_refresh(Ok(Snapshot::default()), Ok(vec![]));
        assert!(work_screen(&empty, 120, 12, &mut 0).contains("No work items registered"));
        let unknown = AppState::from_refresh(
            Err("tmux unreachable".into()),
            Ok(vec![registered(
                PaneAvailability::Unavailable,
                WorkItemKind::Implementation,
            )]),
        );
        let text = work_screen(&unknown, 120, 15, &mut 0);
        for expected in [
            "WORK • attention: 0",
            "Discovery unavailable",
            "ABC-123  UNKNOWN",
            "Agent pane unavailable",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        let error =
            AppState::from_refresh(Ok(snapshot()), Err("unsupported schema version".into()));
        let text = work_screen(&error, 120, 12, &mut 0);
        assert!(text.contains("WORK • attention: unavailable"));
        assert!(text.contains("unsupported schema version"));
        assert!(!text.contains("No known items need attention"));
        let mut scroll = u16::MAX;
        work_screen(&error, 1, 1, &mut scroll);
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
    fn work_selection_and_reply_editor_work_in_small_viewports() {
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
            .insert("yes λ🙂")
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
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
        for expected in [
            "> C  WAITING_FOR_INPUT",
            "Reply to C",
            "yes λ🙂",
            "Send: <enter>",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(interaction.work_list_offset > 0);
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
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
            assert_eq!(
                text.contains("Observed locally; task completion unverified."),
                status != AgentStatus::Complete,
                "{text}"
            );
        }
    }

    #[test]
    fn finished_work_items_fit_one_row_and_keep_details_and_storage_unchanged() {
        for (kind, label, full_name) in [
            (WorkItemKind::Implementation, "Build", "Implementation"),
            (WorkItemKind::ExternalReview, "Review", "External Review"),
        ] {
            let mut finished = registered(PaneAvailability::Present, kind);
            finished.status = AgentStatus::Complete;
            let original = finished.clone();
            let state = AppState::from_refresh(Ok(snapshot()), Ok(vec![finished]));
            // Header + one content row + one compact shortcut row.
            let text = work_screen(&state, 80, 3, &mut 0);
            let rows: Vec<_> = text.lines().map(str::trim_end).collect();
            assert_eq!(rows[1], "> ABC-123  TURN FINISHED      !");
            assert!(!rows[1].contains(label));
            assert_eq!(
                rows[2],
                "Respond: r | Edit: e | Unregister: u | Clean up: c | … | Help: ?"
            );
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
        assert!(interaction.work_list_offset > 0);
    }

    #[test]
    fn status_and_prompt_changes_never_move_list_rows_or_change_selection() {
        let items = ["A", "B", "C", "D", "E", "F"]
            .into_iter()
            .map(|id| {
                let mut item = registered(PaneAvailability::Present, WorkItemKind::Implementation);
                item.item.id = id.into();
                item.status = AgentStatus::Running;
                item
            })
            .collect();
        let mut state = AppState::from_refresh(Ok(snapshot()), Ok(items));
        let mut interaction = Interaction {
            selected_id: Some("F".into()),
            ..Interaction::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 14)).unwrap();
        let mut scroll = 0;
        let mut initial_row = None;
        let mut initial_offset = None;
        for status in [
            AgentStatus::Running,
            AgentStatus::WaitingForInput,
            AgentStatus::Complete,
            AgentStatus::Unknown,
            AgentStatus::Running,
        ] {
            if let Some(Ok(items)) = &mut state.work_items {
                items[0].status = AgentStatus::WaitingForInput;
                items[0].attention_prompt = Some("UNSELECTED PROMPT".repeat(500));
                items[5].status = status;
                items[5].attention_prompt = Some("SELECTED PROMPT\n".repeat(200));
            }
            state.refresh_attention(&mut interaction.attention_tracker);
            state.sync_selection(&mut interaction);
            terminal
                .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
                .unwrap();
            let rows: Vec<String> = terminal
                .backend()
                .buffer()
                .content()
                .chunks(80)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect())
                .collect();
            let selected_row = rows
                .iter()
                .position(|row| row.starts_with("> F  "))
                .unwrap();
            assert_eq!(*initial_row.get_or_insert(selected_row), selected_row);
            assert_eq!(
                *initial_offset.get_or_insert(interaction.work_list_offset),
                interaction.work_list_offset
            );
            assert!(rows.iter().any(|row| row.contains("Details — F")));
            assert!(!rows.iter().any(|row| row.contains("UNSELECTED PROMPT")));
            assert_eq!(interaction.selected_id.as_deref(), Some("F"));
        }
    }

    #[test]
    fn compact_work_columns_align_mixed_id_widths_statuses_and_attention() {
        let items = [
            ("nvim", AgentStatus::Complete),
            ("workbench", AgentStatus::WaitingForInput),
            ("界🙂e\u{301}", AgentStatus::Complete),
        ]
        .into_iter()
        .map(|(id, status)| {
            let mut item = registered(PaneAvailability::Present, WorkItemKind::Implementation);
            item.item.id = id.into();
            item.status = status;
            item
        })
        .collect();
        let state = AppState::from_refresh(Ok(snapshot()), Ok(items));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut interaction = Interaction {
            selected_id: Some("workbench".into()),
            ..Interaction::default()
        };
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        for (index, status) in ["TURN FINISHED", "WAITING_FOR_INPUT", "TURN FINISHED"]
            .into_iter()
            .enumerate()
        {
            let row = &terminal.backend().buffer().content()[(index + 1) * 80..(index + 2) * 80];
            assert_eq!(
                row.iter().position(|cell| cell.symbol() == &status[..1]),
                Some(13)
            ); // prefix + longest ID + gap
            assert_eq!(row.iter().position(|cell| cell.symbol() == "!"), Some(32));
            let text: String = row.iter().map(|cell| cell.symbol()).collect();
            assert!(!text.contains("Build"));
            assert!(!text.contains("Review"));
        }
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Type: Build"));
        assert_eq!(interaction.selected_id.as_deref(), Some("workbench"));
    }

    #[test]
    fn work_cells_pad_and_truncate_by_display_width_not_bytes_or_characters() {
        for (text, width, expected) in [
            ("nvim", 9, "nvim     "),
            ("界🙂", 6, "界🙂  "),
            ("界🙂long", 5, "界🙂…"),
            ("界🙂long", 4, "界… "),
            ("e\u{301}", 3, "e\u{301}  "),
            ("abcdef", 1, "…"),
            ("abcdef", 0, ""),
        ] {
            assert_eq!(work_cell(text, width), expected);
            assert_eq!(Line::from(work_cell(text, width)).width(), width);
        }
        for status in [
            AgentStatus::Running,
            AgentStatus::WaitingForInput,
            AgentStatus::Idle,
            AgentStatus::Complete,
            AgentStatus::Unknown,
        ] {
            assert!(Line::from(status.to_string()).width() <= WORK_STATUS_WIDTH);
        }
    }

    #[test]
    fn long_ids_fit_without_hiding_status_and_columns_stay_fixed_while_scrolling() {
        let long_id = "long-task-".repeat(10);
        let items = ["nvim", "workbench", "third", long_id.as_str()]
            .into_iter()
            .map(|id| {
                let mut item = registered(PaneAvailability::Present, WorkItemKind::ExternalReview);
                item.item.id = id.into();
                item.status = AgentStatus::WaitingForInput;
                item
            })
            .collect();
        let state = AppState::from_refresh(Ok(snapshot()), Ok(items));
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut interaction = Interaction {
            selected_id: Some("nvim".into()),
            ..Interaction::default()
        };
        let mut status_column = None;
        for id in ["nvim", "workbench", long_id.as_str()] {
            interaction.selected_id = Some(id.into());
            terminal
                .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
            let row = terminal
                .backend()
                .buffer()
                .content()
                .chunks(40)
                .find(|row| row[0].symbol() == ">")
                .unwrap();
            let column = row.iter().position(|cell| cell.symbol() == "W").unwrap();
            assert_eq!(*status_column.get_or_insert(column), column);
            let text: String = row.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("WAITING_FOR_INPUT"));
            assert!(text.contains('!'));
            if id == long_id {
                assert!(text.contains('…'));
            }
        }
        assert_eq!(state.items()[3].item.id, long_id);
        for (width, height) in [(0, 0), (1, 1), (10, 8), (20, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
        }
    }

    #[test]
    fn detail_scrolling_is_independent_of_the_list_and_resets_only_for_a_new_item() {
        let items = ["A", "B"]
            .into_iter()
            .map(|id| {
                let mut item = registered(PaneAvailability::Present, WorkItemKind::Implementation);
                item.item.id = id.into();
                item.status = AgentStatus::WaitingForInput;
                item.attention_prompt = Some(format!("PROMPT FOR {id}\n").repeat(200));
                item
            })
            .collect();
        let state = AppState::from_refresh(Ok(snapshot()), Ok(items));
        let mut interaction = Interaction {
            selected_id: Some("B".into()),
            ..Interaction::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        let mut scroll = 0;
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        scroll = 10;
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        assert_eq!(scroll, 10);
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("> B  WAITING_FOR_INPUT"));
        assert!(text.contains("Details — B"));
        assert!(text.contains("PROMPT FOR B"));
        let mut updated = state.clone();
        if let Some(Ok(items)) = &mut updated.work_items {
            items[1]
                .attention_prompt
                .as_mut()
                .unwrap()
                .push_str("NEW OPTION\n");
        }
        updated.sync_selection(&mut interaction);
        terminal
            .draw(|frame| render_work(frame, &updated, &mut scroll, &mut interaction))
            .unwrap();
        assert_eq!(scroll, 10);
        interaction.move_selection(state.items(), -1);
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        assert_eq!(scroll, 0);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        assert_eq!(interaction.detail_item_id.as_deref(), Some("A"));
    }

    #[test]
    fn acknowledgement_only_removes_attention_marker_not_the_selected_row() {
        let mut item = registered(PaneAvailability::Present, WorkItemKind::Implementation);
        item.status = AgentStatus::Complete;
        item.completion_fingerprint = Some(42);
        let mut state = AppState::from_refresh(Ok(snapshot()), Ok(vec![item]));
        let mut interaction = Interaction::default();
        state.refresh_attention(&mut interaction.attention_tracker);
        state.sync_selection(&mut interaction);
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let row = |terminal: &Terminal<TestBackend>| -> String {
            terminal
                .backend()
                .buffer()
                .content()
                .chunks(80)
                .nth(1)
                .unwrap()
                .iter()
                .map(|cell| cell.symbol())
                .collect()
        };
        assert!(row(&terminal).contains("TURN FINISHED      !"));
        interaction.acknowledgement_requested = Some(
            interaction
                .attention_tracker
                .capture(&state.items()[0])
                .unwrap(),
        );
        state.apply_acknowledgement(&mut interaction);
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        assert!(row(&terminal).starts_with("> ABC-123  TURN FINISHED"));
        assert!(!row(&terminal).contains('!'));
        assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
        assert_eq!(state.items().len(), 1);
        assert!(state.attention.is_empty());
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
            .insert("yes λ🙂")
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
        assert!(text.contains("Send: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace"));
        assert!(text.contains("Clear: <c-u> | …"));
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
    }

    #[test]
    fn reply_cursor_tracks_unicode_and_scrolls_back_to_the_start_of_long_input() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let state = AppState::from_refresh(
            Ok(snapshot()),
            Ok(vec![registered(
                PaneAvailability::Present,
                WorkItemKind::Implementation,
            )]),
        );
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        interaction.begin_reply(state.items());
        interaction.draft.as_mut().unwrap().insert("aλ🙂z").unwrap();
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 6); // border + 5 display cells
        interaction
            .draft
            .as_mut()
            .unwrap()
            .edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        interaction
            .draft
            .as_mut()
            .unwrap()
            .edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 3); // border + aλ
        interaction
            .draft
            .as_mut()
            .unwrap()
            .insert(&"x".repeat(100))
            .unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 18);
        for _ in 0..105 {
            interaction
                .draft
                .as_mut()
                .unwrap()
                .edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
                .unwrap();
        }
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 1);
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "ABC-123");
        assert!(terminal.backend().cursor_visible());
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
        assert_eq!(scroll, 2);
        assert!(text.contains("/work/my repo"));
        screen(Some(Ok(Snapshot::default())), 10, 2, &mut scroll);
        assert_eq!(scroll, 1);
        screen(Some(Ok(snapshot())), 1, 1, &mut scroll);
    }
}
