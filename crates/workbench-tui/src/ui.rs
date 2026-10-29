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

pub(crate) fn render_work(
    frame: &mut Frame<'_>,
    state: &AppState,
    scroll: &mut u16,
    interaction: &mut Interaction,
) {
    // Details always render from the top; list navigation reveals the selected row.
    *scroll = 0;
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
    let sessions = if interaction.show_sessions {
        "shown"
    } else {
        "hidden"
    };
    let title = format!("AGENT WORKBENCH · WORK • attention: {count} • sessions: {sessions}");
    frame.render_widget(Paragraph::new(title).style(theme::header()), header);
    let mut lines = Vec::new();
    let rows = state.rows(interaction.show_sessions);
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
        Some(Ok(_)) if rows.is_empty() => {
            lines.push(Line::from("No work items registered."));
            lines.push(Line::from(if interaction.show_sessions {
                "No unregistered agent sessions found."
            } else {
                "Press Tab to show sessions and register an agent."
            }));
            if interaction.show_sessions {
                match &state.discovery {
                    None => lines.push(Line::from("Discovering sessions…")),
                    Some(Err(error)) => lines.push(Line::styled(
                        format!(
                            "Discovery unavailable: {}. Retrying automatically.",
                            visible(error)
                        ),
                        theme::error(),
                    )),
                    _ => {}
                }
            }
        }
        Some(Ok(items)) => {
            // Each item always occupies exactly one row. Prompts, errors and
            // metadata live in a separate viewport and cannot move list rows.
            let list_height = rows.len().min(usize::from((body.height / 3).max(1))) as u16;
            let [list, details] =
                Layout::vertical([Constraint::Length(list_height), Constraint::Min(0)]).areas(body);
            if let Some(index) = rows.iter().position(|row| row.selected(interaction)) {
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
                .min(rows.len().saturating_sub(usize::from(list.height)));
            // Measure all row labels, not only visible rows, so navigation
            // and status changes cannot shift the columns. Reserve space for
            // selection, gaps, attention and a feedback glyph. Reserve the same
            // space even without feedback so approval never shifts the columns.
            let available = usize::from(list.width).saturating_sub(10);
            let id_width = rows
                .iter()
                .map(|row| Line::from(visible(row.label())).width())
                .max()
                .unwrap_or(0)
                .min(
                    available
                        .saturating_sub(WORK_STATUS_WIDTH)
                        .max(available / 3),
                );
            let status_width = WORK_STATUS_WIDTH.min(available.saturating_sub(id_width));
            for row in rows
                .iter()
                .skip(interaction.work_list_offset)
                .take(usize::from(list.height))
            {
                let selected = row.selected(interaction);
                let mut heading = vec![Span::styled(
                    format!(
                        "{}{}  ",
                        if selected { "> " } else { "  " },
                        work_cell(&visible(row.label()), id_width),
                    ),
                    theme::accent(),
                )];
                if let crate::listing::Row::Work(entry) = row {
                    let item = &entry.item;
                    let (label, style) = entry.resource_issue().map_or_else(
                        || (entry.status.to_string(), theme::status(entry.status)),
                        |issue| (issue.to_string(), theme::resource_issue(issue)),
                    );
                    heading.push(Span::styled(work_cell(&label, status_width), style));
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
                } else if let crate::listing::Row::Session { session, pane, .. } = row {
                    heading.push(Span::styled(
                        work_cell("UNREGISTERED", status_width),
                        theme::muted(),
                    ));
                    heading.push(Span::styled(
                        format!(
                            "      {} ·  {}",
                            visible(session),
                            visible(pane.current_command.as_deref().unwrap_or("unavailable"))
                        ),
                        theme::muted(),
                    ));
                }
                lines.push(selectable_line(Line::from(heading), selected, list.width));
            }
            frame.render_widget(Paragraph::new(std::mem::take(&mut lines)), list);
            let selected = interaction.selected(items);
            let selected_id = selected.map(|state| state.item.id.clone());
            interaction.detail_item_id = selected_id;
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
            } else if let Some(row) = rows.iter().find(|row| row.selected(interaction)) {
                title.push(Span::raw(format!(" · {}", visible(row.label()))));
            }
            let block = Block::bordered()
                .title(Line::from(title))
                .border_style(if feedback.is_some_and(|feedback| feedback.is_pulsing()) {
                    Style::default().fg(theme::GREEN)
                } else {
                    theme::border(true)
                })
                .title_style(theme::accent());
            let (mut detail_area, mut preview_area) = if selected.is_some() {
                crate::preview::areas(details, None)
            } else {
                (details, None)
            };
            let inner = block.inner(detail_area);
            let mut prompt_start = None;
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
                if state.resource_issue().is_some()
                    && state.pane == workbench_core::PaneAvailability::Present
                {
                    lines.push(metadata(
                        "Agent",
                        &state.status.to_string(),
                        theme::status(state.status),
                    ));
                }
                if state.pane == workbench_core::PaneAvailability::Present
                    && state.status != workbench_core::AgentStatus::Complete
                {
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
                match state.workspace_availability {
                    workbench_core::WorkspaceAvailability::Present => {}
                    workbench_core::WorkspaceAvailability::Missing => {
                        lines.push(Line::styled(
                            "  Workspace directory missing.",
                            theme::notice(),
                        ));
                    }
                    workbench_core::WorkspaceAvailability::Unavailable => {
                        lines.push(Line::styled(
                            "  Workspace inaccessible or not a directory.",
                            theme::notice(),
                        ));
                    }
                }
                if state.resource_issue().is_some() {
                    lines.push(Line::styled(
                        "  Registration kept. Unregister after manual cleanup.",
                        theme::muted(),
                    ));
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
                if state.pane == workbench_core::PaneAvailability::Present
                    && state.status == workbench_core::AgentStatus::WaitingForInput
                {
                    lines.push(Line::from(""));
                    lines.push(Line::styled("  Agent requests input:", theme::notice()));
                    if let Some(prompt) = &state.attention_prompt {
                        prompt_start = Some(lines.len());
                        push_prompt(&mut lines, prompt, inner.width);
                    } else {
                        lines.push(Line::from(format!("  {}", visible(&state.status_detail))));
                    }
                }
            } else if let Some(crate::listing::Row::Session {
                session,
                window,
                pane,
            }) = rows.iter().find(|row| row.selected(interaction))
            {
                lines.push(Line::styled("Unregistered agent session", theme::muted()));
                lines.push(metadata(" Session", session, theme::accent()));
                lines.push(metadata(" Window", window_label(window), theme::accent()));
                let command = pane.current_command.as_deref().unwrap_or("unavailable");
                lines.push(metadata(" Agent", command, theme::text()));
                let title = session_pane_title(&pane.title, command, session, window);
                if !title.is_empty() {
                    lines.push(metadata("Title", title, theme::text()));
                }
                lines.push(metadata(
                    "Directory",
                    &pane
                        .working_directory
                        .as_ref()
                        .map(|path| display_path(path))
                        .unwrap_or_else(|| "unavailable".into()),
                    theme::path(),
                ));
            } else {
                lines.push(Line::from("Select a work item to inspect its details."));
            }
            if let Some(start) = prompt_start
                && preview_area.is_some()
                && lines.len() > usize::from(inner.height)
                && let Some(prompt) = selected.and_then(|state| state.attention_prompt.as_deref())
            {
                // A preview must not hide a request that fits at full width.
                lines.truncate(start);
                push_prompt(&mut lines, prompt, block.inner(details).width);
                (detail_area, preview_area) = crate::preview::stacked(details, lines.len());
            } else if selected.is_some() {
                (detail_area, preview_area) = crate::preview::areas(details, Some(lines.len()));
            }
            let inner = block.inner(detail_area);
            frame.render_widget(block, detail_area);
            frame.render_widget(Paragraph::new(std::mem::take(&mut lines)), inner);
            if let Some(area) = preview_area
                && let Some(state) = selected
            {
                if state.pane == workbench_core::PaneAvailability::Present {
                    interaction.preview.render(frame, area, &state.item);
                } else {
                    frame.render_widget(
                        Paragraph::new("Pane preview unavailable.")
                            .style(theme::muted())
                            .block(
                                Block::bordered()
                                    .title("Pane preview")
                                    .border_style(theme::border(false)),
                            ),
                        area,
                    );
                }
            }
        }
    }
    if !lines.is_empty() {
        frame.render_widget(Paragraph::new(lines), body);
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
            interaction.help_context().hints()
        }),
        footer,
    );
}

// Keep attention in a fixed column even when no current item has the longest
// status label. This is presentation sizing, not an agent-state decision.
const WORK_STATUS_WIDTH: usize = 17; // Agent and resource labels share the column.

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
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{
        AgentStatus, Pane, PaneAvailability, Session, Snapshot, Window, WorkItem, WorkItemKind,
        WorkItemState,
    };

    use super::*;

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
            workspace_availability: workbench_core::WorkspaceAvailability::Present,
            status_detail: "Unsupported foreground command.".into(),
            attention_prompt: None,
            completion_fingerprint: None,
        }
    }

    #[test]
    fn test_home_paths() {
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
    fn test_footer() {
        let state = AppState::default();
        let text = work_screen(&state, 80, 10, &mut 0);
        assert_eq!(
            text.lines().last().unwrap().trim_end(),
            "Respond: r | Edit: e | Clean Up: c | Acknowledge finished turn: x | … | Help: ?"
        );
        assert_eq!(text.lines().nth(8).unwrap().trim_end(), "");
        let text = work_screen(&state, 200, 10, &mut 0);
        let footer = text.lines().last().unwrap();
        assert!(footer.contains("Show sessions: Tab"));
        assert!(!footer.contains("Work: w"));
        assert!(!footer.contains("Views:"));
    }

    #[test]
    fn test_work_details() {
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
    fn test_static_prompt() {
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

        let wide = work_screen(&state, 120, 35, &mut 0);
        let compact: String = wide
            .chars()
            .filter(|ch| !ch.is_whitespace() && *ch != '│')
            .collect();
        assert!(compact.contains("END-OF-REASON"), "{wide}");
        assert!(
            compact.contains("No,andtellCodexwhattododifferently"),
            "{wide}"
        );

        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut interaction = Interaction::default();
        interaction.sync(&state.attention);
        let mut scroll = 0;
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        let baseline = terminal.backend().buffer().clone();
        for code in ['d', 'u'] {
            crate::navigate(
                crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(code),
                    crossterm::event::KeyModifiers::CONTROL,
                ),
                &state,
                &mut scroll,
                &mut interaction,
            );
            terminal
                .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
                .unwrap();
            assert_eq!(terminal.backend().buffer(), &baseline);
        }
        assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
    }

    #[test]
    fn test_selected_prompt() {
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
        state.move_row(&mut interaction, 1);
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
    fn test_work_preview() {
        let mut waiting = registered(PaneAvailability::Present, WorkItemKind::Implementation);
        waiting.status = AgentStatus::WaitingForInput;
        waiting.attention_prompt =
            Some("Choose a test strategy?\n\nOptions:\n1. Unit tests\n2. Integration tests".into());
        let mut state = AppState::from_refresh(Ok(snapshot()), Ok(vec![waiting]));
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        let item = state.items()[0].item.clone();
        interaction.preview.select(Some(&item));
        let (ticket, _) = interaction.preview.target().unwrap();
        interaction.preview.finish(
            ticket,
            Ok(workbench_core::PanePreview {
                text: "\x1b[38;2;1;2;3mLatest agent output\x1b[0m\n".into(),
                width: 100,
                height: 30,
            }),
        );
        for (width, height) in [(160, 35), (80, 35)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
                .unwrap();
            let buffer = terminal.backend().buffer();
            theme::assert_text_style(
                buffer,
                "Latest agent output",
                ratatui::style::Color::Rgb(1, 2, 3),
                theme::MANTLE,
            );
            let rows: Vec<String> = buffer
                .content()
                .chunks(usize::from(width))
                .map(|row| row.iter().map(|cell| cell.symbol()).collect())
                .collect();
            let details = rows.iter().position(|row| row.contains("Details")).unwrap();
            let preview = rows
                .iter()
                .position(|row| row.contains("Pane preview: ABC-123"))
                .unwrap();
            if width >= 120 {
                assert_eq!(details, preview);
            } else {
                assert!(preview > details);
            }
            for expected in [
                "Choose a test strategy?",
                "Unit tests",
                "Integration tests",
                "Read-only",
            ] {
                assert!(rows.iter().any(|row| row.contains(expected)), "{rows:?}");
            }
            assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let row = terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .position(|row| {
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .starts_with("> ABC-123")
            })
            .unwrap();
        state.work_items.as_mut().unwrap().as_mut().unwrap()[0].status = AgentStatus::Complete;
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
        let updated = terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .position(|row| {
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .starts_with("> ABC-123")
            })
            .unwrap();
        assert_eq!(row, updated);
        assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
        assert_eq!(interaction.work_list_offset, 0);
        assert_eq!(state.items()[0].status, AgentStatus::Complete);
    }

    #[test]
    fn test_metadata_colors() {
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
            assert_eq!(buffer[(45, 2)].bg, theme::BASE);
            assert_eq!(buffer[(119, 2)].bg, theme::MANTLE);
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
    fn test_response_bar() {
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
    fn test_changed_response() {
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
    fn test_approval_feedback() {
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
    fn test_narrow_feedback() {
        let (mut state, request) = approval_fixture();
        let mut interaction = Interaction::default();
        interaction.sync(state.items());
        interaction.begin_approval(&request);
        interaction.finish_approval(&request, Ok(()));
        state.move_row(&mut interaction, 1);
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
        state.move_row(&mut interaction, -1);
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
    fn test_status_colors() {
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
    fn test_work_states() {
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
            "WORK • attention: 1",
            "Discovery unavailable",
            "ABC-123  UNAVAILABLE",
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
    fn test_prompt_wrapping() {
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
    fn test_small_work_view() {
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
        state.move_row(&mut interaction, 2);
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
    fn test_status_details() {
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
    fn test_finished_row() {
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
                "Respond: r | Edit: e | Clean Up: c | Acknowledge finished turn: x | … | Help: ?"
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
    fn test_work_metadata() {
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
                    "Fix retries",
                    "/work/repo",
                    "/work/ABC-123",
                    "Branch: fix/retries",
                    work_kind_label(kind),
                ] {
                    assert!(text.contains(expected), "missing {expected:?} in {text}");
                }
                let label = match pane {
                    PaneAvailability::Present => "UNKNOWN",
                    PaneAvailability::Missing => "PANE MISSING",
                    PaneAvailability::Unavailable => "UNAVAILABLE",
                };
                assert!(text.contains(&format!("ABC-123  {label}")), "{text}");
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
    fn test_resource_warnings() {
        use workbench_core::WorkspaceAvailability;
        for (pane, workspace, label) in [
            (
                PaneAvailability::Present,
                WorkspaceAvailability::Missing,
                "WORKSPACE MISSING",
            ),
            (
                PaneAvailability::Missing,
                WorkspaceAvailability::Present,
                "PANE MISSING",
            ),
            (
                PaneAvailability::Missing,
                WorkspaceAvailability::Missing,
                "RESOURCES MISSING",
            ),
            (
                PaneAvailability::Unavailable,
                WorkspaceAvailability::Present,
                "UNAVAILABLE",
            ),
            (
                PaneAvailability::Present,
                WorkspaceAvailability::Unavailable,
                "UNAVAILABLE",
            ),
        ] {
            for status in [
                AgentStatus::Running,
                AgentStatus::WaitingForInput,
                AgentStatus::Complete,
            ] {
                let mut item = registered(pane, WorkItemKind::Implementation);
                item.workspace_availability = workspace;
                item.status = status;
                item.attention_prompt = (status == AgentStatus::WaitingForInput
                    && pane == PaneAvailability::Present)
                    .then(|| {
                        "Please choose how to proceed.\nOptions:\n1. Keep working\n2. Stop".into()
                    });
                let state = AppState::from_refresh(Ok(snapshot()), Ok(vec![item.clone()]));
                assert_eq!(state.attention, [item.clone()]);
                let mut interaction = Interaction {
                    selected_id: Some(item.item.id.clone()),
                    ..Interaction::default()
                };
                let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
                terminal
                    .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let rows: Vec<String> = buffer
                    .content()
                    .chunks(100)
                    .map(|row| row.iter().map(|cell| cell.symbol()).collect())
                    .collect();
                let text = rows.join("\n");
                assert!(
                    rows[1].starts_with(&format!("> ABC-123  {label}")),
                    "{text}"
                );
                assert_eq!(rows[1].find('!'), Some(30));
                assert!(!rows[1].contains("Build"));
                theme::assert_text_style(
                    buffer,
                    label,
                    if label == "UNAVAILABLE" {
                        theme::YELLOW
                    } else {
                        theme::PEACH
                    },
                    theme::SURFACE,
                );
                assert!(text.contains("Registration kept."));
                assert!(!text.contains("%14"));
                if pane == PaneAvailability::Present {
                    assert!(text.contains(&format!("Agent: {status}")), "{text}");
                    if status == AgentStatus::WaitingForInput {
                        assert!(text.contains("Please choose how to proceed."));
                        assert!(text.contains("2. Stop"));
                    }
                } else {
                    assert!(!text.contains("Agent: "), "{text}");
                }
                assert_eq!(state.items(), [item]);
                work_screen(&state, 1, 1, &mut 0);
                work_screen(&state, 30, 6, &mut 0);
            }
        }
    }

    #[test]
    fn test_discovery_error() {
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
        assert!(text.contains("ABC-123  UNAVAILABLE"));
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
    fn test_small_states() {
        assert!(work_screen(&AppState::default(), 80, 10, &mut 0).contains("Loading work items"));
        let state = AppState {
            work_items: Some(Ok(Vec::new())),
            ..AppState::default()
        };
        let mut scroll = 100;
        let text = work_screen(&state, 100, 10, &mut scroll);
        assert!(text.contains("No work items registered"));
        assert!(text.contains("Tab to show sessions"));
        assert_eq!(scroll, 0);
        work_screen(&state, 1, 1, &mut scroll);
    }

    #[test]
    fn test_selected_row() {
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
        state.move_row(&mut interaction, 2);
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
    fn test_stable_rows() {
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
        for (status, pane, workspace) in [
            (
                AgentStatus::Running,
                PaneAvailability::Present,
                workbench_core::WorkspaceAvailability::Present,
            ),
            (
                AgentStatus::WaitingForInput,
                PaneAvailability::Present,
                workbench_core::WorkspaceAvailability::Present,
            ),
            (
                AgentStatus::Complete,
                PaneAvailability::Present,
                workbench_core::WorkspaceAvailability::Present,
            ),
            (
                AgentStatus::Unknown,
                PaneAvailability::Present,
                workbench_core::WorkspaceAvailability::Present,
            ),
            (
                AgentStatus::Running,
                PaneAvailability::Present,
                workbench_core::WorkspaceAvailability::Missing,
            ),
            (
                AgentStatus::Unknown,
                PaneAvailability::Missing,
                workbench_core::WorkspaceAvailability::Present,
            ),
            (
                AgentStatus::Unknown,
                PaneAvailability::Missing,
                workbench_core::WorkspaceAvailability::Missing,
            ),
            (
                AgentStatus::Unknown,
                PaneAvailability::Unavailable,
                workbench_core::WorkspaceAvailability::Unavailable,
            ),
            (
                AgentStatus::Running,
                PaneAvailability::Present,
                workbench_core::WorkspaceAvailability::Present,
            ),
        ] {
            if let Some(Ok(items)) = &mut state.work_items {
                items[0].status = AgentStatus::WaitingForInput;
                items[0].attention_prompt = Some("UNSELECTED PROMPT".repeat(500));
                items[5].status = status;
                items[5].pane = pane;
                items[5].workspace_availability = workspace;
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
    fn test_work_columns() {
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
    fn test_cell_width() {
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
        for issue in [
            workbench_core::ResourceIssue::PaneMissing,
            workbench_core::ResourceIssue::WorkspaceMissing,
            workbench_core::ResourceIssue::ResourcesMissing,
            workbench_core::ResourceIssue::Unavailable,
        ] {
            assert!(Line::from(issue.to_string()).width() <= WORK_STATUS_WIDTH);
        }
    }

    #[test]
    fn test_long_ids() {
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
    fn test_static_details() {
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
        assert_eq!(scroll, 0);
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("> B  WAITING_FOR_INPUT"));
        assert!(text.contains("Details — B"));
        assert!(text.contains("Type: Build"));
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
        assert_eq!(scroll, 0);
        state.move_row(&mut interaction, -1);
        terminal
            .draw(|frame| render_work(frame, &state, &mut scroll, &mut interaction))
            .unwrap();
        assert_eq!(scroll, 0);
        assert_eq!(interaction.selected_id.as_deref(), Some("A"));
        assert_eq!(interaction.detail_item_id.as_deref(), Some("A"));
    }

    #[test]
    fn test_notice_expiry() {
        let state = AppState::from_refresh(
            Ok(snapshot()),
            Ok(vec![registered(
                PaneAvailability::Present,
                WorkItemKind::Implementation,
            )]),
        );
        let now = std::time::Instant::now();
        {
            for (width, height) in [(140, 30), (40, 12), (1, 1)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let mut interaction = Interaction::default();
                state.sync_selection(&mut interaction);
                let mut draw = |interaction: &mut Interaction| {
                    terminal
                        .draw(|frame| render_work(frame, &state, &mut 0, interaction))
                        .unwrap();
                    terminal.backend().buffer().clone()
                };
                let baseline = draw(&mut interaction);
                interaction.message = Some(crate::interaction::Message::confirmation(
                    "Reply submitted to café.\x1b[2J",
                    now,
                ));
                let notice = draw(&mut interaction);
                assert!(
                    notice
                        .content()
                        .iter()
                        .all(|cell| { !cell.symbol().chars().any(char::is_control) })
                );
                if width >= 40 {
                    let text: String = notice.content().iter().map(|cell| cell.symbol()).collect();
                    assert!(
                        text.contains("Reply submitted to café.\\u{1b}[2J"),
                        "{text}"
                    );
                    assert_ne!(notice, baseline);
                }
                assert!(interaction.tick_message(now + std::time::Duration::from_secs(3)));
                assert_eq!(draw(&mut interaction), baseline);
                assert_eq!(interaction.selected_id.as_deref(), Some("ABC-123"));
            }
        }
    }

    #[test]
    fn test_acknowledged_row() {
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
    fn test_reply_editor() {
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
        assert!(
            text.contains("Send: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace | Clear: <c-u>")
        );
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();
        terminal
            .draw(|frame| render_work(frame, &state, &mut 0, &mut interaction))
            .unwrap();
    }

    #[test]
    fn test_reply_cursor() {
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
    fn test_pane_titles() {
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
    }
}
