use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};
use workbench_core::{RegistrationDraft, WorkItemKind};

use crate::{
    interaction::Draft,
    ui::{display_path, visible},
};

const LABELS: [&str; 6] = [
    "ID",
    "Title",
    "Kind",
    "Repository",
    "Workspace",
    "Branch (optional)",
];

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
                    display_path(&draft.item.repository),
                    display_path(&draft.item.workspace),
                    draft.item.branch.clone().unwrap_or_default(),
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
                let draft = self.draft.as_ref().expect("draft is present");
                let mut edited = draft.clone();
                edited.item.id = self.fields[0].clone();
                edited.item.title = self.fields[1].clone();
                edited.item.kind = if self.fields[2] == "Implementation" {
                    WorkItemKind::Implementation
                } else {
                    WorkItemKind::ExternalReview
                };
                match (
                    absolute_path(&self.fields[3]),
                    absolute_path(&self.fields[4]),
                ) {
                    (Ok(repository), Ok(workspace)) => {
                        edited.item.repository = repository;
                        edited.item.workspace = workspace;
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        self.error = Some(error);
                        return RegistrationIntent::None;
                    }
                }
                edited.item.branch = (!self.fields[5].is_empty()).then(|| self.fields[5].clone());
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
                };
                match editor.edit(key) {
                    Ok(()) => {
                        self.fields[self.selected] = editor.text;
                        self.error = None;
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
        };
        match editor.append(text) {
            Ok(()) => {
                self.fields[self.selected] = editor.text;
                self.error = None;
            }
            Err(error) => self.error = Some(error.replace("Replies", "Fields")),
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let [header, body, notice, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(format!(
                "REGISTER WORK ITEM — pane {}",
                visible(self.pane.as_deref().unwrap_or("—"))
            ))
            .style(Style::default().fg(Color::Cyan)),
            header,
        );
        frame.render_widget(Paragraph::new(if self.loading {
            "Reading metadata… | Esc: cancel"
        } else if self.saving {
            "Saving… | Please wait"
        } else if self.draft.is_none() {
            "Esc: return to SESSIONS"
        } else {
            "Tab/↑/↓: field | Space: toggle kind | Enter: save | Esc: cancel\nBackspace: edit | Ctrl+u: clear field (q types text)"
        }), footer);
        if self.loading {
            frame.render_widget(
                Paragraph::new("Reading pane and Git metadata… Esc: cancel."),
                body,
            );
            return;
        }
        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(visible(error))
                    .style(Style::default().fg(Color::Red))
                    .wrap(Wrap { trim: false }),
                notice,
            );
        } else if self.saving {
            frame.render_widget(
                Paragraph::new(
                    "Saving registration… Please wait; input and cancellation are disabled.",
                ),
                notice,
            );
        } else if let Some(message) = self.draft.as_ref().and_then(|draft| draft.notice.as_ref()) {
            frame.render_widget(
                Paragraph::new(visible(message)).wrap(Wrap { trim: false }),
                notice,
            );
        } else {
            frame.render_widget(
                Paragraph::new("Pane is fixed. Review the defaults, then Enter to register."),
                notice,
            );
        }
        let areas = Layout::vertical([Constraint::Length(3); 6]).split(body);
        // On short terminals, show only the active editor so it stays usable.
        if body.height < 18 {
            if !self.fields.is_empty() {
                self.render_field(frame, body, self.selected);
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
        let block = Block::bordered()
            .title(format!(
                "{} / 6 — {}{}",
                index + 1,
                LABELS[index],
                if selected { " (editing)" } else { "" }
            ))
            .border_style(Style::default().fg(if selected {
                Color::Yellow
            } else {
                Color::DarkGray
            }));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let text = visible(&self.fields[index]);
        let width = Line::from(text.as_str()).width().min(u16::MAX as usize) as u16;
        let offset = if selected {
            width.saturating_sub(inner.width.saturating_sub(1))
        } else {
            0
        };
        frame.render_widget(Paragraph::new(text).scroll((0, offset)), inner);
        if selected && !self.saving && index != 2 && inner.width > 0 && inner.height > 0 {
            frame.set_cursor_position((inner.x + width.saturating_sub(offset), inner.y));
        }
    }
}

fn absolute_path(text: &str) -> Result<std::path::PathBuf, String> {
    let path = if text == "~" || text.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| "HOME is unavailable; enter an absolute path.".to_owned())?;
        if text == "~" {
            home
        } else {
            home.join(&text[2..])
        }
    } else {
        text.into()
    };
    if !path.is_absolute() {
        return Err("Repository and workspace must be absolute paths (~/ is supported).".into());
    }
    Ok(path)
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
                title: "Task λ".into(),
                kind: WorkItemKind::Implementation,
                repository: "/work/repo".into(),
                workspace: "/work/linked task".into(),
                branch: Some("feature/task".into()),
                pane_id: "%14".into(),
            },
            pane_directory: Some("/work/linked task/src".into()),
            notice: None,
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
    fn confirming_defaults_saves_the_bound_pane_and_disables_repeat_submission() {
        let mut form = ready();
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
    fn editing_pasting_kind_and_optional_branch_are_explicit_and_unicode_safe() {
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
        for _ in 0..4 {
            key(&mut form, KeyCode::Tab);
        }
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        let RegistrationIntent::Save(saved) = key(&mut form, KeyCode::Enter) else {
            panic!("expected save");
        };
        assert_eq!(saved.item.kind, WorkItemKind::ExternalReview);
        assert_eq!(saved.item.branch, None);
        assert_eq!(saved.item.id, "ID λq");
        assert_eq!(saved.item.pane_id, "%14");
    }

    #[test]
    fn invalid_paths_and_save_errors_preserve_edits_for_retry() {
        let mut form = ready();
        form.selected = 4;
        form.fields[4] = "relative/path".into();
        assert!(matches!(
            key(&mut form, KeyCode::Enter),
            RegistrationIntent::None
        ));
        assert!(form.error.as_ref().unwrap().contains("absolute"));
        assert!(!form.saving);
        form.fields[4] = "/work/correct".into();
        assert!(matches!(
            key(&mut form, KeyCode::Enter),
            RegistrationIntent::Save(_)
        ));
        form.saving = false;
        form.error = Some("Duplicate ID; choose another".into());
        assert_eq!(form.fields[4], "/work/correct");
        assert!(screen(&form, 80, 24).contains("Duplicate ID"));
        assert!(absolute_path("relative").is_err());
        if let Some(home) = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            assert_eq!(
                absolute_path("~/some path").unwrap(),
                home.join("some path")
            );
        }
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
        assert!(!screen(&form, 80, 12).contains("Enter: save"));
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
            "Task λ",
            "Repository",
            "Workspace",
            "feature/task",
            "Enter: save",
        ] {
            assert!(wide.contains(text), "{wide}");
        }
        form.selected = 4;
        form.fields[4] = "/work/λ\x1b".into();
        let small = screen(&form, 80, 12);
        assert!(small.contains("5 / 6 — Workspace"));
        assert!(small.contains("/work/λ\\u{1b}"));
        assert!(!small.contains('\x1b'));
        for (width, height) in [(40, 8), (1, 1), (0, 1)] {
            screen(&form, width, height);
        }
    }
}
