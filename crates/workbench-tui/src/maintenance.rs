//! Explicit, captured-target maintenance dialogs; no tmux/Git operations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};
use workbench_core::{
    Engine, MAX_SHORT_DESCRIPTION_CHARS, WorkItem, validate_short_description,
    validate_work_item_id,
};

use crate::{
    interaction::Draft,
    theme,
    ui::{display_path, visible},
};

#[derive(Debug)]
pub(crate) enum Request {
    Edit(WorkItem),
    Rename(WorkItem),
    Unregister(WorkItem),
}

impl Request {
    fn item(&self) -> &WorkItem {
        match self {
            Self::Edit(item) | Self::Rename(item) | Self::Unregister(item) => item,
        }
    }

    fn field_label(&self) -> &'static str {
        if matches!(self, Self::Rename(_)) {
            "Work ID"
        } else {
            "Short Description"
        }
    }

    fn editing(&self) -> bool {
        !matches!(self, Self::Unregister(_))
    }
}

pub(crate) enum Mutation {
    Description { expected: WorkItem, text: String },
    Rename { expected: WorkItem, text: String },
    Unregister(WorkItem),
}

impl Mutation {
    pub fn execute(&self, engine: &Engine) -> Result<(), String> {
        match self {
            Self::Description { expected, text } => engine
                .update_work_item_description(expected, text)
                .map(|_| ()),
            Self::Rename { expected, text } => engine.rename_work_item(expected, text).map(|_| ()),
            Self::Unregister(expected) => engine.unregister_work_item(expected),
        }
        .map_err(|error| error.to_string())
    }

    pub fn success_message(&self) -> String {
        match self {
            Self::Description { expected, .. } => {
                format!("Updated Short Description for {}.", expected.id)
            }
            Self::Rename { expected, text } => format!(
                "Renamed {} to {text}. Pane, workspace and review marks kept.",
                expected.id
            ),
            Self::Unregister(expected) => format!(
                "Unregistered {}. Only the Workbench entry was removed; pane, branch, worktree and review history were kept.",
                expected.id
            ),
        }
    }
}

pub(crate) enum Intent {
    None,
    Cancel,
    Save(Box<Mutation>),
}

#[derive(Default)]
pub(crate) struct MaintenanceUi {
    request: Option<Request>,
    text: String,
    saving: bool,
    error: Option<String>,
}

impl MaintenanceUi {
    pub fn is_open(&self) -> bool {
        self.request.is_some()
    }

    pub fn open(&mut self, request: Request) {
        if self.is_open() {
            return;
        }
        self.text = if matches!(request, Request::Rename(_)) {
            request.item().id.clone()
        } else {
            request.item().title.clone()
        };
        self.request = Some(request);
        self.saving = false;
        self.error = None;
    }

    pub fn finish(&mut self, result: Result<(), String>) {
        self.saving = false;
        match result {
            Ok(()) => self.request = None,
            Err(error) => self.error = Some(error),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Intent {
        if self.saving || !self.is_open() {
            return Intent::None;
        }
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.request = None;
            return Intent::Cancel;
        }
        if key.code == KeyCode::Enter && key.modifiers.is_empty() {
            let mutation = match self.request.as_ref().expect("dialog is open") {
                Request::Edit(item) => {
                    if let Err(error) = validate_short_description(&self.text) {
                        self.error = Some(error.to_string());
                        return Intent::None;
                    }
                    Mutation::Description {
                        expected: item.clone(),
                        text: self.text.clone(),
                    }
                }
                Request::Unregister(item) => Mutation::Unregister(item.clone()),
                Request::Rename(item) => {
                    if let Err(error) = validate_work_item_id(&self.text) {
                        self.error = Some(error.to_string());
                        return Intent::None;
                    }
                    Mutation::Rename {
                        expected: item.clone(),
                        text: self.text.clone(),
                    }
                }
            };
            self.saving = true;
            self.error = None;
            return Intent::Save(Box::new(mutation));
        }
        if self.request.as_ref().is_some_and(Request::editing) {
            let mut editor = Draft {
                item_id: String::new(),
                text: self.text.clone(),
            };
            match editor.edit(key) {
                Ok(()) => self.apply_edit(editor.text),
                Err(error) => {
                    self.error =
                        Some(error.replace("Replies", self.request.as_ref().unwrap().field_label()))
                }
            }
        }
        Intent::None
    }

    pub fn paste(&mut self, text: &str) {
        if self.saving || !self.request.as_ref().is_some_and(Request::editing) {
            return;
        }
        let mut editor = Draft {
            item_id: String::new(),
            text: self.text.clone(),
        };
        match editor.append(text) {
            Ok(()) => self.apply_edit(editor.text),
            Err(error) => {
                self.error =
                    Some(error.replace("Replies", self.request.as_ref().unwrap().field_label()))
            }
        }
    }

    fn apply_edit(&mut self, text: String) {
        let count = text.chars().count();
        // Legacy descriptions may exceed the new limit: allow reducing them,
        // but never silently truncate existing data or permit further growth.
        if matches!(self.request, Some(Request::Edit(_)))
            && count > MAX_SHORT_DESCRIPTION_CHARS
            && count >= self.text.chars().count()
        {
            self.error = Some(format!(
                "Short Description is limited to {MAX_SHORT_DESCRIPTION_CHARS} characters; input was not added."
            ));
        } else {
            self.text = text;
            self.error = None;
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let Some(request) = &self.request else {
            return;
        };
        theme::paint(frame);
        let editing = request.editing();
        let item = request.item();
        let [header, body, error, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(if matches!(request, Request::Rename(_)) {
                "RENAME WORK ID"
            } else if editing {
                "EDIT WORK ITEM"
            } else {
                "UNREGISTER WORK ITEM"
            })
            .style(theme::header()),
            header,
        );
        let [details, editor] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(if editing { 3 } else { 0 }),
        ])
        .areas(body);
        let mut text = format!(
            "Work ID: {}\nWorkspace: {}\n\n",
            visible(&item.id),
            visible(&display_path(&item.workspace))
        );
        if matches!(request, Request::Rename(_)) {
            text.push_str("Only Work ID will change. Description, pane mapping, workspace and review marks stay unchanged. Old review history is retained.");
        } else if editing {
            text.push_str(
                "Only Short Description will change. ID and pane mapping stay unchanged.",
            );
        } else {
            text.push_str(&format!("Short Description: {}\n\nRemove this entry from Workbench?\n\nTmux pane, branch, worktree, files and review history will be kept.", visible(&item.title)));
        }
        frame.render_widget(
            Paragraph::new(text)
                .style(theme::text())
                .wrap(Wrap { trim: false }),
            details,
        );
        if editing {
            let block = Block::bordered()
                .title(if matches!(request, Request::Rename(_)) {
                    "Work ID".into()
                } else {
                    format!(
                        "Short Description ({}/{MAX_SHORT_DESCRIPTION_CHARS})",
                        self.text.chars().count()
                    )
                })
                .style(theme::panel())
                .border_style(theme::border(true))
                .title_style(theme::accent());
            let inner = block.inner(editor);
            frame.render_widget(block, editor);
            let text = visible(&self.text);
            let width = Line::from(text.as_str()).width().min(u16::MAX as usize) as u16;
            let offset = width.saturating_sub(inner.width.saturating_sub(1));
            frame.render_widget(
                Paragraph::new(text)
                    .style(theme::panel())
                    .scroll((0, offset)),
                inner,
            );
            if !self.saving && inner.width > 0 && inner.height > 0 {
                frame.set_cursor_position((inner.x + width.saturating_sub(offset), inner.y));
            }
        }
        if let Some(message) = &self.error {
            frame.render_widget(
                Paragraph::new(visible(message))
                    .style(theme::error())
                    .wrap(Wrap { trim: false }),
                error,
            );
        }
        frame.render_widget(
            theme::footer(if self.saving {
                "Saving… | Please wait"
            } else if editing {
                "Enter: save | Esc/Ctrl-C: cancel\nBackspace: edit | Ctrl+u: clear (q types text)"
            } else {
                "Enter: unregister entry only | Esc/Ctrl-C: cancel"
            }),
            footer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::WorkItemKind;

    fn item(id: &str) -> WorkItem {
        WorkItem {
            id: id.into(),
            title: "Fix λ🙂".into(),
            repository: "/work".into(),
            workspace: "/work/task".into(),
            branch: Some("feature/task".into()),
            kind: WorkItemKind::Implementation,
            pane_id: "%14".into(),
        }
    }
    fn key(form: &mut MaintenanceUi, code: KeyCode) -> Intent {
        form.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn screen(form: &MaintenanceUi, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| form.render(frame)).unwrap();
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
    fn id_rename_captures_target_validates_and_preserves_draft_on_errors() {
        let mut form = MaintenanceUi::default();
        let original = item("A");
        form.open(Request::Rename(original.clone()));
        assert_eq!(form.text, "A");
        let text = screen(&form, 100, 20);
        assert!(text.contains("RENAME WORK ID"), "{text}");
        assert!(text.contains("review marks stay unchanged"), "{text}");
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for invalid in ["", " ", " B"] {
            form.text = invalid.into();
            assert!(matches!(key(&mut form, KeyCode::Enter), Intent::None));
            assert!(form.error.is_some());
        }
        form.text.clear();
        form.paste("Task λ🙂");
        form.paste("\nbad");
        assert_eq!(form.text, "Task λ🙂");
        let Intent::Save(mutation) = key(&mut form, KeyCode::Enter) else {
            panic!("expected rename")
        };
        let Mutation::Rename { expected, text } = &*mutation else {
            panic!("expected rename mutation")
        };
        assert_eq!(expected, &original);
        assert_eq!(text, "Task λ🙂");
        assert!(matches!(key(&mut form, KeyCode::Esc), Intent::None));
        form.paste("must not change");
        form.finish(Err("Duplicate ID; choose another".into()));
        assert!(form.is_open());
        assert_eq!(&form.text, text);
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        engine.register_work_item(original.clone()).unwrap();
        mutation.execute(&engine).unwrap();
        let mut renamed = original;
        renamed.id = text.clone();
        assert_eq!(engine.work_items().unwrap(), [renamed]);
        assert!(matches!(key(&mut form, KeyCode::Esc), Intent::Cancel));
        for (width, height) in [(40, 8), (1, 1), (0, 1)] {
            form.open(Request::Rename(item("A")));
            screen(&form, width, height);
        }
    }

    #[test]
    fn description_edit_captures_target_and_blocks_repeat_or_cancel_during_save() {
        let mut form = MaintenanceUi::default();
        let original = item("A");
        form.open(Request::Edit(original.clone()));
        form.open(Request::Edit(item("B")));
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste("Updated λ🙂");
        key(&mut form, KeyCode::Char('q'));
        let Intent::Save(mutation) = key(&mut form, KeyCode::Enter) else {
            panic!("expected save")
        };
        let Mutation::Description { expected, text } = *mutation else {
            panic!("expected description mutation")
        };
        assert_eq!(expected, original);
        assert_eq!(text, "Updated λ🙂q");
        assert!(form.saving);
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('q')] {
            assert!(matches!(key(&mut form, code), Intent::None));
        }
        form.paste("must not change");
        assert_eq!(form.text, text);
        form.finish(Err("State busy; retry".into()));
        assert!(form.is_open());
        assert_eq!(form.text, text);
        assert!(screen(&form, 80, 18).contains("State busy; retry"));
        assert!(matches!(key(&mut form, KeyCode::Enter), Intent::Save(_)));
        form.finish(Ok(()));
        assert!(!form.is_open());
    }

    #[test]
    fn descriptions_enforce_limits_atomic_paste_and_allow_shortening_legacy_text() {
        let mut form = MaintenanceUi::default();
        form.open(Request::Edit(item("A")));
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(matches!(key(&mut form, KeyCode::Enter), Intent::None));
        assert!(form.error.is_some());
        form.paste(&"🙂".repeat(MAX_SHORT_DESCRIPTION_CHARS));
        let boundary = form.text.clone();
        key(&mut form, KeyCode::Char('a'));
        assert_eq!(form.text, boundary);
        assert!(form.error.as_ref().unwrap().contains("120 characters"));
        form.paste("more");
        assert_eq!(form.text, boundary);
        form.paste("new\nline");
        assert_eq!(form.text, boundary);
        key(&mut form, KeyCode::Backspace);
        assert!(form.error.is_none());
        assert_eq!(form.text.chars().count(), MAX_SHORT_DESCRIPTION_CHARS - 1);
        let mut legacy = item("legacy");
        legacy.title = "λ".repeat(MAX_SHORT_DESCRIPTION_CHARS + 2);
        let mut form = MaintenanceUi::default();
        form.open(Request::Edit(legacy));
        assert!(matches!(key(&mut form, KeyCode::Enter), Intent::None));
        for _ in 0..2 {
            key(&mut form, KeyCode::Backspace);
        }
        assert_eq!(form.text.chars().count(), MAX_SHORT_DESCRIPTION_CHARS);
        assert!(matches!(key(&mut form, KeyCode::Enter), Intent::Save(_)));
    }

    #[test]
    fn unregister_requires_confirmation_and_cancel_never_emits_a_mutation() {
        let mut form = MaintenanceUi::default();
        form.open(Request::Unregister(item("A")));
        for code in [
            KeyCode::Char('u'),
            KeyCode::Char('y'),
            KeyCode::Char('q'),
            KeyCode::Delete,
        ] {
            assert!(matches!(key(&mut form, code), Intent::None));
        }
        form.paste("text cannot confirm or change target");
        assert!(matches!(
            form.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
            Intent::None
        ));
        assert!(matches!(key(&mut form, KeyCode::Esc), Intent::Cancel));
        assert!(!form.is_open());
        form.open(Request::Edit(item("B")));
        assert!(matches!(
            form.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Intent::Cancel
        ));
        form.open(Request::Unregister(item("A")));
        let Intent::Save(mutation) = key(&mut form, KeyCode::Enter) else {
            panic!("expected unregister")
        };
        let Mutation::Unregister(expected) = *mutation else {
            panic!("expected unregister mutation")
        };
        assert_eq!(expected, item("A"));
        assert!(matches!(key(&mut form, KeyCode::Enter), Intent::None));
    }

    #[test]
    fn dialogs_show_target_scope_counter_and_safe_controls_at_small_sizes() {
        for request in [Request::Edit(item("A")), Request::Unregister(item("A"))] {
            let mut form = MaintenanceUi::default();
            form.open(request);
            let text = screen(&form, 100, 20);
            assert!(text.contains("Work ID: A"), "{text}");
            assert!(text.contains("Workspace: /work/task"), "{text}");
            assert!(text.contains("Esc/Ctrl-C: cancel"), "{text}");
            if matches!(form.request, Some(Request::Edit(_))) {
                assert!(text.contains("Short Description (6/120)"), "{text}");
                assert!(
                    text.contains("ID and pane mapping stay unchanged"),
                    "{text}"
                );
            } else {
                assert!(text.contains("Remove this entry from Workbench?"), "{text}");
                assert!(
                    text.contains(
                        "Tmux pane, branch, worktree, files and review history will be kept."
                    ),
                    "{text}"
                );
                assert!(text.contains("Enter: unregister entry only"), "{text}");
            }
            for (width, height) in [(80, 12), (40, 8), (1, 1), (0, 1)] {
                screen(&form, width, height);
            }
        }
    }

    #[test]
    fn mutations_use_core_validation_and_do_not_redirect_after_external_changes() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let original = item("A");
        engine.register_work_item(original.clone()).unwrap();
        let edited = Mutation::Description {
            expected: original.clone(),
            text: "Updated".into(),
        };
        edited.execute(&engine).unwrap();
        assert!(
            edited
                .success_message()
                .contains("Updated Short Description for A")
        );
        let stale = Mutation::Unregister(original);
        assert!(
            stale
                .execute(&engine)
                .unwrap_err()
                .contains("changed since")
        );
        let fresh = engine.work_items().unwrap().remove(0);
        Mutation::Unregister(fresh).execute(&engine).unwrap();
        assert!(engine.work_items().unwrap().is_empty());
    }
}
