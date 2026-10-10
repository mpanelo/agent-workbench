//! Captured-target detail editing and registry mutations; no tmux/Git operations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Paragraph, Wrap},
};
use workbench_core::{
    Engine, MAX_SHORT_DESCRIPTION_CHARS, WorkItem, validate_short_description,
    validate_work_item_id,
};

use crate::{
    interaction::{Draft, InputCursor},
    theme,
    ui::{display_path, visible},
};

#[derive(Debug)]
pub(crate) enum Request {
    Edit(WorkItem),
}

impl Request {
    fn item(&self) -> &WorkItem {
        match self {
            Self::Edit(item) => item,
        }
    }
}

pub(crate) enum Mutation {
    Details {
        expected: WorkItem,
        id: String,
        description: String,
    },
    Unregister(WorkItem),
}

impl Mutation {
    pub fn execute(&self, engine: &Engine) -> Result<(), String> {
        match self {
            Self::Details {
                expected,
                id,
                description,
            } => engine
                .update_work_item_details(expected, id, description)
                .map(|_| ()),
            Self::Unregister(expected) => engine.unregister_work_item(expected),
        }
        .map_err(|error| error.to_string())
    }

    pub fn success_message(&self) -> String {
        match self {
            Self::Details { expected, id, .. } if expected.id != *id => format!(
                "Updated {} as {id}. Pane, workspace and review marks kept.",
                expected.id
            ),
            Self::Details { id, .. } => format!("Updated details for {id}."),
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
    id: String,
    field: Field,
    cursors: [InputCursor; 2],
    saving: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Field {
    Id,
    #[default]
    Description,
}

impl Field {
    fn index(self) -> usize {
        match self {
            Self::Id => 0,
            Self::Description => 1,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Id => "Work ID",
            Self::Description => "Short Description",
        }
    }
}

impl MaintenanceUi {
    pub fn help_context(&self) -> Option<crate::help::Context> {
        self.request.as_ref().map(|_| crate::help::Context::Edit)
    }
    pub fn is_open(&self) -> bool {
        self.request.is_some()
    }

    pub fn open(&mut self, request: Request) {
        if self.is_open() {
            return;
        }
        self.id = request.item().id.clone();
        self.text = request.item().title.clone();
        self.field = Field::Description;
        self.cursors = [InputCursor::default(); 2];
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
            let Request::Edit(item) = self.request.as_ref().expect("dialog is open");
            if let Err(error) = validate_work_item_id(&self.id) {
                self.field = Field::Id;
                self.error = Some(error.to_string());
                return Intent::None;
            }
            if let Err(error) = validate_short_description(&self.text) {
                self.field = Field::Description;
                self.error = Some(error.to_string());
                return Intent::None;
            }
            let mutation = Mutation::Details {
                expected: item.clone(),
                id: self.id.clone(),
                description: self.text.clone(),
            };
            self.saving = true;
            self.error = None;
            return Intent::Save(Box::new(mutation));
        }
        if matches!(
            key.code,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down
        ) && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.field = if self.field == Field::Id {
                Field::Description
            } else {
                Field::Id
            };
            return Intent::None;
        }
        let mut editor = Draft {
            item_id: String::new(),
            text: self.field_text().into(),
            cursor: self.cursors[self.field.index()],
        };
        match editor.edit(key) {
            Ok(()) => self.apply_edit(editor),
            Err(error) => self.error = Some(error.replace("Replies", self.field.label())),
        }
        Intent::None
    }

    pub fn paste(&mut self, text: &str) {
        if self.saving || !self.is_open() {
            return;
        }
        let mut editor = Draft {
            item_id: String::new(),
            text: self.field_text().into(),
            cursor: self.cursors[self.field.index()],
        };
        match editor.insert(text) {
            Ok(()) => self.apply_edit(editor),
            Err(error) => self.error = Some(error.replace("Replies", self.field.label())),
        }
    }

    fn apply_edit(&mut self, editor: Draft) {
        let count = editor.text.chars().count();
        // Legacy descriptions may exceed the new limit: allow reducing them,
        // but never silently truncate existing data or permit further growth.
        if self.field == Field::Description
            && editor.text != self.text
            && count > MAX_SHORT_DESCRIPTION_CHARS
            && count >= self.text.chars().count()
        {
            self.error = Some(format!(
                "Short Description is limited to {MAX_SHORT_DESCRIPTION_CHARS} characters; input was not added."
            ));
        } else {
            if self.field == Field::Id {
                self.id = editor.text;
            } else {
                self.text = editor.text;
            }
            self.cursors[self.field.index()] = editor.cursor;
            self.error = None;
        }
    }

    fn field_text(&self) -> &str {
        match self.field {
            Field::Id => &self.id,
            Field::Description => &self.text,
        }
    }

    fn render_field(&self, frame: &mut Frame<'_>, area: Rect, field: Field) {
        let active = self.field == field;
        let text = if field == Field::Id {
            &self.id
        } else {
            &self.text
        };
        let title = if field == Field::Id {
            "Work ID".into()
        } else {
            format!(
                "Short Description ({}/{MAX_SHORT_DESCRIPTION_CHARS})",
                text.chars().count()
            )
        };
        let block = Block::bordered()
            .title(title)
            .style(theme::panel())
            .border_style(theme::border(active))
            .title_style(if active {
                theme::accent()
            } else {
                theme::muted()
            });
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let width = self.cursors[field.index()].column(text);
        let text = visible(text);
        let offset = width.saturating_sub(inner.width.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(text)
                .style(theme::panel())
                .scroll((0, offset)),
            inner,
        );
        if active && !self.saving && inner.width > 0 && inner.height > 0 {
            frame.set_cursor_position((inner.x + width.saturating_sub(offset), inner.y));
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let Some(request) = &self.request else {
            return;
        };
        theme::paint(frame);
        let item = request.item();
        let [header, body, error, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new("EDIT WORK ITEM").style(theme::header()),
            header,
        );
        let [details, editor] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(6)]).areas(body);
        let mut text = format!(
            "Work ID: {}\nWorkspace: {}\n\n",
            visible(&item.id),
            visible(&display_path(&item.workspace))
        );
        text.push_str("Edit Work ID and Short Description together.\nPane mapping, workspace and review marks stay unchanged.\nOld review history is retained when renaming.");
        frame.render_widget(
            Paragraph::new(text)
                .style(theme::text())
                .wrap(Wrap { trim: false }),
            details,
        );
        let [id, description] =
            Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).areas(editor);
        self.render_field(frame, id, Field::Id);
        self.render_field(frame, description, Field::Description);
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
            } else {
                crate::help::Context::Edit.hints()
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
        form.open(Request::Edit(original.clone()));
        key(&mut form, KeyCode::Tab);
        assert_eq!(form.id, "A");
        let text = screen(&form, 100, 20);
        assert!(text.contains("EDIT WORK ITEM"), "{text}");
        assert!(text.contains("review marks stay unchanged"), "{text}");
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for invalid in ["", " ", " B"] {
            form.id = invalid.into();
            assert!(matches!(key(&mut form, KeyCode::Enter), Intent::None));
            assert!(form.error.is_some());
        }
        form.id.clear();
        form.paste("Task λ🙂");
        form.paste("\nbad");
        assert_eq!(form.id, "Task λ🙂");
        let Intent::Save(mutation) = key(&mut form, KeyCode::Enter) else {
            panic!("expected rename")
        };
        let Mutation::Details {
            expected,
            id: text,
            description,
        } = &*mutation
        else {
            panic!("expected rename mutation")
        };
        assert_eq!(expected, &original);
        assert_eq!(text, "Task λ🙂");
        assert_eq!(description, &original.title);
        assert!(matches!(key(&mut form, KeyCode::Esc), Intent::None));
        form.paste("must not change");
        form.finish(Err("Duplicate ID; choose another".into()));
        assert!(form.is_open());
        assert_eq!(&form.id, text);
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        engine.register_work_item(original.clone()).unwrap();
        mutation.execute(&engine).unwrap();
        let mut renamed = original;
        renamed.id = text.clone();
        assert_eq!(engine.work_items().unwrap(), [renamed]);
        assert!(matches!(key(&mut form, KeyCode::Esc), Intent::Cancel));
        for (width, height) in [(40, 8), (1, 1), (0, 1)] {
            form.open(Request::Edit(item("A")));
            screen(&form, width, height);
        }
    }

    #[test]
    fn combined_form_edits_both_fields_and_keeps_drafts_after_duplicate_id_failure() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let original = item("A");
        engine.register_work_item(original.clone()).unwrap();
        engine.register_work_item(item("Duplicate")).unwrap();
        let mut form = MaintenanceUi::default();
        form.open(Request::Edit(original.clone()));
        assert!(form.field == Field::Description);
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste("New description λ🙂");
        key(&mut form, KeyCode::Tab);
        assert!(form.field == Field::Id);
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste("Duplicate");
        let Intent::Save(mutation) = key(&mut form, KeyCode::Enter) else {
            panic!("expected save")
        };
        form.finish(mutation.execute(&engine));
        assert!(form.is_open());
        assert_eq!(form.id, "Duplicate");
        assert_eq!(form.text, "New description λ🙂");
        assert_eq!(engine.work_items().unwrap()[0], original);
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste("Renamed λ🙂");
        for event in [
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        ] {
            form.key(event);
        }
        assert!(form.field == Field::Description);
        let Intent::Save(mutation) = key(&mut form, KeyCode::Enter) else {
            panic!("expected save")
        };
        let Mutation::Details {
            expected,
            id,
            description,
        } = &*mutation
        else {
            panic!("expected details")
        };
        assert_eq!(expected, &original);
        assert_eq!(id, "Renamed λ🙂");
        assert_eq!(description, "New description λ🙂");
        form.finish(mutation.execute(&engine));
        assert!(!form.is_open());
        let saved = engine.work_items().unwrap().remove(0);
        let mut expected = original;
        expected.id = id.clone();
        expected.title = description.clone();
        assert_eq!(saved, expected);
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
        let Mutation::Details {
            expected,
            description: text,
            id,
        } = *mutation
        else {
            panic!("expected description mutation")
        };
        assert_eq!(expected, original);
        assert_eq!(id, original.id);
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
        let Intent::Save(mutation) = key(&mut form, KeyCode::Enter) else {
            panic!("expected empty description save");
        };
        let Mutation::Details {
            description, id, ..
        } = *mutation
        else {
            panic!("expected details edit");
        };
        assert!(description.is_empty());
        assert_eq!(id, "A");
        form.finish(Ok(()));
        form.open(Request::Edit(item("A")));
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste(" ");
        assert!(matches!(key(&mut form, KeyCode::Enter), Intent::None));
        assert!(form.error.is_some());
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste(&"🙂".repeat(MAX_SHORT_DESCRIPTION_CHARS));
        let boundary = form.text.clone();
        key(&mut form, KeyCode::Char('a'));
        assert_eq!(form.text, boundary);
        assert!(form.error.as_ref().unwrap().contains("120 characters"));
        form.paste("more");
        assert_eq!(form.text, boundary);
        form.paste("new\nline");
        assert_eq!(form.text, boundary);
        key(&mut form, KeyCode::Tab);
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste(&"a".repeat(MAX_SHORT_DESCRIPTION_CHARS + 1));
        assert_eq!(form.id.chars().count(), MAX_SHORT_DESCRIPTION_CHARS + 1);
        assert_eq!(form.text, boundary);
        form.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
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
    fn cancelling_an_edit_never_emits_a_mutation() {
        let mut form = MaintenanceUi::default();
        form.open(Request::Edit(item("A")));
        form.paste("unsaved edit");
        assert!(matches!(key(&mut form, KeyCode::Esc), Intent::Cancel));
        assert!(!form.is_open());
        form.open(Request::Edit(item("B")));
        assert!(matches!(
            form.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Intent::Cancel
        ));
        assert!(!form.is_open());
    }

    #[test]
    fn maintenance_shortcuts_truncate_on_one_row_at_80_columns() {
        let mut form = MaintenanceUi::default();
        form.open(Request::Edit(item("A")));
        let text = screen(&form, 80, 20);
        let rows: Vec<_> = text.lines().map(str::trim_end).collect();
        assert_eq!(
            rows[19],
            "Field: Tab/Shift-Tab/↑/↓ | Save: <enter> | Cancel: Esc/Ctrl-C | …"
        );
    }

    #[test]
    fn details_edit_keeps_independent_cursors_and_rejects_growth_without_moving_them() {
        let mut form = MaintenanceUi::default();
        form.open(Request::Edit(item("A")));
        form.text = "aλ🙂z".into();
        key(&mut form, KeyCode::Left);
        key(&mut form, KeyCode::Left);
        form.paste("X");
        assert_eq!(form.text, "aλX🙂z");
        let cursor = form.cursors[1];
        key(&mut form, KeyCode::Up);
        key(&mut form, KeyCode::Left);
        form.paste("B");
        assert_eq!(form.id, "BA");
        key(&mut form, KeyCode::Down);
        assert_eq!(form.cursors[1], cursor);
        key(&mut form, KeyCode::Backspace);
        assert_eq!(form.text, "aλ🙂z");
        let mut terminal = Terminal::new(TestBackend::new(24, 20)).unwrap();
        terminal.draw(|frame| form.render(frame)).unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 3);
        form.text = "a".repeat(MAX_SHORT_DESCRIPTION_CHARS + 10);
        key(&mut form, KeyCode::Left); // Movement also works for legacy long text.
        let cursor = form.cursors[1];
        form.paste("x");
        assert_eq!(form.cursors[1], cursor);
        assert_eq!(form.text.len(), MAX_SHORT_DESCRIPTION_CHARS + 10);
        key(&mut form, KeyCode::Backspace);
        assert_eq!(form.text.len(), MAX_SHORT_DESCRIPTION_CHARS + 9);
        form.finish(Err("save failed".into()));
        assert_eq!(form.cursors[1], cursor);
        for _ in 0..MAX_SHORT_DESCRIPTION_CHARS + 10 {
            key(&mut form, KeyCode::Left);
        }
        terminal.draw(|frame| form.render(frame)).unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 1);
        key(&mut form, KeyCode::Esc);
        form.open(Request::Edit(item("C")));
        assert_eq!(form.cursors, [InputCursor::default(); 2]);
    }

    #[test]
    fn dialogs_show_target_scope_counter_and_safe_controls_at_small_sizes() {
        let mut form = MaintenanceUi::default();
        form.open(Request::Edit(item("A")));
        let text = screen(&form, 100, 20);
        for expected in [
            "Work ID: A",
            "Workspace: /work/task",
            "Cancel: Esc/Ctrl-C",
            "Short Description (6/120)",
            "review marks stay unchanged",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(!text.contains("UNREGISTER"));
        for (width, height) in [(80, 12), (40, 8), (1, 1), (0, 1)] {
            screen(&form, width, height);
        }
    }

    #[test]
    fn mutations_use_core_validation_and_do_not_redirect_after_external_changes() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        let original = item("A");
        engine.register_work_item(original.clone()).unwrap();
        let edited = Mutation::Details {
            expected: original.clone(),
            id: original.id.clone(),
            description: "Updated".into(),
        };
        edited.execute(&engine).unwrap();
        assert!(edited.success_message().contains("Updated details for A"));
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
