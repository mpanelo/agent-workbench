//! Read-only action picker. Existing confirmation flows and core safety policy
//! remain responsible for unregistering and workspace cleanup.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Clear, Paragraph, Wrap},
};
use workbench_core::{WorkItem, WorkItemState};

use crate::{help, theme, ui::visible};

pub enum Intent {
    None,
    Cancel,
    Unregister(WorkItem),
    Cleanup(WorkItem),
}

#[derive(Default)]
pub struct RemovalUi {
    item: Option<WorkItem>,
    changed: bool,
    unavailable: bool,
}

impl RemovalUi {
    pub fn is_open(&self) -> bool {
        self.item.is_some()
    }

    pub fn open(&mut self, item: WorkItem) {
        if !self.is_open() {
            self.item = Some(item);
            self.changed = false;
            self.unavailable = false;
        }
    }

    /// Only registration identity matters, not status, prompts or selection.
    /// Failed loads pause actions; a confirmed change latches until reopening.
    pub fn observe(&mut self, items: Option<&[WorkItemState]>) {
        self.unavailable = items.is_none();
        if let (Some(target), Some(items)) = (&self.item, items) {
            self.changed |= !items.iter().any(|state| state.item == *target);
        }
    }

    pub fn help_context(&self) -> help::Context {
        help::Context::Removal {
            blocked: self.changed || self.unavailable,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Intent {
        if !self.is_open() || key.kind != KeyEventKind::Press || !key.modifiers.is_empty() {
            return Intent::None;
        }
        if key.code == KeyCode::Esc {
            self.item = None;
            return Intent::Cancel;
        }
        if self.changed || self.unavailable {
            return Intent::None;
        }
        match key.code {
            KeyCode::Char('u') => Intent::Unregister(self.item.take().unwrap()),
            KeyCode::Char('c') => Intent::Cleanup(self.item.take().unwrap()),
            // Enter does not choose a destructive default. No other view's
            // navigation, quit, reply, paste or maintenance actions apply here.
            _ => Intent::None,
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let Some(item) = &self.item else { return };
        let area = frame.area();
        frame.render_widget(
            theme::footer(self.help_context().hints()),
            Rect::new(
                area.x,
                area.bottom().saturating_sub(1),
                area.width,
                area.height.min(1),
            ),
        );
        let disabled = self.changed || self.unavailable;
        let width = area.width.saturating_sub(4).max(area.width.min(30)).min(58);
        let detailed = width >= 46 && area.height >= if disabled { 11 } else { 9 };
        let height = area
            .height
            .min(if detailed { 9 } else { 6 } + if disabled { 2 } else { 0 });
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::bordered()
            .title("Remove work item")
            .style(theme::panel())
            .border_style(theme::border(true))
            .title_style(theme::accent());
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let [target, _, actions, notice, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(u16::from(detailed)),
            Constraint::Length(if detailed { 4 } else { 2 }),
            Constraint::Length(if disabled { 2 } else { 0 }),
            Constraint::Length(1),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(format!("Work ID: {}", visible(&item.id))).style(theme::panel()),
            target,
        );
        let [unregister, cleanup] = Layout::vertical([
            Constraint::Length(if detailed { 2 } else { 1 }),
            Constraint::Min(0),
        ])
        .areas(actions);
        for (area, hint, description) in [
            (unregister, "Unregister: u", "Keep pane and files."),
            (
                cleanup,
                "Clean: c",
                "Close entire workmux window; remove worktree.",
            ),
        ] {
            let [heading, detail] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
            let line = if disabled {
                Line::styled(
                    format!("{} (disabled)", hint.split(':').next().unwrap()),
                    theme::muted(),
                )
            } else {
                theme::shortcut_line(hint)
            };
            frame.render_widget(Paragraph::new(line).style(theme::panel()), heading);
            frame.render_widget(
                Paragraph::new(description)
                    .style(theme::panel())
                    .wrap(Wrap { trim: false }),
                detail,
            );
        }
        let message = if self.changed {
            "Registration changed or removed. Cancel and reopen."
        } else if self.unavailable {
            "Registration check unavailable. Wait or cancel."
        } else {
            ""
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(theme::notice())
                .wrap(Wrap { trim: false }),
            notice,
        );
        frame.render_widget(theme::footer("Cancel: Esc | Help: ?"), footer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{AgentStatus, PaneAvailability, WorkItemKind, WorkspaceAvailability};

    fn state() -> WorkItemState {
        WorkItemState {
            item: WorkItem {
                id: "fix-auth".into(),
                title: "Fix auth".into(),
                repository: "/repo".into(),
                workspace: "/repo/worktree".into(),
                branch: Some("fix-auth".into()),
                kind: WorkItemKind::Implementation,
                pane_id: "%14".into(),
            },
            status: AgentStatus::Running,
            pane: PaneAvailability::Present,
            workspace_availability: WorkspaceAvailability::Present,
            status_detail: String::new(),
            attention_prompt: None,
            completion_fingerprint: None,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn menu() -> RemovalUi {
        let mut menu = RemovalUi::default();
        menu.open(state().item);
        menu
    }

    fn screen(menu: &RemovalUi, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                theme::paint(frame);
                menu.render(frame);
            })
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
    fn choices_capture_the_original_target_and_close_without_retrying() {
        for code in ['u', 'c'] {
            let original = state();
            let mut menu = menu();
            let mut other = original.clone();
            other.item.id = "other-selection".into();
            menu.open(other.item.clone()); // Cannot replace an already-open menu.
            menu.observe(Some(&[other, original.clone()]));
            match menu.key(key(KeyCode::Char(code))) {
                Intent::Unregister(item) => {
                    assert_eq!(code, 'u');
                    assert_eq!(item, original.item);
                }
                Intent::Cleanup(item) => {
                    assert_eq!(code, 'c');
                    assert_eq!(item, original.item);
                }
                _ => panic!("expected the explicit action"),
            }
            assert!(!menu.is_open());
            assert!(matches!(menu.key(key(KeyCode::Char(code))), Intent::None));
        }
    }

    #[test]
    fn enter_and_unapproved_keys_never_choose_an_action() {
        let mut menu = menu();
        for code in [
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('q'),
            KeyCode::Char('r'),
            KeyCode::Char('e'),
            KeyCode::Char('s'),
            KeyCode::Char('w'),
            KeyCode::Char('x'),
        ] {
            assert!(matches!(menu.key(key(code)), Intent::None));
            assert!(menu.is_open());
        }
        for code in [KeyCode::Char('c'), KeyCode::Char('u'), KeyCode::Esc] {
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
                KeyModifiers::SHIFT,
            ] {
                assert!(matches!(
                    menu.key(KeyEvent::new(code, modifiers)),
                    Intent::None
                ));
            }
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                let mut event = key(code);
                event.kind = kind;
                assert!(matches!(menu.key(event), Intent::None));
            }
        }
        assert!(matches!(menu.key(key(KeyCode::Esc)), Intent::Cancel));
        assert!(!menu.is_open());
    }

    #[test]
    fn activity_prompt_and_resource_changes_do_not_retarget_or_disable_actions() {
        for status in [
            AgentStatus::Running,
            AgentStatus::WaitingForInput,
            AgentStatus::Complete,
            AgentStatus::Unknown,
        ] {
            let mut menu = menu();
            let mut observed = state();
            observed.status = status;
            observed.pane = PaneAvailability::Missing;
            observed.workspace_availability = WorkspaceAvailability::Missing;
            observed.attention_prompt = Some("A new prompt".into());
            menu.observe(Some(&[observed.clone()]));
            assert_eq!(
                menu.help_context(),
                help::Context::Removal { blocked: false }
            );
            assert!(
                matches!(menu.key(key(KeyCode::Char('u'))), Intent::Unregister(item) if item == observed.item)
            );
        }
    }

    #[test]
    fn changed_or_removed_registrations_latch_until_cancel_and_reopen() {
        for change in 0..5 {
            let mut menu = menu();
            let mut observed = state();
            match change {
                0 => observed.item.id = "renamed".into(),
                1 => observed.item.pane_id = "%99".into(),
                2 => observed.item.workspace = "/other".into(),
                3 => observed.item.title = "New description".into(),
                _ => {}
            }
            menu.observe(Some(if change == 4 {
                &[]
            } else {
                std::slice::from_ref(&observed)
            }));
            menu.observe(Some(&[state()])); // An old snapshot cannot re-enable it.
            for code in ['u', 'c'] {
                assert!(matches!(menu.key(key(KeyCode::Char(code))), Intent::None));
            }
            assert_eq!(
                menu.help_context(),
                help::Context::Removal { blocked: true }
            );
            assert!(screen(&menu, 100, 24).contains("Registration changed or removed."));
            assert!(matches!(menu.key(key(KeyCode::Esc)), Intent::Cancel));
            menu.open(state().item);
            assert!(matches!(
                menu.key(key(KeyCode::Char('u'))),
                Intent::Unregister(_)
            ));
        }
    }

    #[test]
    fn a_failed_load_pauses_actions_without_inventing_removal() {
        let mut menu = menu();
        menu.observe(None);
        assert!(screen(&menu, 100, 24).contains("Registration check unavailable."));
        for code in ['u', 'c'] {
            assert!(matches!(menu.key(key(KeyCode::Char(code))), Intent::None));
        }
        menu.observe(Some(&[state()]));
        assert_eq!(
            menu.help_context(),
            help::Context::Removal { blocked: false }
        );
        assert!(matches!(
            menu.key(key(KeyCode::Char('c'))),
            Intent::Cleanup(_)
        ));
    }

    #[test]
    fn popup_shows_scope_context_and_menu_only_hints_at_normal_and_small_sizes() {
        let menu = menu();
        let text = screen(&menu, 100, 24);
        for expected in [
            "Remove work item",
            "Work ID: fix-auth",
            "Unregister: u",
            "Clean: c",
            "Keep pane and files.",
            "Close entire workmux window; remove worktree.",
            "Cancel: Esc",
            "Help: ?",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(!text.contains("%14"));
        assert!(!text.contains("Workspace:"));
        assert!(!text.contains("/repo/worktree"));
        assert!(!text.contains("Nothing is removed"));
        assert!(!text.contains("confirmation"));
        assert!(!text.contains('\u{2014}'));
        for (width, height) in [(30, 6), (30, 10), (80, 8)] {
            let text = screen(&menu, width, height);
            for expected in ["Work ID: fix-auth", "Unregister: u", "Clean: c", "Help: ?"] {
                assert!(text.contains(expected), "missing {expected}: {text}");
            }
        }
        for (width, height) in [(0, 0), (1, 1), (10, 5), (30, 6), (80, 12)] {
            screen(&menu, width, height);
        }
    }

    #[test]
    fn blocked_popup_keeps_specific_warnings_and_hides_action_shortcuts() {
        for unavailable in [false, true] {
            let mut menu = menu();
            menu.observe(if unavailable { None } else { Some(&[]) });
            let text = screen(&menu, 100, 24);
            for expected in ["Unregister (disabled)", "Clean (disabled)", "Cancel: Esc"] {
                assert!(text.contains(expected), "missing {expected}: {text}");
            }
            assert!(text.contains(if unavailable {
                "Registration check unavailable. Wait or cancel."
            } else {
                "Registration changed or removed. Cancel and reopen."
            }));
            assert!(!text.contains("Unregister: u"));
            assert!(!text.contains("Clean: c"));
            assert!(!text.contains('\u{2014}'));
            assert!(matches!(menu.key(key(KeyCode::Char('c'))), Intent::None));
            assert!(matches!(menu.key(key(KeyCode::Char('u'))), Intent::None));
        }
    }
}
