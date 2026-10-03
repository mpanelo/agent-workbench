//! View-local shortcuts and a read-only help screen. No engine operations.
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{theme, ui::View};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Work,
    Attention,
    Sessions { all: bool },
    Review,
    Reply,
    Registration,
    Edit,
    Unregister,
    Cleanup,
}

impl Context {
    pub fn view(view: View, all: bool) -> Self {
        match view {
            View::Work => Self::Work,
            View::Attention => Self::Attention,
            View::Sessions => Self::Sessions { all },
        }
    }

    pub fn text_input(self) -> bool {
        matches!(self, Self::Reply | Self::Registration | Self::Edit)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Work => "WORK",
            Self::Attention => "ATTENTION",
            Self::Sessions { .. } => "SESSIONS",
            Self::Review => "REVIEW",
            Self::Reply => "REPLY",
            Self::Registration => "REGISTER",
            Self::Edit => "EDIT DETAILS",
            Self::Unregister => "UNREGISTER",
            Self::Cleanup => "CLEAN UP",
        }
    }

    // The footer and full help share the same hints, so hidden shortcuts remain
    // accessible without maintaining a second, divergent list.
    pub fn hints(self) -> &'static str {
        match self {
            Self::Work => {
                "Edit: e | Unregister: u | Clean up: c | Views: a/w/s | Quit: q | Select: j/k | Open: <enter> | Reply: r | Review: d | Scroll: <c-d>/<c-u> | Help: ?"
            }
            Self::Attention => {
                "Acknowledge finished turn: x | Views: a/w/s | Quit: q | Select: j/k | Open: <enter> | Reply: r | Review: d | Scroll: <c-d>/<c-u> | Help: ?"
            }
            Self::Sessions { all: false } => {
                "All panes: f | Views: a/w/s | Quit: q | Select: j/k | Register: <enter>/r | Scroll: <c-d>/<c-u> | Help: ?"
            }
            Self::Sessions { all: true } => {
                "Agents only: f | Views: a/w/s | Quit: q | Select: j/k | Register: <enter>/r | Scroll: <c-d>/<c-u> | Help: ?"
            }
            Self::Review => {
                "Select: j/k | Toggle mark: Space | Scroll: <c-d>/<c-u> | Pan: h/l | Reload: r | Full/since: c | Pending: Tab | Back: Esc | Quit: q | Help: ?"
            }
            Self::Reply => {
                "Send: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace | Clear: <c-u> | Move cursor: ←/→"
            }
            Self::Registration => {
                "Field: Tab/Shift-Tab/↑/↓ | Toggle kind: Space | Save: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace | Clear field: <c-u> | Move cursor: ←/→"
            }
            Self::Edit => {
                "Field: Tab/Shift-Tab/↑/↓ | Save: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace | Clear field: <c-u> | Move cursor: ←/→"
            }
            Self::Unregister => "Unregister entry: <enter> | Cancel: Esc/Ctrl-C | Help: ?",
            Self::Cleanup => {
                "Choose: Tab/←/→ | Confirm: <enter> | Cancel: Esc/q/<c-c> | Scroll: <c-d>/<c-u> | Help: ?"
            }
        }
    }

    fn lines(self) -> Vec<Line<'static>> {
        let mut lines: Vec<_> = self
            .hints()
            .split(" | ")
            .map(theme::shortcut_line)
            .collect();
        let extras: &[&str] = match self {
            Self::Work | Self::Attention => &[
                "Select (arrows): ↑/↓",
                "Next attention item: Tab",
                "Attention: a",
                "Work: w",
                "Sessions: s",
                "Quit (alternatives): Esc/<c-c>",
            ],
            Self::Sessions { .. } => &[
                "Select (arrows): ↑/↓",
                "Attention: a",
                "Work: w",
                "Sessions: s",
                "Quit (alternatives): Esc/<c-c>",
            ],
            Self::Review => &[
                "Select (arrows): ↑/↓",
                "Pan (arrows): ←/→",
                "Quit (alternative): <c-c>",
            ],
            _ => &[],
        };
        lines.extend(extras.iter().map(|hint| theme::shortcut_line(hint)));
        if self.text_input() {
            lines.push(Line::styled(
                "? and q type text while editing.",
                theme::muted(),
            ));
        }
        lines.push(Line::styled(
            "Actions may be unavailable while loading or saving.",
            theme::muted(),
        ));
        lines
    }
}

#[derive(Default)]
pub struct HelpUi {
    context: Option<Context>,
    scroll: u16,
}

impl HelpUi {
    pub fn is_open(&self) -> bool {
        self.context.is_some()
    }

    /// Consume all input while open so dismissing help never activates a view
    /// action, submits a draft, quits the app, or pastes into an editor.
    pub fn event(&mut self, event: &Event, context: Context, height: u16) -> bool {
        if let Event::Key(key) = event
            && key.kind != KeyEventKind::Release
        {
            let plain = !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
            let toggle = plain && key.code == KeyCode::Char('?') && !context.text_input();
            if self.is_open() {
                if toggle
                    || plain && matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | '?'))
                    || key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.context = None;
                } else if plain || key.modifiers == KeyModifiers::CONTROL {
                    let page = height.saturating_sub(2).max(1);
                    match key.code {
                        KeyCode::Down | KeyCode::Char('j') if plain => {
                            self.scroll = self.scroll.saturating_add(1)
                        }
                        KeyCode::Up | KeyCode::Char('k') if plain => {
                            self.scroll = self.scroll.saturating_sub(1)
                        }
                        KeyCode::Char('d') if !plain => {
                            self.scroll = self.scroll.saturating_add((page / 2).max(1))
                        }
                        KeyCode::Char('u') if !plain => {
                            self.scroll = self.scroll.saturating_sub((page / 2).max(1))
                        }
                        _ => {}
                    }
                }
                return true;
            }
            if toggle {
                self.context = Some(context);
                self.scroll = 0;
                return true;
            }
        }
        self.is_open()
    }

    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let Some(context) = self.context else {
            return;
        };
        theme::paint(frame);
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(format!("KEYBINDINGS — {}", context.title())).style(theme::header()),
            header,
        );
        let lines = wrapped_lines(context.lines(), body.width);
        let count = lines.len().min(usize::from(u16::MAX)) as u16;
        self.scroll = self.scroll.min(count.saturating_sub(body.height));
        frame.render_widget(
            Paragraph::new(lines)
                .style(theme::text())
                .scroll((self.scroll, 0)),
            body,
        );
        frame.render_widget(
            theme::footer("Close: Esc/q/? | Scroll: j/k/<c-d>/<c-u>"),
            footer,
        );
    }
}

// Explicit display-width wrapping keeps scroll bounds exact, including at
// narrow widths, without enabling Ratatui's unstable line-count API.
pub(crate) fn wrapped_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for line in lines {
        let mut row = Line::default().style(line.style);
        for span in line.spans {
            for ch in span.content.chars() {
                let cell = Span::styled(ch.to_string(), span.style);
                if row.width() + cell.width() > usize::from(width) && row.width() > 0 {
                    result.push(row);
                    row = Line::default().style(line.style);
                }
                row.spans.push(cell);
            }
        }
        result.push(row);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn screen(help: &mut HelpUi, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| help.render(frame)).unwrap();
        if width == 0 {
            return String::new();
        }
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_view_shows_its_own_complete_bindings_including_hidden_footer_hints() {
        for (context, required, excluded) in [
            (Context::Work, "Unregister: u", "Toggle mark: Space"),
            (
                Context::Cleanup,
                "Choose: Tab/←/→",
                "Unregister entry: <enter>",
            ),
            (
                Context::Attention,
                "Acknowledge finished turn: x",
                "Unregister: u",
            ),
            (
                Context::Sessions { all: false },
                "All panes: f",
                "Agents only: f",
            ),
            (
                Context::Sessions { all: true },
                "Agents only: f",
                "All panes: f",
            ),
            (Context::Review, "Back: Esc", "Unregister: u"),
            (Context::Reply, "Send: <enter>", "Save: <enter>"),
            (Context::Registration, "Toggle kind: Space", "Send: <enter>"),
            (Context::Edit, "Clear field: <c-u>", "Toggle kind: Space"),
            (
                Context::Unregister,
                "Unregister entry: <enter>",
                "Clear field: <c-u>",
            ),
        ] {
            let mut help = HelpUi {
                context: Some(context),
                scroll: 0,
            };
            let text = screen(&mut help, 80, 40);
            assert!(text.contains(&format!("KEYBINDINGS — {}", context.title())));
            assert!(text.contains(required), "{text}");
            assert!(!text.contains(excluded), "{text}");
            assert!(!text.contains("Home"));
            assert!(!text.contains("F1"));
            for hint in context.hints().split(" | ") {
                assert!(text.contains(hint), "missing {hint}: {text}");
            }
        }
        assert_eq!(Context::view(View::Work, false), Context::Work);
        assert_eq!(Context::view(View::Attention, false), Context::Attention);
        assert_eq!(
            Context::view(View::Sessions, true),
            Context::Sessions { all: true }
        );
    }

    #[test]
    fn help_consumes_actions_paste_and_dismissal_without_forwarding_them() {
        let context = Context::Work;
        for close in [
            key(KeyCode::Esc),
            key(KeyCode::Char('q')),
            key(KeyCode::Char('?')),
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ] {
            let mut help = HelpUi::default();
            // '?' often arrives with SHIFT.
            assert!(help.event(
                &Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)),
                context,
                24
            ));
            for event in [
                key(KeyCode::Enter),
                key(KeyCode::Char('e')),
                key(KeyCode::Char('u')),
                key(KeyCode::Char('w')),
                key(KeyCode::F(1)),
                Event::Paste("do not insert".into()),
            ] {
                assert!(help.event(&event, context, 24));
                assert!(help.is_open());
            }
            assert!(help.event(&close, context, 24));
            assert!(!help.is_open());
            assert!(!help.event(&key(KeyCode::Char('e')), context, 24));
        }
        let mut help = HelpUi::default();
        let mut release = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(!help.event(&Event::Key(release), context, 24));
        assert!(!help.event(
            &Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::ALT)),
            context,
            24
        ));
    }

    #[test]
    fn question_mark_is_literal_in_text_fields_and_f1_is_unbound() {
        for context in [Context::Reply, Context::Registration, Context::Edit] {
            let mut help = HelpUi::default();
            let mut draft = crate::interaction::Draft {
                item_id: "A".into(),
                text: "Why".into(),
                cursor: crate::interaction::InputCursor::default(),
            };
            let question = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
            assert!(!help.event(&Event::Key(question), context, 24));
            draft.edit(question).unwrap();
            assert_eq!(draft.text, "Why?");
            draft
                .edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
                .unwrap();
            let cursor = draft.cursor;
            let f1 = KeyEvent::from(KeyCode::F(1));
            assert!(!help.event(&Event::Key(f1), context, 24));
            draft.edit(f1).unwrap();
            assert!(!help.is_open());
            assert_eq!(draft.item_id, "A");
            assert_eq!(draft.text, "Why?");
            assert_eq!(draft.cursor, cursor);
        }
        for context in [
            Context::Work,
            Context::Attention,
            Context::Sessions { all: false },
            Context::Review,
            Context::Cleanup,
            Context::Unregister,
        ] {
            let mut help = HelpUi::default();
            assert!(!help.event(&key(KeyCode::F(1)), context, 24));
            assert!(!help.is_open());
        }
    }

    #[test]
    fn narrow_help_wraps_scrolls_clamps_and_resets_without_losing_bindings() {
        let context = Context::Work;
        let mut help = HelpUi::default();
        help.event(&key(KeyCode::Char('?')), context, 8);
        let mut pages = String::new();
        for _ in 0..30 {
            pages.push_str(&screen(&mut help, 20, 8));
            help.event(
                &Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
                context,
                8,
            );
        }
        let compact: String = pages.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(compact.contains("Scroll:<c-d>/<c-u>"));
        assert!(compact.contains("Quit(alternatives):Esc/<c-c>"));
        assert!(help.scroll > 0);
        let before = help.scroll;
        help.event(&key(KeyCode::PageDown), context, 8);
        help.event(&key(KeyCode::PageUp), context, 8);
        assert_eq!(help.scroll, before);
        help.event(
            &Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            context,
            8,
        );
        let before = help.scroll;
        help.event(&key(KeyCode::Home), context, 8);
        assert_eq!(help.scroll, before);
        help.scroll = u16::MAX;
        screen(&mut help, 80, 40);
        assert_eq!(help.scroll, 0);
        for (width, height) in [(1, 1), (0, 0), (5, 3)] {
            screen(&mut help, width, height);
        }
        help.event(&key(KeyCode::Esc), context, 8);
        help.event(&key(KeyCode::Char('?')), Context::Review, 8);
        assert_eq!(help.context, Some(Context::Review));
        assert_eq!(help.scroll, 0);
    }

    #[test]
    fn wrapping_preserves_every_character_and_span_color() {
        let text = "Clear λ🙂 field: <c-u>";
        let lines = wrapped_lines(vec![theme::shortcut_line(text)], 10);
        assert!(lines.iter().all(|line| line.width() <= 10));
        let actual: String = lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(actual, text);
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.content == "<" && span.style.fg == Some(theme::PEACH))
        );
    }
}
