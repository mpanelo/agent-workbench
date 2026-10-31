//! View-local shortcuts and a read-only help screen. No engine operations.
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Work { sessions: bool, unregistered: bool },
    Review,
    Reply,
    Response { changed: bool, can_reject: bool },
    Registration,
    Edit,
    Removal { blocked: bool },
    Cleanup,
}

impl Context {
    pub fn text_input(self) -> bool {
        matches!(self, Self::Reply | Self::Registration | Self::Edit)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Work { .. } => "WORK",
            Self::Review => "REVIEW",
            Self::Reply => "REPLY",
            Self::Response { .. } => "RESPONSE",
            Self::Registration => "REGISTER",
            Self::Edit => "EDIT DETAILS",
            Self::Removal { .. } => "REMOVE",
            Self::Cleanup => "CLEAN UP",
        }
    }

    // The footer and full help share action hints. Familiar navigation keys
    // remain supported without appearing in either list.
    pub fn hints(self) -> &'static str {
        match self {
            Self::Work {
                sessions: false, ..
            } => {
                "Show sessions: Tab | Respond: r | Edit: e | Clean Up: c | Quit: q | Open: <enter> | Review: d | Help: ?"
            }
            Self::Work {
                sessions: true,
                unregistered: false,
            } => {
                "Hide sessions: Tab | Respond: r | Edit: e | Clean Up: c | Quit: q | Open: <enter> | Review: d | Help: ?"
            }
            Self::Work {
                sessions: true,
                unregistered: true,
            } => "Hide sessions: Tab | Register: <enter>/r | Quit: q | Help: ?",
            Self::Review => {
                "Toggle mark: Space | Scroll: <c-d>/<c-u> | Reload: r | Full/since: c | Pending: Tab | Back: Esc | Quit: q | Help: ?"
            }
            Self::Reply => "Send: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace | Clear: <c-u>",
            Self::Response {
                changed: false,
                can_reject: true,
            } => "Approve once: y | Reject: n | Reply: <enter> | Cancel: Esc | Help: ?",
            Self::Response {
                changed: false,
                can_reject: false,
            } => "Approve once: y | Reply: <enter> | Cancel: Esc | Help: ?",
            Self::Response { changed: true, .. } => {
                "Request changed; cancel and reopen. | Cancel: Esc | Help: ?"
            }
            Self::Registration => {
                "Field: Tab/Shift-Tab | Toggle type: Space | Save: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace | Clear field: <c-u>"
            }
            Self::Edit => {
                "Field: Tab/Shift-Tab | Save: <enter> | Cancel: Esc/Ctrl-C | Edit: Backspace | Clear field: <c-u>"
            }
            Self::Removal { blocked: false } => "Unregister: u | Clean: c | Cancel: Esc",
            Self::Removal { blocked: true } => "Cancel: Esc",
            Self::Cleanup => "Choose: Tab | Confirm: <enter> | Cancel: Esc/q/<c-c> | Help: ?",
        }
    }

    fn lines(self) -> Vec<Line<'static>> {
        let mut lines: Vec<_> = self
            .hints()
            .split(" | ")
            .filter(|hint| *hint != "Help: ?")
            .map(theme::shortcut_line)
            .collect();
        let extras: &[&str] = match self {
            Self::Work { .. } => &["Quit (alternatives): Esc/<c-c>"],
            Self::Review => &["Quit (alternative): <c-c>"],
            _ => &[],
        };
        lines.extend(extras.iter().map(|hint| theme::shortcut_line(hint)));
        if matches!(self, Self::Response { .. }) {
            lines.push(Line::styled(
                "Approval shortcuts require a supported, current approval prompt.",
                theme::muted(),
            ));
        }
        if matches!(
            self,
            Self::Response {
                changed: false,
                can_reject: true
            }
        ) {
            lines.push(Line::styled(
                "Reject sends the rejection first, then opens a reply for instructions.",
                theme::muted(),
            ));
        } else if matches!(
            self,
            Self::Response {
                changed: false,
                can_reject: false
            }
        ) {
            lines.push(Line::styled("No supported rejection shortcut for this prompt; inspect the agent pane to reject.", theme::muted()));
        }
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
}

impl HelpUi {
    pub fn is_open(&self) -> bool {
        self.context.is_some()
    }

    /// Consume all input while open so dismissing help never activates a view
    /// action, submits a draft, quits the app, or pastes into an editor.
    pub fn event(&mut self, event: &Event, context: Context) -> bool {
        if let Event::Key(key) = event
            && key.kind != KeyEventKind::Release
        {
            let plain = !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
            let toggle = plain
                && key.code == KeyCode::Char('?')
                && !context.text_input()
                && !matches!(context, Context::Removal { .. });
            if self.is_open() {
                if toggle
                    || plain && matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | '?'))
                    || key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.context = None;
                }
                return true;
            }
            if toggle {
                self.context = Some(context);
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
            Paragraph::new(format!("KEYBINDINGS · {}", context.title())).style(theme::header()),
            header,
        );
        let lines = wrapped_lines(context.lines(), body.width);
        frame.render_widget(Paragraph::new(lines).style(theme::text()), body);
        frame.render_widget(theme::footer("Close: Esc/q/?"), footer);
    }
}

// Explicit display-width wrapping preserves styled text at narrow widths.
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
    fn test_view_bindings() {
        for (context, required, excluded) in [
            (
                Context::Work {
                    sessions: false,
                    unregistered: false,
                },
                "Clean Up: c",
                "Unregister: u",
            ),
            (Context::Cleanup, "Choose: Tab", "Unregister entry: <enter>"),
            (
                Context::Work {
                    sessions: true,
                    unregistered: true,
                },
                "Register: <enter>/r",
                "Review: d",
            ),
            (
                Context::Work {
                    sessions: true,
                    unregistered: false,
                },
                "Hide sessions: Tab",
                "Register: <enter>/r",
            ),
            (Context::Review, "Back: Esc", "Unregister: u"),
            (Context::Reply, "Send: <enter>", "Save: <enter>"),
            (
                Context::Response {
                    changed: false,
                    can_reject: true,
                },
                "Approve once: y",
                "Respond: r",
            ),
            (
                Context::Response {
                    changed: true,
                    can_reject: true,
                },
                "Request changed",
                "Approve once: y",
            ),
            (
                Context::Response {
                    changed: false,
                    can_reject: false,
                },
                "Approve once: y",
                "Reject: n",
            ),
            (Context::Registration, "Toggle type: Space", "Send: <enter>"),
            (Context::Edit, "Clear field: <c-u>", "Toggle type: Space"),
        ] {
            let mut help = HelpUi {
                context: Some(context),
            };
            let text = screen(&mut help, 80, 40);
            assert!(text.contains(&format!("KEYBINDINGS · {}", context.title())));
            assert!(text.contains(required), "{text}");
            assert!(!text.contains(excluded), "{text}");
            assert!(!text.contains("Home"));
            assert!(!text.contains("F1"));
            assert!(!text.contains("legacy alias"));
            assert!(!text.contains("Work: a"));
            assert!(!text.contains("Views:"));
            assert!(!context.hints().contains("Views:"));
            assert!(!context.hints().contains("Acknowledge"));
            assert!(!text.contains("Sessions: s"));
            assert!(!text.contains("Work: w"));
            for hidden in ["j/k", "↑", "↓", "←", "→", "(arrows)"] {
                assert!(!text.contains(hidden), "unexpected {hidden}: {text}");
                assert!(!context.hints().contains(hidden));
            }
            for hint in context.hints().split(" | ") {
                if hint == "Help: ?" {
                    assert!(!text.contains(hint), "unexpected {hint}: {text}");
                } else {
                    assert!(text.contains(hint), "missing {hint}: {text}");
                }
            }
            assert_eq!(
                text.contains("Approve once: y"),
                matches!(context, Context::Response { changed: false, .. })
            );
            assert_eq!(
                text.contains("Reject: n"),
                context
                    == Context::Response {
                        changed: false,
                        can_reject: true
                    }
            );
        }
    }

    #[test]
    fn test_removal_menu() {
        for blocked in [false, true] {
            let mut help = HelpUi::default();
            let context = Context::Removal { blocked };
            assert!(!context.hints().contains("Help"));
            for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
                assert!(!help.event(
                    &Event::Key(KeyEvent::new(KeyCode::Char('?'), modifiers)),
                    context
                ));
                assert!(!help.is_open());
            }
            // Cancellation belongs to the popup, not a hidden help screen.
            assert!(!help.event(&key(KeyCode::Esc), context));
            assert!(help.event(
                &key(KeyCode::Char('?')),
                Context::Work {
                    sessions: false,
                    unregistered: false
                }
            ));
            assert!(help.is_open());
        }
    }

    #[test]
    fn test_modal_input() {
        let context = Context::Work {
            sessions: false,
            unregistered: false,
        };
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
                context
            ));
            for event in [
                key(KeyCode::Enter),
                key(KeyCode::Char('e')),
                key(KeyCode::Char('u')),
                key(KeyCode::Char('y')),
                key(KeyCode::Char('n')),
                key(KeyCode::Char('w')),
                key(KeyCode::F(1)),
                Event::Paste("do not insert".into()),
            ] {
                assert!(help.event(&event, context));
                assert!(help.is_open());
            }
            assert!(help.event(&close, context));
            assert!(!help.is_open());
            assert!(!help.event(&key(KeyCode::Char('e')), context));
        }
        let mut help = HelpUi::default();
        let mut release = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(!help.event(&Event::Key(release), context));
        assert!(!help.event(
            &Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::ALT)),
            context
        ));
    }

    #[test]
    fn test_literal_question() {
        for context in [Context::Reply, Context::Registration, Context::Edit] {
            let mut help = HelpUi::default();
            let mut draft = crate::interaction::Draft {
                item_id: "A".into(),
                text: "Why".into(),
                cursor: crate::interaction::InputCursor::default(),
            };
            let question = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
            assert!(!help.event(&Event::Key(question), context));
            draft.edit(question).unwrap();
            assert_eq!(draft.text, "Why?");
            draft
                .edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
                .unwrap();
            let cursor = draft.cursor;
            let f1 = KeyEvent::from(KeyCode::F(1));
            assert!(!help.event(&Event::Key(f1), context));
            draft.edit(f1).unwrap();
            assert!(!help.is_open());
            assert_eq!(draft.item_id, "A");
            assert_eq!(draft.text, "Why?");
            assert_eq!(draft.cursor, cursor);
        }
        for context in [
            Context::Work {
                sessions: false,
                unregistered: false,
            },
            Context::Work {
                sessions: true,
                unregistered: true,
            },
            Context::Review,
            Context::Cleanup,
        ] {
            let mut help = HelpUi::default();
            assert!(!help.event(&key(KeyCode::F(1)), context));
            assert!(!help.is_open());
        }
    }

    #[test]
    fn test_static_help() {
        for context in [
            Context::Work {
                sessions: false,
                unregistered: false,
            },
            Context::Review,
        ] {
            let mut help = HelpUi::default();
            help.event(&key(KeyCode::Char('?')), context);
            let baseline = screen(&mut help, 40, 8);
            for event in [
                key(KeyCode::Char('j')),
                key(KeyCode::Char('k')),
                key(KeyCode::Down),
                key(KeyCode::Up),
                key(KeyCode::PageDown),
                key(KeyCode::PageUp),
                key(KeyCode::Home),
                Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
                Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            ] {
                assert!(help.event(&event, context));
                assert_eq!(screen(&mut help, 40, 8), baseline);
            }
            assert!(!baseline.lines().last().unwrap().contains("Scroll"));
            let wide = screen(&mut help, 120, 40);
            assert!(wide.contains("Quit (alternatives)") || context == Context::Review);
            for (width, height) in [(1, 1), (0, 0), (5, 3)] {
                screen(&mut help, width, height);
            }
            help.event(&key(KeyCode::Esc), context);
            assert!(!help.is_open());
        }
    }

    #[test]
    fn test_scroll_bindings() {
        for context in [
            Context::Work {
                sessions: false,
                unregistered: false,
            },
            Context::Work {
                sessions: true,
                unregistered: true,
            },
            Context::Work {
                sessions: true,
                unregistered: false,
            },
            Context::Reply,
            Context::Response {
                changed: false,
                can_reject: true,
            },
            Context::Response {
                changed: false,
                can_reject: false,
            },
            Context::Response {
                changed: true,
                can_reject: true,
            },
            Context::Registration,
            Context::Edit,
            Context::Removal { blocked: false },
            Context::Removal { blocked: true },
            Context::Cleanup,
        ] {
            assert!(!context.hints().contains("Scroll"));
            assert!(!context.hints().contains("<c-d>"));
        }
        assert!(Context::Review.hints().contains("Scroll: <c-d>/<c-u>"));
        assert!(Context::Reply.hints().contains("Clear: <c-u>"));
    }

    #[test]
    fn test_styled_wrapping() {
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
