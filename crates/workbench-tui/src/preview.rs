//! Read-only selected-pane display. Captures never drive agent status or input.
use std::time::{Duration, Instant};

use ansi_to_tui::IntoText;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};
use workbench_core::{PanePreview, WorkItem};

use crate::{theme, ui::visible};

const REFRESH: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub(crate) struct PreviewUi {
    target: Option<WorkItem>,
    generation: u64,
    busy: bool,
    requested: Option<Instant>,
    content: Option<Result<Vec<Line<'static>>, String>>,
}

impl PreviewUi {
    /// Clearing a selection also clears its output, including failed loads and
    /// modal/view changes. A prior capture cannot paint a new work item's pane.
    pub fn select(&mut self, target: Option<&WorkItem>) -> bool {
        if self.target.as_ref() == target {
            return false;
        }
        self.target = target.cloned();
        self.generation = self.generation.wrapping_add(1);
        self.busy = false;
        self.requested = None;
        self.content = None;
        true
    }

    pub fn request(&mut self, now: Instant) -> Option<(u64, WorkItem)> {
        let target = self.target.as_ref()?;
        if self.busy
            || self
                .requested
                .is_some_and(|last| now.saturating_duration_since(last) < REFRESH)
        {
            return None;
        }
        self.busy = true;
        self.requested = Some(now);
        Some((self.generation, target.clone()))
    }

    pub fn finish(&mut self, ticket: u64, result: Result<PanePreview, String>) {
        if ticket != self.generation || self.target.is_none() {
            return;
        }
        self.busy = false;
        self.content = Some(result.map(|capture| decode(&capture.text)));
    }

    pub fn fail(&mut self) {
        self.finish(
            self.generation,
            Err("Preview capture stopped; retrying.".into()),
        );
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, item: &WorkItem) {
        let block = Block::bordered()
            .title(format!("Pane preview: {}", visible(&item.id)))
            .title_bottom(Line::styled("Read-only · recent output", theme::muted()))
            .style(theme::panel())
            .border_style(theme::border(false))
            .title_style(theme::accent());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let content = self
            .content
            .as_ref()
            .filter(|_| self.target.as_ref() == Some(item));
        match content {
            Some(Ok(lines)) if !lines.is_empty() => {
                frame.render_widget(
                    Paragraph::new(crate::wrap::tail(lines, inner.width, inner.height))
                        .style(theme::panel()),
                    inner,
                );
            }
            Some(Ok(_)) => {
                frame.render_widget(
                    Paragraph::new("Pane has no output.").style(theme::muted()),
                    inner,
                );
            }
            Some(Err(error)) => {
                frame.render_widget(
                    Paragraph::new(format!("Preview unavailable: {}", visible(error)))
                        .style(theme::muted())
                        .wrap(Wrap { trim: false }),
                    inner,
                );
            }
            None => {
                frame.render_widget(
                    Paragraph::new("Loading pane preview…").style(theme::muted()),
                    inner,
                );
            }
        }
    }
}

/// Narrow layouts allocate preview space only after measuring the details, so
/// adding a preview cannot push an agent's request out of the existing viewport.
pub(crate) fn areas(area: Rect, rows: Option<usize>) -> (Rect, Option<Rect>) {
    if area.width >= 120 && area.height >= 4 {
        let [details, preview] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Min(0)]).areas(area);
        return (details, Some(preview));
    }
    rows.map_or((area, None), |rows| stacked(area, rows))
}

pub(crate) fn stacked(area: Rect, rows: usize) -> (Rect, Option<Rect>) {
    if let Ok(rows) = u16::try_from(rows)
        && area.height >= rows.saturating_add(8)
    {
        let [details, preview] = Layout::vertical([
            Constraint::Length(rows.saturating_add(2)),
            Constraint::Min(6),
        ])
        .areas(area);
        return (details, Some(preview));
    }
    (area, None)
}

fn decode(content: &str) -> Vec<Line<'static>> {
    let mut lines = content
        .into_text()
        .map(|text| text.lines)
        .unwrap_or_else(|_| {
            content
                .lines()
                .map(|line| Line::from(visible(line)))
                .collect()
        });
    for line in &mut lines {
        let mut column = 0;
        for span in &mut line.spans {
            let mut text = String::new();
            for (index, part) in span.content.split('\t').enumerate() {
                if index > 0 {
                    let spaces = 8 - column % 8;
                    text.extend(std::iter::repeat_n(' ', spaces));
                    column += spaces;
                }
                // Even successful ANSI parsing is not permission to pass through
                // controls. This also protects every parser-error fallback.
                let part = visible(part);
                column += Span::raw(part.as_str()).width();
                text.push_str(&part);
            }
            span.content = text.into();
        }
    }
    while lines
        .last()
        .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
    {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, style::Color};

    fn item(id: &str) -> WorkItem {
        WorkItem {
            id: id.into(),
            title: "Task".into(),
            repository: "/work".into(),
            workspace: "/work/task".into(),
            branch: None,
            kind: workbench_core::WorkItemKind::Implementation,
            pane_id: "%14".into(),
        }
    }

    fn capture(text: &str) -> PanePreview {
        PanePreview {
            text: text.into(),
            width: 100,
            height: 30,
        }
    }

    fn plain(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn screen(ui: &PreviewUi, item: &WorkItem, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui.render(frame, frame.area(), item))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(usize::from(width).max(1))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_selection() {
        let (a, b) = (item("A"), item("B"));
        let mut ui = PreviewUi::default();
        let now = Instant::now();
        assert!(ui.request(now).is_none());
        assert!(ui.select(Some(&a)));
        let (old, requested) = ui.request(now).unwrap();
        assert_eq!(requested, a);
        ui.finish(old, Ok(capture("old output")));
        assert!(screen(&ui, &a, 45, 8).contains("old output"));
        assert!(ui.select(Some(&b)));
        let (current, _) = ui.request(now).unwrap();
        ui.finish(old, Ok(capture("late old output")));
        assert!(ui.content.is_none());
        assert!(ui.busy);
        ui.finish(current, Ok(capture("new output")));
        let text = screen(&ui, &b, 45, 8);
        assert!(text.contains("Pane preview: B"));
        assert!(text.contains("new output"));
        assert!(!text.contains("old output"));
        assert!(!screen(&ui, &a, 45, 8).contains("new output"));
        assert!(!ui.select(Some(&b)));
        let mut edited = b.clone();
        edited.title = "Changed".into();
        assert!(ui.select(Some(&edited)));
        ui.finish(current, Ok(capture("old registration")));
        assert!(ui.content.is_none());
        assert!(ui.select(None));
        ui.finish(current, Ok(capture("removed")));
        assert!(ui.content.is_none());
        assert!(ui.target.is_none());
    }

    #[test]
    fn test_refresh() {
        let mut ui = PreviewUi::default();
        ui.select(Some(&item("A")));
        let now = Instant::now();
        let (ticket, _) = ui.request(now).unwrap();
        assert!(ui.request(now + REFRESH).is_none());
        ui.finish(ticket, Ok(capture("first")));
        assert!(
            ui.request(now + REFRESH - Duration::from_millis(1))
                .is_none()
        );
        assert!(ui.request(now + REFRESH).is_some());
        ui.fail();
        assert!(ui.request(now + REFRESH * 2).is_some());
    }

    #[test]
    fn test_colors() {
        let lines = decode("\x1b[38;2;1;2;3mRGB\x1b[0m \x1b[38;5;42mindexed\x1b[0m\n");
        assert_eq!(plain(&lines), "RGB indexed");
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content == "RGB" && span.style.fg == Some(Color::Rgb(1, 2, 3)))
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content == "indexed" && span.style.fg == Some(Color::Indexed(42)))
        );
        assert_eq!(plain(&decode("界\tX\n\n")), "界      X");
        assert_eq!(plain(&decode(r"literal \t stays")), r"literal \t stays");
    }

    #[test]
    fn test_controls() {
        for source in [
            "\x1b]52;c;secret\x07after",
            "\x1b]0;title\x1b\\after",
            "\x1bPpayload\x1b\\after",
            "\x1b[2Jafter",
            "\x1b[?25lafter",
            "\x1b",
            "\x1b[",
            "bell\x07nul\0backspace\x08",
            "c1\u{9b}52;c;text\u{9c}",
        ] {
            for line in decode(source) {
                for span in line.spans {
                    assert!(
                        !span.content.chars().any(char::is_control),
                        "{source:?}: {:?}",
                        span.content
                    );
                }
            }
        }
        assert_eq!(plain(&decode("\x1b]52;c;secret\x07after")), "after");
        assert_eq!(plain(&decode("a\x07b")), r"a\u{7}b");
    }

    #[test]
    fn test_viewport() {
        let item = item("A");
        let mut ui = PreviewUi::default();
        ui.select(Some(&item));
        let (ticket, _) = ui.request(Instant::now()).unwrap();
        let output = (0..30).map(|i| format!("row {i:02}\n")).collect::<String>() + "\n\n";
        ui.finish(ticket, Ok(capture(&output)));
        let text = screen(&ui, &item, 45, 8);
        assert!(text.contains("row 29"));
        assert!(!text.contains("row 00"));
        assert!(text.contains("Read-only"));
        ui.finish(ticket, Err("pane vanished\x1b".into()));
        let text = screen(&ui, &item, 45, 8);
        assert!(text.contains("Preview unavailable"));
        assert!(!text.contains("row 29"));
        assert!(!text.contains('\x1b'));
        ui.finish(ticket, Ok(capture("\n\n")));
        assert!(screen(&ui, &item, 45, 8).contains("Pane has no output"));
        for (width, height) in [(1, 1), (0, 0), (5, 3)] {
            screen(&ui, &item, width, height);
        }
    }

    #[test]
    fn test_wrapping() {
        let item = item("A");
        let mut ui = PreviewUi::default();
        ui.select(Some(&item));
        let (ticket, _) = ui.request(Instant::now()).unwrap();
        let text = format!("\x1b[38;2;1;2;3mBEGIN{}TAIL\x1b[0m", "x".repeat(80));
        ui.finish(ticket, Ok(capture(&text)));
        let rendered = screen(&ui, &item, 32, 8);
        assert!(rendered.contains("BEGIN"));
        assert!(rendered.contains("TAIL"));
        let mut terminal = Terminal::new(TestBackend::new(32, 8)).unwrap();
        terminal
            .draw(|frame| ui.render(frame, frame.area(), &item))
            .unwrap();
        theme::assert_text_style(
            terminal.backend().buffer(),
            "TAIL",
            Color::Rgb(1, 2, 3),
            theme::MANTLE,
        );
        assert!(!rendered.contains('\x1b'));

        let text = format!("BEGIN{}TAIL\nlast row", "x".repeat(1000));
        ui.finish(ticket, Ok(capture(&text)));
        for width in [32, 52] {
            let rendered = screen(&ui, &item, width, 8);
            assert!(rendered.contains("TAIL"), "{rendered}");
            assert!(rendered.contains("last row"));
            assert!(!rendered.contains("BEGIN"));
        }
    }

    #[test]
    fn test_layout() {
        let wide = Rect::new(0, 3, 160, 30);
        let (details, preview) = areas(wide, Some(12));
        let preview = preview.unwrap();
        assert_eq!(details.y, preview.y);
        assert_eq!(details.right(), preview.x);
        assert_eq!(details.height, preview.height);
        let narrow = Rect::new(0, 3, 80, 30);
        assert_eq!(areas(narrow, None), (narrow, None));
        let (details, preview) = areas(narrow, Some(12));
        let preview = preview.unwrap();
        assert_eq!(details.height, 14);
        assert_eq!(details.bottom(), preview.y);
        assert_eq!(details.width, preview.width);
        for area in [Rect::new(0, 0, 40, 8), Rect::new(0, 0, 0, 0)] {
            assert_eq!(areas(area, Some(12)), (area, None));
        }
    }
}
