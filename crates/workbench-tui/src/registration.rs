use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};
use workbench_core::{
    MAX_SHORT_DESCRIPTION_CHARS, RegistrationDraft, WorkItemKind, validate_short_description,
};

use crate::{
    interaction::{Draft, InputCursor},
    theme,
    ui::{display_path, visible},
};

const LABELS: [&str; 3] = ["ID", "Short Description", "Type"];

#[derive(Default)]
pub(crate) struct RegistrationUi {
    pub pane: Option<String>,
    pub draft: Option<RegistrationDraft>,
    pub loading: bool,
    pub saving: bool,
    pub error: Option<String>,
    pub generation: u64,
    fields: Vec<String>,
    selected: usize,
    cursors: [InputCursor; 3],
}

pub(crate) enum RegistrationIntent {
    None,
    Cancel,
    Save(Box<RegistrationDraft>),
}

impl RegistrationUi {
    pub fn open(&mut self, pane: String) -> u64 {
        self.generation += 1;
        self.pane = Some(pane);
        self.draft = None;
        self.loading = true;
        self.saving = false;
        self.error = None;
        self.fields.clear();
        self.selected = 0;
        self.cursors = [InputCursor::default(); 3];
        self.generation
    }

    pub fn finish(&mut self, ticket: u64, result: Result<RegistrationDraft, String>) {
        if ticket != self.generation || self.pane.is_none() {
            return;
        }
        self.loading = false;
        match result {
            Ok(draft) if self.pane.as_deref() == Some(draft.item.pane_id.as_str()) => {
                self.fields = vec![
                    draft.item.id.clone(),
                    draft.item.title.clone(),
                    draft.item.kind.to_string(),
                ];
                self.draft = Some(draft);
                self.error = None;
            }
            Ok(_) => {
                self.error =
                    Some("Registration result belongs to another pane; cancel and retry.".into())
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn close(&mut self) {
        self.pane = None;
        self.draft = None;
        self.loading = false;
        self.saving = false;
        self.generation += 1;
    }

    pub fn key(&mut self, key: KeyEvent) -> RegistrationIntent {
        if self.saving {
            return RegistrationIntent::None;
        }
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.close();
            return RegistrationIntent::Cancel;
        }
        if self.loading || self.draft.is_none() {
            return RegistrationIntent::None;
        }
        match key.code {
            KeyCode::Tab | KeyCode::Down => self.selected = (self.selected + 1) % LABELS.len(),
            KeyCode::BackTab | KeyCode::Up => {
                self.selected = (self.selected + LABELS.len() - 1) % LABELS.len()
            }
            KeyCode::Enter => {
                if let Err(error) = validate_short_description(&self.fields[1]) {
                    self.error = Some(error.to_string());
                    return RegistrationIntent::None;
                }
                let draft = self.draft.as_ref().expect("draft is present");
                let mut edited = draft.clone();
                edited.item.id = self.fields[0].clone();
                edited.item.title = self.fields[1].clone();
                edited.item.kind = if self.fields[2] == "Implementation" {
                    WorkItemKind::Implementation
                } else {
                    WorkItemKind::ExternalReview
                };
                self.saving = true;
                self.error = None;
                return RegistrationIntent::Save(Box::new(edited));
            }
            KeyCode::Char(' ') if self.selected == 2 && key.modifiers.is_empty() => {
                self.fields[2] = if self.fields[2] == "Implementation" {
                    "External Review"
                } else {
                    "Implementation"
                }
                .into();
            }
            _ if self.selected != 2 => {
                let mut editor = Draft {
                    item_id: String::new(),
                    text: self.fields[self.selected].clone(),
                    cursor: self.cursors[self.selected],
                };
                match editor.edit(key) {
                    Ok(()) => {
                        self.apply_edit(editor);
                    }
                    Err(error) => self.error = Some(error.replace("Replies", "Fields")),
                }
            }
            _ => {}
        }
        RegistrationIntent::None
    }

    pub fn paste(&mut self, text: &str) {
        if self.loading || self.saving || self.draft.is_none() || self.selected == 2 {
            return;
        }
        let mut editor = Draft {
            item_id: String::new(),
            text: self.fields[self.selected].clone(),
            cursor: self.cursors[self.selected],
        };
        match editor.insert(text) {
            Ok(()) => {
                self.apply_edit(editor);
            }
            Err(error) => self.error = Some(error.replace("Replies", "Fields")),
        }
    }

    fn apply_edit(&mut self, editor: Draft) {
        if self.selected == 1
            && editor.text != self.fields[self.selected]
            && editor.text.chars().count() > MAX_SHORT_DESCRIPTION_CHARS
        {
            self.error = Some(format!(
                "Short Description is limited to {MAX_SHORT_DESCRIPTION_CHARS} characters; input was not added."
            ));
        } else {
            self.fields[self.selected] = editor.text;
            self.cursors[self.selected] = editor.cursor;
            self.error = None;
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        theme::paint(frame);
        let [header, body, notice, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(format!(
                "REGISTER WORK ITEM — pane {}",
                visible(self.pane.as_deref().unwrap_or("—"))
            ))
            .style(theme::header()),
            header,
        );
        frame.render_widget(
            theme::footer(if self.loading {
                "Reading metadata… | Cancel: Esc"
            } else if self.saving {
                "Saving… | Please wait"
            } else if self.draft.is_none() {
                "Sessions: Esc"
            } else {
                crate::help::Context::Registration.hints()
            }),
            footer,
        );
        if self.loading {
            frame.render_widget(
                Paragraph::new("Reading pane and Git metadata… Cancel: Esc."),
                body,
            );
            return;
        }
        let Some(draft) = &self.draft else {
            // Discovery failures use the full body, rather than the short
            // validation area, so the manual-registration advice stays visible.
            if let Some(error) = &self.error {
                frame.render_widget(
                    Paragraph::new(visible(error))
                        .style(theme::error())
                        .wrap(Wrap { trim: false }),
                    body.union(notice),
                );
            }
            return;
        };
        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(visible(error))
                    .style(theme::error())
                    .wrap(Wrap { trim: false }),
                notice,
            );
        } else if self.saving {
            frame.render_widget(
                Paragraph::new(
                    "Saving registration… Please wait; input and cancellation are disabled.",
                )
                .style(theme::notice()),
                notice,
            );
        } else {
            frame.render_widget(
                Paragraph::new("Detected Git target is read-only. <enter> to register.")
                    .style(theme::muted()),
                notice,
            );
        }
        let mut metadata = vec![
            format!(
                "Workspace: {}",
                visible(&display_path(&draft.item.workspace))
            ),
            format!(
                "Branch: {}",
                visible(draft.item.branch.as_deref().unwrap_or("Detached HEAD"))
            ),
        ];
        if draft.item.repository != draft.item.workspace {
            metadata.push(format!(
                "Repository: {}",
                visible(&display_path(&draft.item.repository))
            ));
        }
        let metadata = metadata_rows(metadata, body.width);
        let metadata_height = metadata.len();
        // Keep at least one full editor on short terminals; very small terminals
        // prioritize input over the read-only summary.
        let [summary, editors] = Layout::vertical([
            Constraint::Length(
                u16::try_from(metadata_height)
                    .unwrap_or(u16::MAX)
                    .min(body.height.saturating_sub(3)),
            ),
            Constraint::Min(0),
        ])
        .areas(body);
        frame.render_widget(Paragraph::new(metadata).style(theme::muted()), summary);
        let areas = Layout::vertical([Constraint::Length(3); 3]).split(editors);
        // On short terminals, show only the active editor so it stays usable.
        if editors.height < 9 {
            if !self.fields.is_empty() {
                self.render_field(frame, editors, self.selected);
            }
        } else {
            for (index, area) in areas.iter().enumerate() {
                if !self.fields.is_empty() {
                    self.render_field(frame, *area, index);
                }
            }
        }
    }

    fn render_field(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect, index: usize) {
        let selected = index == self.selected;
        let count = if index == 1 {
            format!(
                " ({}/{MAX_SHORT_DESCRIPTION_CHARS})",
                self.fields[index].chars().count()
            )
        } else {
            String::new()
        };
        let block = Block::bordered()
            .title(format!(
                "{} / {} — {}{}{}",
                index + 1,
                LABELS.len(),
                LABELS[index],
                count,
                if selected { " (editing)" } else { "" }
            ))
            .style(if selected {
                theme::text().bg(theme::SURFACE)
            } else {
                theme::text()
            })
            .border_style(theme::border(selected))
            .title_style(if selected {
                theme::accent()
            } else {
                theme::muted()
            });
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let text = visible(&self.fields[index]);
        let width = self.cursors[index].column(&self.fields[index]);
        let offset = if selected {
            width.saturating_sub(inner.width.saturating_sub(1))
        } else {
            0
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(if selected {
                    theme::text().bg(theme::SURFACE)
                } else {
                    theme::text()
                })
                .scroll((0, offset)),
            inner,
        );
        if selected && !self.saving && index != 2 && inner.width > 0 && inner.height > 0 {
            frame.set_cursor_position((inner.x + width.saturating_sub(offset), inner.y));
        }
    }
}

// Hard-wrap detected paths without losing spaces or relying on estimated word
// wrap heights. The read-only summary must not overlap the editable fields.
fn metadata_rows(values: Vec<String>, width: u16) -> Vec<Line<'static>> {
    let limit = usize::from(width.max(1));
    let mut lines = Vec::new();
    for value in values {
        let mut row = String::new();
        let mut columns = 0;
        for ch in value.chars() {
            let ch_width = Line::from(ch.to_string()).width();
            if columns + ch_width > limit && !row.is_empty() {
                lines.push(Line::from(std::mem::take(&mut row)));
                columns = 0;
            }
            row.push(ch);
            columns += ch_width;
        }
        lines.push(Line::from(row));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::WorkItem;

    fn draft() -> RegistrationDraft {
        RegistrationDraft {
            item: WorkItem {
                id: "feature-task".into(),
                title: String::new(),
                kind: WorkItemKind::Implementation,
                repository: "/work/repo".into(),
                workspace: "/work/linked task".into(),
                branch: Some("feature/task".into()),
                pane_id: "%14".into(),
            },
            pane_directory: Some("/work/linked task/src".into()),
        }
    }
    fn ready() -> RegistrationUi {
        let mut form = RegistrationUi::default();
        let ticket = form.open("%14".into());
        form.finish(ticket, Ok(draft()));
        form
    }
    fn key(form: &mut RegistrationUi, code: KeyCode) -> RegistrationIntent {
        form.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn screen(form: &RegistrationUi, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| form.render(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width.max(1) as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn short_description_caps_typing_and_paste_without_partial_changes() {
        let mut form = ready();
        form.selected = 1;
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste(&"🙂".repeat(MAX_SHORT_DESCRIPTION_CHARS - 1));
        key(&mut form, KeyCode::Char('λ'));
        let boundary = form.fields[1].clone();
        assert_eq!(boundary.chars().count(), MAX_SHORT_DESCRIPTION_CHARS);
        key(&mut form, KeyCode::Char('q'));
        assert_eq!(form.fields[1], boundary);
        assert!(form.error.as_ref().unwrap().contains("120 characters"));
        key(&mut form, KeyCode::Backspace);
        assert!(form.error.is_none());
        let before_paste = form.fields[1].clone();
        form.paste("two");
        assert_eq!(form.fields[1], before_paste);
        assert!(form.error.is_some());
        key(&mut form, KeyCode::Char('a'));
        let RegistrationIntent::Save(saved) = key(&mut form, KeyCode::Enter) else {
            panic!("expected boundary save")
        };
        assert_eq!(
            saved.item.title.chars().count(),
            MAX_SHORT_DESCRIPTION_CHARS
        );
        // The new limit applies only to the short description, not the ID.
        let mut form = ready();
        form.paste(&"a".repeat(MAX_SHORT_DESCRIPTION_CHARS + 1));
        assert!(form.error.is_none());
    }

    #[test]
    fn short_description_label_counter_and_save_validation_are_visible() {
        let mut form = ready();
        form.selected = 1;
        for (width, height) in [(120, 24), (80, 12)] {
            let text = screen(&form, width, height);
            assert!(text.contains("Short Description (0/120)"), "{text}");
            assert!(!text.contains("Title"), "{text}");
        }
        for invalid in [" ".into(), "λ".repeat(MAX_SHORT_DESCRIPTION_CHARS + 1)] {
            form.fields[1] = invalid.clone();
            assert!(matches!(
                key(&mut form, KeyCode::Enter),
                RegistrationIntent::None
            ));
            assert!(!form.saving);
            assert!(form.error.is_some());
            assert_eq!(form.fields[1], invalid);
        }
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste("Ready");
        assert!(matches!(
            key(&mut form, KeyCode::Enter),
            RegistrationIntent::Save(_)
        ));
    }

    #[test]
    fn confirming_defaults_saves_the_bound_pane_and_disables_repeat_submission() {
        let mut form = ready();
        assert!(form.fields[1].is_empty());
        let RegistrationIntent::Save(saved) = key(&mut form, KeyCode::Enter) else {
            panic!("expected save");
        };
        assert_eq!(saved.item, draft().item);
        assert_eq!(saved.pane_directory, draft().pane_directory);
        assert!(form.saving);
        assert!(matches!(
            key(&mut form, KeyCode::Enter),
            RegistrationIntent::None
        ));
        assert!(matches!(
            key(&mut form, KeyCode::Esc),
            RegistrationIntent::None
        ));
        assert!(form.pane.is_some());
        form.paste("must not change");
        assert_eq!(form.fields[0], "feature-task");
    }

    #[test]
    fn only_identity_fields_are_editable_and_unicode_safe() {
        let mut form = ready();
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        form.paste("ID λ🙂");
        key(&mut form, KeyCode::Backspace);
        key(&mut form, KeyCode::Char('q'));
        assert_eq!(form.fields[0], "ID λq");
        form.paste("\nnot allowed");
        assert_eq!(form.fields[0], "ID λq");
        assert!(form.error.is_some());
        key(&mut form, KeyCode::Tab);
        key(&mut form, KeyCode::Tab);
        key(&mut form, KeyCode::Char(' '));
        key(&mut form, KeyCode::Up);
        assert_eq!(form.selected, 1);
        for _ in 0..3 {
            key(&mut form, KeyCode::Tab);
        }
        assert_eq!(form.selected, 1);
        assert_eq!(form.fields.len(), 3);
        key(&mut form, KeyCode::Tab);
        form.paste("must not edit type or metadata");
        assert_eq!(form.fields[2], "External Review");
        key(&mut form, KeyCode::BackTab);
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        let RegistrationIntent::Save(saved) = key(&mut form, KeyCode::Enter) else {
            panic!("expected save");
        };
        assert_eq!(saved.item.kind, WorkItemKind::ExternalReview);
        assert_eq!(saved.item.branch, draft().item.branch);
        assert_eq!(saved.item.repository, draft().item.repository);
        assert_eq!(saved.item.workspace, draft().item.workspace);
        assert_eq!(saved.item.id, "ID λq");
        assert_eq!(saved.item.pane_id, "%14");
    }

    #[test]
    fn save_errors_preserve_identity_edits_and_detected_target_for_retry() {
        let mut form = ready();
        form.fields[0] = "edited-id".into();
        form.fields[1] = "edited description".into();
        assert!(matches!(
            key(&mut form, KeyCode::Enter),
            RegistrationIntent::Save(_)
        ));
        form.saving = false;
        form.error = Some("Duplicate ID; choose another".into());
        assert_eq!(form.fields[0], "edited-id");
        assert_eq!(form.fields[1], "edited description");
        assert!(screen(&form, 80, 24).contains("Duplicate ID"));
        let RegistrationIntent::Save(saved) = key(&mut form, KeyCode::Enter) else {
            panic!("expected retry");
        };
        assert_eq!(saved.item.workspace, draft().item.workspace);
        assert_eq!(saved.item.branch, draft().item.branch);
    }

    #[test]
    fn cancelled_loading_and_stale_results_cannot_retarget_registration() {
        let mut form = RegistrationUi::default();
        let old = form.open("%14".into());
        assert!(screen(&form, 80, 12).contains("Reading pane"));
        assert!(matches!(
            key(&mut form, KeyCode::Esc),
            RegistrationIntent::Cancel
        ));
        let current = form.open("%15".into());
        form.finish(old, Ok(draft()));
        assert!(form.loading);
        form.finish(current, Ok(draft()));
        assert!(form.draft.is_none());
        assert!(form.error.as_ref().unwrap().contains("another pane"));
        assert!(!screen(&form, 80, 12).contains("Save: <enter>"));
        key(&mut form, KeyCode::Esc);
        form.finish(current, Ok(draft()));
        assert!(form.pane.is_none());
    }

    #[test]
    fn form_renders_defaults_active_field_and_escaped_metadata_at_small_sizes() {
        let mut form = ready();
        let wide = screen(&form, 120, 24);
        for text in [
            "pane %14",
            "Short Description (0/120)",
            "Repository",
            "Workspace",
            "feature/task",
            "Save: <enter>",
        ] {
            assert!(wide.contains(text), "{wide}");
        }
        form.selected = 1;
        form.draft.as_mut().unwrap().item.workspace = "/work/λ\x1b".into();
        let small = screen(&form, 80, 12);
        assert!(small.contains("2 / 3 — Short Description"));
        assert!(small.contains("/work/λ\\u{1b}"));
        assert!(!small.contains('\x1b'));
        for (width, height) in [(40, 8), (1, 1), (0, 1)] {
            screen(&form, width, height);
        }
    }

    #[test]
    fn registration_shortcuts_truncate_on_one_row_at_80_columns() {
        let form = ready();
        let text = screen(&form, 80, 24);
        let rows: Vec<_> = text.lines().map(str::trim_end).collect();
        assert_eq!(
            rows[23],
            "Field: Tab/Shift-Tab/↑/↓ | Toggle type: Space | Save: <enter> | …"
        );
    }

    #[test]
    fn field_cursors_survive_switches_failed_paste_and_render_at_the_cursor() {
        let mut form = ready();
        form.fields[0] = "aλ🙂z".into();
        key(&mut form, KeyCode::Left);
        key(&mut form, KeyCode::Left);
        form.paste("X");
        assert_eq!(form.fields[0], "aλX🙂z");
        let cursor = form.cursors[0];
        form.paste("\ninvalid");
        assert_eq!(form.cursors[0], cursor);
        assert_eq!(form.fields[0], "aλX🙂z");
        key(&mut form, KeyCode::Down);
        key(&mut form, KeyCode::Left);
        let description_cursor = form.cursors[1];
        key(&mut form, KeyCode::Up);
        assert_eq!(form.cursors[0], cursor);
        key(&mut form, KeyCode::Backspace);
        assert_eq!(form.fields[0], "aλ🙂z");
        let mut terminal = Terminal::new(TestBackend::new(24, 24)).unwrap();
        terminal.draw(|frame| form.render(frame)).unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 3); // border + aλ
        key(&mut form, KeyCode::Down);
        assert_eq!(form.cursors[1], description_cursor);
        form.fields[1] = "a".repeat(MAX_SHORT_DESCRIPTION_CHARS);
        let cursor = form.cursors[1];
        form.paste("x");
        assert_eq!(form.cursors[1], cursor);
        assert_eq!(form.fields[1].len(), MAX_SHORT_DESCRIPTION_CHARS);
        for _ in 0..MAX_SHORT_DESCRIPTION_CHARS {
            key(&mut form, KeyCode::Left);
        }
        terminal.draw(|frame| form.render(frame)).unwrap();
        assert_eq!(terminal.backend().cursor_position().x, 1);
        form.close();
        form.open("%14".into());
        assert_eq!(form.cursors, [InputCursor::default(); 3]);
    }

    #[test]
    fn registration_uses_theme_for_active_fields_inactive_fields_and_errors() {
        let mut form = ready();
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| form.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        theme::assert_text_style(buffer, "REGISTER WORK ITEM", theme::LAVENDER, theme::MANTLE);
        theme::assert_text_style(buffer, "feature-task", theme::TEXT, theme::SURFACE);
        theme::assert_text_style(buffer, "2 / 3", theme::SUBTEXT, theme::BASE);
        form.error = Some("test registration error".into());
        terminal.draw(|frame| form.render(frame)).unwrap();
        theme::assert_text_style(
            terminal.backend().buffer(),
            "test registration error",
            theme::RED,
            theme::BASE,
        );
    }

    #[test]
    fn read_only_summary_hides_duplicate_repository_and_labels_detached_head() {
        let mut form = ready();
        let item = &mut form.draft.as_mut().unwrap().item;
        item.repository = item.workspace.clone();
        item.branch = None;
        let text = screen(&form, 120, 24);
        assert!(text.contains("Workspace: /work/linked task"), "{text}");
        assert!(text.contains("Branch: Detached HEAD"), "{text}");
        assert!(!text.contains("Repository"), "{text}");
        assert!(!text.contains("optional"), "{text}");
        let RegistrationIntent::Save(saved) = key(&mut form, KeyCode::Enter) else {
            panic!("expected detached save");
        };
        assert!(saved.item.branch.is_none());
    }

    #[test]
    fn unavailable_metadata_blocks_saving_and_shows_recovery_instructions() {
        let mut form = RegistrationUi::default();
        let ticket = form.open("%14".into());
        form.finish(
            ticket,
            Err(
                "Git metadata is unavailable. Use workbench register for manual registration."
                    .into(),
            ),
        );
        let text = screen(&form, 40, 12);
        assert!(text.contains("workbench register"), "{text}");
        assert!(!text.contains("Detached HEAD"), "{text}");
        assert!(!text.contains("Save:"), "{text}");
        assert!(matches!(
            key(&mut form, KeyCode::Enter),
            RegistrationIntent::None
        ));
        assert!(!form.saving);
    }

    #[test]
    fn detected_metadata_wraps_without_losing_spaces_or_overlapping_fields() {
        let values = vec![
            "Workspace: /work/λ linked task".into(),
            "Branch: feature/task".into(),
        ];
        let rows = metadata_rows(values.clone(), 12);
        assert!(rows.iter().all(|row| row.width() <= 12));
        let text: String = rows.iter().map(|row| row.to_string()).collect();
        assert_eq!(text, values.join(""));
        let text = screen(&ready(), 40, 24);
        assert!(text.contains("Workspace: /work/linked task"), "{text}");
        assert!(text.contains("Branch: feature/task"), "{text}");
        assert!(text.contains("3 / 3 — Type"), "{text}");
    }
}
