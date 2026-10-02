//! Catppuccin Mocha, presentation-only. No terminal palette or tmux config edits.
//! Palette: https://catppuccin.com/palette/#mocha
use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use workbench_core::{AgentStatus, ReviewStatus, WorkItemKind};

pub const BASE: Color = Color::Rgb(30, 30, 46);
pub const MANTLE: Color = Color::Rgb(24, 24, 37);
// Brighter focus surfaces, borders, and supporting text from the Mocha palette.
pub const SURFACE: Color = Color::Rgb(69, 71, 90);
pub const BORDER: Color = Color::Rgb(127, 132, 156);
pub const TEXT: Color = Color::Rgb(205, 214, 244);
pub const SUBTEXT: Color = Color::Rgb(186, 194, 222);
pub const LAVENDER: Color = Color::Rgb(180, 190, 254);
pub const BLUE: Color = Color::Rgb(137, 180, 250);
pub const MAUVE: Color = Color::Rgb(203, 166, 247);
pub const PINK: Color = Color::Rgb(245, 194, 231);
pub const TEAL: Color = Color::Rgb(148, 226, 213);
pub const SKY: Color = Color::Rgb(137, 220, 235);
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
    Style::default().fg(RED).add_modifier(Modifier::BOLD)
}
pub fn notice() -> Style {
    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
}
pub fn accent() -> Style {
    Style::default().fg(TEAL).add_modifier(Modifier::BOLD)
}
pub fn header() -> Style {
    Style::default()
        .fg(MANTLE)
        .bg(MAUVE)
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
    Style::default().fg(LAVENDER).add_modifier(Modifier::BOLD)
}

pub fn path() -> Style {
    Style::default().fg(SKY)
}

pub fn work_kind(kind: WorkItemKind) -> Style {
    Style::default()
        .fg(match kind {
            WorkItemKind::Implementation => BLUE,
            WorkItemKind::ExternalReview => PINK,
        })
        .add_modifier(Modifier::BOLD)
}

pub fn paint(frame: &mut Frame<'_>) {
    frame.render_widget(Block::default().style(text()), frame.area());
}

pub fn status(status: AgentStatus) -> Style {
    Style::default()
        .fg(match status {
            AgentStatus::Running => SKY,
            AgentStatus::WaitingForInput => YELLOW,
            AgentStatus::Idle => LAVENDER,
            AgentStatus::Complete => GREEN,
            AgentStatus::Unknown => PEACH,
        })
        .add_modifier(Modifier::BOLD)
}

pub fn review_status(status: ReviewStatus) -> Style {
    Style::default()
        .fg(match status {
            ReviewStatus::Reviewed => GREEN,
            ReviewStatus::Unreviewed => BLUE,
            ReviewStatus::ChangedAfterReview => PEACH,
        })
        .add_modifier(Modifier::BOLD)
}

/// Preserve shortcut text/separators exactly; emphasize keys, not descriptions.
pub fn footer(text: &str) -> Paragraph<'static> {
    let lines: Vec<_> = text
        .lines()
        .map(|line| {
            let mut spans = Vec::new();
            for (index, shortcut) in line.split(" | ").enumerate() {
                if index > 0 {
                    spans.push(Span::styled(" | ", Style::default().fg(BORDER)));
                }
                if let Some((key, description)) = shortcut.split_once(':') {
                    spans.push(Span::styled(
                        key.to_owned(),
                        Style::default().fg(PEACH).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(format!(":{description}"), muted()));
                } else {
                    spans.push(Span::styled(
                        shortcut.to_owned(),
                        Style::default().fg(PEACH).add_modifier(Modifier::BOLD),
                    ));
                }
            }
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).style(Style::default().bg(MANTLE))
}

pub fn prompt_line(line: &str) -> Style {
    let line = line.trim_start();
    if line == "Options:" {
        accent()
    } else if line.starts_with("› ") {
        Style::default()
            .fg(PEACH)
            .bg(SURFACE)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with("$ ") {
        Style::default().fg(TEAL).add_modifier(Modifier::BOLD)
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
    fn vivid_roles_keep_focus_types_and_prompt_choices_distinct() {
        assert_eq!(header().fg, Some(MANTLE));
        assert_eq!(header().bg, Some(MAUVE));
        assert_eq!(accent().fg, Some(TEAL));
        assert_eq!(border(true).fg, Some(TEAL));
        assert_eq!(border(false).fg, Some(BORDER));
        assert_eq!(selection().bg, Some(SURFACE));
        assert_eq!(muted().fg, Some(SUBTEXT));
        assert_eq!(work_kind(WorkItemKind::Implementation).fg, Some(BLUE));
        assert_eq!(work_kind(WorkItemKind::ExternalReview).fg, Some(PINK));
        assert_eq!(review_status(ReviewStatus::Unreviewed).fg, Some(BLUE));
        let choice = prompt_line("  › 1. Yes, proceed (y)");
        assert_eq!(choice.fg, Some(PEACH));
        assert_eq!(choice.bg, Some(SURFACE));
        assert!(choice.add_modifier.contains(Modifier::BOLD));
        assert_eq!(prompt_line("  2. No, cancel (esc)").fg, Some(TEXT));
        assert_eq!(prompt_line("  2. No, cancel (esc)").bg, None);
        assert_eq!(prompt_line("Options:").fg, Some(TEAL));
        assert_eq!(prompt_line("$ cargo test").fg, Some(TEAL));
    }

    #[test]
    fn statuses_keep_distinct_semantic_colors_including_unknown() {
        for (state, color) in [
            (AgentStatus::Running, SKY),
            (AgentStatus::WaitingForInput, YELLOW),
            (AgentStatus::Idle, LAVENDER),
            (AgentStatus::Complete, GREEN),
            (AgentStatus::Unknown, PEACH),
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
        let mut terminal = Terminal::new(TestBackend::new(80, 2)).unwrap();
        let text = "j/k: files | Space: reviewed | Esc: back\nq: quit";
        terminal
            .draw(|frame| {
                paint(frame);
                frame.render_widget(footer(text), frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_text_style(buffer, "Space", PEACH, MANTLE);
        assert_text_style(buffer, "reviewed", SUBTEXT, MANTLE);
        assert_text_style(buffer, "q", PEACH, MANTLE);
        assert_text_style(buffer, " | ", BORDER, MANTLE);
        let rendered = buffer
            .content()
            .chunks(80)
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
}
