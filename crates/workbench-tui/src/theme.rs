//! Catppuccin Mocha, presentation-only. No terminal palette or tmux config edits.
//! Palette: https://catppuccin.com/palette/#mocha
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use workbench_core::{AgentStatus, ReviewStatus, WorkItemKind};

pub const BASE: Color = Color::Rgb(30, 30, 46);
pub const MANTLE: Color = Color::Rgb(24, 24, 37);
// Subtle Surface 0 selection, with brighter Overlay 1/Subtext 1 supporting text.
pub const SURFACE: Color = Color::Rgb(49, 50, 68);
pub const BORDER: Color = Color::Rgb(127, 132, 156);
pub const TEXT: Color = Color::Rgb(205, 214, 244);
pub const SUBTEXT: Color = Color::Rgb(186, 194, 222);
pub const LAVENDER: Color = Color::Rgb(180, 190, 254);
pub const BLUE: Color = Color::Rgb(137, 180, 250);
pub const MAUVE: Color = Color::Rgb(203, 166, 247);
pub const PINK: Color = Color::Rgb(245, 194, 231);
pub const TEAL: Color = Color::Rgb(148, 226, 213);
pub const GREEN: Color = Color::Rgb(166, 227, 161);
pub const YELLOW: Color = Color::Rgb(249, 226, 175);
pub const PEACH: Color = Color::Rgb(250, 179, 135);
pub const RED: Color = Color::Rgb(243, 139, 168);

pub fn text() -> Style {
    Style::default().fg(TEXT).bg(BASE)
}
pub fn muted() -> Style {
    Style::default().fg(SUBTEXT)
}
pub fn error() -> Style {
    Style::default().fg(RED)
}
pub fn notice() -> Style {
    Style::default().fg(YELLOW)
}
pub fn accent() -> Style {
    Style::default().fg(TEAL).add_modifier(Modifier::BOLD)
}
pub fn header() -> Style {
    Style::default()
        .fg(LAVENDER)
        .bg(MANTLE)
        .add_modifier(Modifier::BOLD)
}
pub fn selection() -> Style {
    Style::default().bg(SURFACE).add_modifier(Modifier::BOLD)
}
pub fn border(active: bool) -> Style {
    Style::default().fg(if active { TEAL } else { BORDER })
}
pub fn panel() -> Style {
    Style::default().fg(TEXT).bg(MANTLE)
}

pub fn label() -> Style {
    muted()
}

pub fn path() -> Style {
    Style::default().fg(TEXT)
}

pub fn work_kind(kind: WorkItemKind) -> Style {
    Style::default().fg(match kind {
        WorkItemKind::Implementation => BLUE,
        WorkItemKind::ExternalReview => PINK,
    })
}

pub fn paint(frame: &mut Frame<'_>) {
    frame.render_widget(Block::default().style(text()), frame.area());
}

pub fn status(status: AgentStatus) -> Style {
    Style::default()
        .fg(match status {
            AgentStatus::Running => BLUE,
            AgentStatus::WaitingForInput => YELLOW,
            AgentStatus::Idle => TEAL,
            AgentStatus::Complete => GREEN,
            AgentStatus::Unknown => SUBTEXT,
        })
        .add_modifier(Modifier::BOLD)
}

pub fn review_status(status: ReviewStatus) -> Style {
    Style::default().fg(match status {
        ReviewStatus::Reviewed => GREEN,
        ReviewStatus::Unreviewed => SUBTEXT,
        ReviewStatus::ChangedAfterReview => PEACH,
    })
}

/// Action-first hints, with keys accented independently of action labels.
pub fn shortcut_line(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, shortcut) in text.split(" | ").enumerate() {
        if index > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(BORDER)));
        }
        if let Some((action, key)) = shortcut.split_once(':') {
            spans.push(Span::styled(format!("{action}:"), muted()));
            spans.push(Span::styled(
                key.to_owned(),
                Style::default().fg(PEACH).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(shortcut.to_owned(), muted()));
        }
    }
    Line::from(spans)
}

pub fn footer(text: &str) -> Footer {
    Footer(text.lines().collect::<Vec<_>>().join(" | "))
}

pub struct Footer(String);

impl Widget for Footer {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let line = compact_footer(&self.0, usize::from(area.width));
        Paragraph::new(shortcut_line(&line))
            .style(Style::default().bg(MANTLE))
            .render(area, buffer);
    }
}

/// Hide whole hints, not half a key binding. Keep help discoverable on overflow.
fn compact_footer(text: &str, width: usize) -> String {
    if Line::from(text).width() <= width {
        return text.to_owned();
    }
    let hints: Vec<_> = text.split(" | ").collect();
    let help = hints
        .last()
        .copied()
        .filter(|hint| hint.starts_with("Help:"));
    let suffix = help.map_or_else(|| "…".to_owned(), |hint| format!("… | {hint}"));
    if Line::from(suffix.as_str()).width() > width {
        return help
            .filter(|hint| Line::from(*hint).width() <= width)
            .unwrap_or(if width == 0 { "" } else { "…" })
            .to_owned();
    }
    let mut shown = String::new();
    for hint in hints.iter().take(hints.len() - usize::from(help.is_some())) {
        let next = if shown.is_empty() {
            (*hint).to_owned()
        } else {
            format!("{shown} | {hint}")
        };
        if Line::from(format!("{next} | {suffix}")).width() > width {
            break;
        }
        shown = next;
    }
    if shown.is_empty() {
        suffix
    } else {
        format!("{shown} | {suffix}")
    }
}

pub fn prompt_line(line: &str) -> Style {
    let line = line.trim_start();
    if line == "Options:" {
        accent()
    } else if line.starts_with("› ") {
        Style::default().fg(MAUVE).add_modifier(Modifier::BOLD)
    } else if line.starts_with("$ ") {
        Style::default().fg(TEAL)
    } else if line.starts_with("Reason:") || line.starts_with("Environment:") {
        Style::default().fg(PEACH)
    } else {
        Style::default().fg(TEXT)
    }
}

// Terminal colors have no alpha channel: blend 20% diff accent into Mantle.
fn diff_background(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            ((24 * 4 + u16::from(r)) / 5) as u8,
            ((24 * 4 + u16::from(g)) / 5) as u8,
            ((37 * 4 + u16::from(b)) / 5) as u8,
        ),
        _ => MANTLE,
    }
}

pub fn diff_line(line: &str, in_hunk: bool) -> Style {
    if line.starts_with("@@") {
        Style::default().fg(PEACH).add_modifier(Modifier::BOLD)
    } else if !in_hunk && (line.starts_with("--- ") || line.starts_with("+++ ")) {
        Style::default().fg(PINK)
    } else if in_hunk && line.starts_with('+') {
        Style::default().fg(GREEN).bg(diff_background(GREEN))
    } else if in_hunk && line.starts_with('-') {
        Style::default().fg(RED).bg(diff_background(RED))
    } else if line.starts_with("diff ") {
        Style::default().fg(BLUE).add_modifier(Modifier::BOLD)
    } else if !in_hunk {
        muted()
    } else {
        Style::default().fg(TEXT)
    }
}

#[cfg(test)]
pub fn assert_text_style(buffer: &ratatui::buffer::Buffer, text: &str, fg: Color, bg: Color) {
    for row in buffer
        .content()
        .chunks(usize::from(buffer.area.width).max(1))
    {
        let rendered: String = row.iter().map(|cell| cell.symbol()).collect();
        if let Some(offset) = rendered.find(text) {
            let column = Line::from(&rendered[..offset]).width();
            let cell = &row[column];
            assert_eq!(cell.fg, fg, "foreground for {text}");
            assert_eq!(cell.bg, bg, "background for {text}");
            return;
        }
    }
    panic!("missing styled text: {text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_roles_keep_dark_chrome_and_restrain_supporting_text() {
        assert_eq!(header().fg, Some(LAVENDER));
        assert_eq!(header().bg, Some(MANTLE));
        assert_eq!(accent().fg, Some(TEAL));
        assert_eq!(border(true).fg, Some(TEAL));
        assert_eq!(border(false).fg, Some(BORDER));
        assert_eq!(selection().bg, Some(SURFACE));
        assert_eq!(muted().fg, Some(SUBTEXT));
        assert_eq!(work_kind(WorkItemKind::Implementation).fg, Some(BLUE));
        assert_eq!(work_kind(WorkItemKind::ExternalReview).fg, Some(PINK));
        assert_eq!(review_status(ReviewStatus::Unreviewed).fg, Some(SUBTEXT));
        assert_eq!(label().fg, Some(SUBTEXT));
        assert_eq!(path().fg, Some(TEXT));
        for style in [
            label(),
            path(),
            work_kind(WorkItemKind::Implementation),
            work_kind(WorkItemKind::ExternalReview),
            review_status(ReviewStatus::Unreviewed),
            prompt_line("$ cargo test"),
        ] {
            assert!(!style.add_modifier.contains(Modifier::BOLD));
            assert_eq!(style.bg, None);
        }
        let choice = prompt_line("  › 1. Yes, proceed (y)");
        assert_eq!(choice.fg, Some(MAUVE));
        assert_eq!(choice.bg, None);
        assert!(choice.add_modifier.contains(Modifier::BOLD));
        assert_eq!(prompt_line("  2. No, cancel (esc)").fg, Some(TEXT));
        assert_eq!(prompt_line("  2. No, cancel (esc)").bg, None);
        assert_eq!(prompt_line("Options:").fg, Some(TEAL));
        assert_eq!(prompt_line("$ cargo test").fg, Some(TEAL));
    }

    #[test]
    fn statuses_keep_distinct_semantic_colors_including_unknown() {
        for (state, color) in [
            (AgentStatus::Running, BLUE),
            (AgentStatus::WaitingForInput, YELLOW),
            (AgentStatus::Idle, TEAL),
            (AgentStatus::Complete, GREEN),
            (AgentStatus::Unknown, SUBTEXT),
        ] {
            assert_eq!(status(state).fg, Some(color));
        }
        assert_eq!(review_status(ReviewStatus::Reviewed).fg, Some(GREEN));
        assert_eq!(
            review_status(ReviewStatus::ChangedAfterReview).fg,
            Some(PEACH)
        );
    }

    #[test]
    fn diff_styling_distinguishes_metadata_from_header_like_hunk_content() {
        assert_eq!(diff_line("+++ b/file", false).fg, Some(PINK));
        assert_eq!(diff_line("--- a/file", false).fg, Some(PINK));
        assert_eq!(diff_line("+++ actual content", true).fg, Some(GREEN));
        assert_eq!(diff_line("--- actual content", true).fg, Some(RED));
        assert_eq!(diff_line("@@ -1 +1 @@", false).fg, Some(PEACH));
        assert_eq!(
            diff_line("+inserted", true).bg,
            Some(Color::Rgb(52, 64, 61))
        );
        assert_eq!(diff_line("-removed", true).bg, Some(Color::Rgb(67, 47, 63)));
    }

    #[test]
    fn footer_preserves_shortcut_text_and_uses_surface_and_key_colors() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(120, 1)).unwrap();
        let text = "Open: <enter> | Scroll: <c-d>/<c-u> | Toggle mark: Space | Quit: q | Saving… | q types text";
        terminal
            .draw(|frame| {
                paint(frame);
                frame.render_widget(footer(text), frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_text_style(buffer, "Space", PEACH, MANTLE);
        assert_text_style(buffer, "Open:", SUBTEXT, MANTLE);
        assert_text_style(buffer, "Scroll:", SUBTEXT, MANTLE);
        assert_text_style(buffer, "Toggle mark:", SUBTEXT, MANTLE);
        assert_text_style(buffer, "Quit:", SUBTEXT, MANTLE);
        assert_text_style(buffer, "Saving…", SUBTEXT, MANTLE);
        assert_text_style(buffer, "q types text", SUBTEXT, MANTLE);
        assert_text_style(buffer, "q", PEACH, MANTLE);
        assert_text_style(buffer, " | ", BORDER, MANTLE);
        assert_text_style(buffer, "<enter>", PEACH, MANTLE);
        assert_text_style(buffer, "<c-d>/<c-u>", PEACH, MANTLE);
        let rendered = buffer
            .content()
            .chunks(120)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(rendered, text);
    }

    #[test]
    fn footer_truncates_whole_hints_preserves_help_and_measures_display_width() {
        let text = "Edit: e | Unregister: u | Quit: q | Help: ?";
        assert_eq!(compact_footer(text, 80), text);
        assert_eq!(compact_footer(text, Line::from(text).width()), text);
        assert_eq!(compact_footer(text, 30), "Edit: e | … | Help: ?");
        assert_eq!(compact_footer(text, 12), "… | Help: ?");
        assert_eq!(compact_footer(text, 8), "Help: ?");
        assert_eq!(compact_footer(text, 1), "…");
        assert_eq!(compact_footer(text, 0), "");
        assert_eq!(
            compact_footer("界: 🙂 | Clear: <c-u> | Help: ?", 20),
            "界: 🙂 | … | Help: ?"
        );
        assert_eq!(
            compact_footer("Busy | Very long status message", 8),
            "Busy | …"
        );
        for width in 0..100 {
            assert!(Line::from(compact_footer(text, width)).width() <= width);
        }
    }

    #[test]
    fn compact_footer_renders_on_one_row_with_styled_help_and_no_wrapping() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(30, 2)).unwrap();
        terminal
            .draw(|frame| {
                paint(frame);
                frame.render_widget(
                    footer("Edit: e\nUnregister: u | Quit: q | Help: ?"),
                    frame.area(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_text_style(buffer, "Edit:", SUBTEXT, MANTLE);
        assert_text_style(buffer, "Help:", SUBTEXT, MANTLE);
        assert_text_style(buffer, "?", PEACH, MANTLE);
        assert!(
            buffer.content()[30..]
                .iter()
                .all(|cell| cell.symbol() == " ")
        );
    }

    #[test]
    fn view_footers_restore_all_hints_when_resized_wider() {
        use crate::help::Context;
        for context in [
            Context::Work,
            Context::Attention,
            Context::Sessions { all: false },
            Context::Sessions { all: true },
            Context::Review,
            Context::Reply,
            Context::Registration,
            Context::Edit,
            Context::Unregister,
        ] {
            let text = context.hints();
            let narrow = compact_footer(text, 40);
            assert!(narrow.contains('…'));
            assert!(narrow.ends_with(if context.text_input() {
                "Help: F1"
            } else {
                "Help: ?"
            }));
            assert_eq!(compact_footer(text, 240), text);
            for width in 0..240 {
                assert!(Line::from(compact_footer(text, width)).width() <= width);
            }
        }
    }
}
