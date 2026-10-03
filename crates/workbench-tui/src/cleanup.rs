//! Captured-target, cancel-first confirmation. The core owns all cleanup policy.
use crate::{
    help, theme,
    ui::{display_path, visible},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Line,
    widgets::Paragraph,
};
use workbench_core::{CleanupDetails, CleanupPreview, WorkItem};

pub enum Intent {
    None,
    Cancel,
    Confirm(Box<CleanupPreview>),
}

#[derive(Default)]
pub struct CleanupUi {
    item: Option<WorkItem>,
    preview: Option<CleanupPreview>,
    loading: bool,
    executing: bool,
    proceed: bool,
    error: Option<String>,
    scroll: u16,
    pub generation: u64,
}

impl CleanupUi {
    pub fn is_open(&self) -> bool {
        self.item.is_some()
    }
    pub fn open(&mut self, item: WorkItem) -> u64 {
        self.generation += 1;
        self.item = Some(item);
        self.preview = None;
        self.loading = true;
        self.executing = false;
        self.proceed = false;
        self.error = None;
        self.scroll = 0;
        self.generation
    }
    pub fn finish_preview(&mut self, ticket: u64, result: Result<CleanupPreview, String>) {
        if ticket != self.generation || !self.is_open() {
            return;
        }
        self.loading = false;
        self.proceed = false;
        match result {
            Ok(preview)
                if self
                    .item
                    .as_ref()
                    .is_some_and(|item| item.id == preview.details.work_id) =>
            {
                self.preview = Some(preview)
            }
            Ok(_) => {
                self.error = Some("Preview belongs to another work item; cancel and reopen.".into())
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn finish_execution(&mut self, ticket: u64, result: Result<(), String>) {
        if ticket != self.generation || !self.is_open() {
            return;
        }
        self.executing = false;
        match result {
            Ok(()) => self.close(),
            Err(error) => {
                self.preview = None; // No blind retry of a potentially partial cleanup.
                self.proceed = false;
                self.error = Some(error);
            }
        }
    }
    pub fn fail(&mut self, error: String) {
        self.loading = false;
        self.finish_execution(self.generation, Err(error));
    }
    fn close(&mut self) {
        self.item = None;
        self.preview = None;
        self.loading = false;
        self.generation += 1;
    }
    pub fn key(&mut self, key: KeyEvent, height: u16) -> Intent {
        if self.executing || !self.is_open() {
            return Intent::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            self.close();
            return Intent::Cancel;
        }
        if key.modifiers == KeyModifiers::CONTROL {
            let amount = (height.saturating_sub(3) / 2).max(1);
            match key.code {
                KeyCode::Char('d') => self.scroll = self.scroll.saturating_add(amount),
                KeyCode::Char('u') => self.scroll = self.scroll.saturating_sub(amount),
                _ => {}
            }
            return Intent::None;
        }
        if !key.modifiers.is_empty() {
            return Intent::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close();
                return Intent::Cancel;
            }
            KeyCode::Tab | KeyCode::Left | KeyCode::Right if self.preview.is_some() => {
                self.proceed = !self.proceed
            }
            KeyCode::Enter if !self.proceed => {
                self.close();
                return Intent::Cancel;
            }
            KeyCode::Enter if !self.loading => {
                if let Some(preview) = self.preview.clone() {
                    self.executing = true;
                    return Intent::Confirm(Box::new(preview));
                }
            }
            _ => {}
        }
        Intent::None
    }
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let Some(item) = &self.item else {
            return;
        };
        theme::paint(frame);
        let [header, body, choice, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new("CLEAN UP WORKMUX WORKSPACE").style(theme::header()),
            header,
        );
        let mut lines = if let Some(preview) = &self.preview {
            preview_lines(&preview.details)
        } else {
            vec![
                Line::raw(format!("Work ID: {}", visible(&item.id))),
                Line::raw(format!(
                    "Workspace: {}",
                    visible(&display_path(&item.workspace))
                )),
            ]
        };
        if self.loading {
            lines.push(Line::styled(
                "Checking ownership and Git safety… Nothing is being removed.",
                theme::muted(),
            ));
        }
        if let Some(error) = &self.error {
            lines.push(Line::default());
            lines.push(Line::styled(visible(error), theme::error()));
            lines.push(Line::styled(
                "Cleanup is disabled. Cancel and inspect the workspace before reopening.",
                theme::notice(),
            ));
        }
        let lines = help::wrapped_lines(lines, body.width);
        self.scroll = self
            .scroll
            .min((lines.len().min(usize::from(u16::MAX)) as u16).saturating_sub(body.height));
        frame.render_widget(
            Paragraph::new(lines)
                .style(theme::text())
                .scroll((self.scroll, 0)),
            body,
        );
        let choices = if self.executing {
            "Cleaning up… Please wait. Do not retry."
        } else if self.proceed {
            "  Cancel     > Clean up window + worktree"
        } else if self.preview.is_some() {
            "> Cancel       Clean up window + worktree"
        } else {
            "> Cancel       Clean up unavailable"
        };
        frame.render_widget(Paragraph::new(choices).style(theme::notice()), choice);
        frame.render_widget(theme::footer(help::Context::Cleanup.hints()), footer);
    }
}

fn preview_lines(details: &CleanupDetails) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(format!("Work ID: {}", visible(&details.work_id))),
        Line::raw(format!(
            "Remove worktree: {}",
            visible(&display_path(&details.worktree))
        )),
        Line::raw(format!("Keep branch: {}", visible(&details.branch))),
        Line::raw(format!(
            "Close ENTIRE window: {} (session: {})",
            visible(&details.window.name),
            visible(&details.sessions.join(", "))
        )),
        Line::default(),
        Line::styled(
            "Every pane below will close, terminating its running programs:",
            theme::notice(),
        ),
    ];
    for pane in &details.window.panes {
        lines.push(Line::raw(format!(
            "  {} — {} — {}",
            visible(&pane.title),
            visible(pane.current_command.as_deref().unwrap_or("unknown command")),
            pane.working_directory
                .as_ref()
                .map(|p| visible(&display_path(p)))
                .unwrap_or_else(|| "directory unavailable".into())
        )));
    }
    lines.extend([
        Line::default(),
        Line::styled(
            "Git check: clean (no staged, unstaged, or untracked changes).",
            theme::accent(),
        ),
        Line::raw("Branch and Workbench review history will be kept."),
        Line::raw("The Workbench entry is removed only after cleanup succeeds."),
        Line::styled(
            "Workmux's configured cleanup hooks and managed-resource cleanup will run.",
            theme::notice(),
        ),
        Line::default(),
        Line::styled(
            "Ignored files/directories in this worktree will also be deleted:",
            theme::notice(),
        ),
    ]);
    if details.ignored_paths.is_empty() {
        lines.push(Line::raw("  None found."));
    }
    for path in &details.ignored_paths {
        lines.push(Line::raw(format!(
            "  {}",
            visible(&path.display().to_string())
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{Pane, Window, WorkItemKind};

    fn item(id: &str) -> WorkItem {
        WorkItem {
            id: id.into(),
            title: "Task".into(),
            repository: "/repo".into(),
            workspace: "/trees/task".into(),
            branch: Some("feature".into()),
            kind: WorkItemKind::Implementation,
            pane_id: "%1".into(),
        }
    }

    #[test]
    fn default_enter_cancels_loading_and_failed_previews_without_confirmation() {
        for failed in [false, true] {
            let mut ui = CleanupUi::default();
            let ticket = ui.open(item("A"));
            if failed {
                ui.finish_preview(ticket, Err("blocked".into()));
            }
            for code in [KeyCode::Tab, KeyCode::Left, KeyCode::Right] {
                assert!(matches!(ui.key(KeyEvent::from(code), 24), Intent::None));
                assert!(!ui.proceed);
            }
            assert!(matches!(
                ui.key(KeyEvent::from(KeyCode::Enter), 24),
                Intent::Cancel
            ));
            assert!(!ui.is_open());
        }
    }

    #[test]
    fn stale_results_do_not_change_new_target_and_execution_blocks_all_keys() {
        let mut ui = CleanupUi::default();
        let old = ui.open(item("A"));
        ui.open(item("B"));
        ui.finish_preview(old, Err("stale preview".into()));
        ui.finish_execution(old, Ok(()));
        assert!(ui.loading);
        assert!(ui.error.is_none());
        assert_eq!(ui.item.as_ref().unwrap().id, "B");
        ui.executing = true;
        for key in [
            KeyEvent::from(KeyCode::Enter),
            KeyEvent::from(KeyCode::Esc),
            KeyEvent::from(KeyCode::Char('q')),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert!(matches!(ui.key(key, 24), Intent::None));
            assert!(ui.is_open());
        }
        ui.finish_execution(ui.generation, Err("partly removed".into()));
        assert!(!ui.executing);
        assert!(ui.preview.is_none());
        assert!(!ui.proceed);
        assert!(matches!(
            ui.key(KeyEvent::from(KeyCode::Tab), 24),
            Intent::None
        ));
        assert!(!ui.proceed);
    }

    #[test]
    fn preview_discloses_all_panes_ignored_files_hooks_and_kept_history() {
        let details = CleanupDetails {
            work_id: "A".into(),
            handle: "task".into(),
            repository: "/repo".into(),
            worktree: "/trees/task".into(),
            branch: "feature".into(),
            sessions: vec!["session".into()],
            ignored_paths: vec![".env".into()],
            window: Window {
                id: "@7".into(),
                index: 0,
                name: "task".into(),
                panes: ["Agent", "Shell"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, title)| Pane {
                        id: format!("%{index}"),
                        index: index as u32,
                        title: title.into(),
                        current_command: Some(if index == 0 { "codex" } else { "fish" }.into()),
                        working_directory: Some("/trees/task".into()),
                    })
                    .collect(),
            },
        };
        let text = preview_lines(&details)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "ENTIRE window",
            "Agent",
            "Shell",
            "codex",
            "fish",
            ".env",
            "review history",
            "only after cleanup succeeds",
            "cleanup hooks",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
    }

    #[test]
    fn long_errors_can_scroll_and_choices_remain_visible_in_narrow_windows() {
        let mut ui = CleanupUi::default();
        let ticket = ui.open(item("A"));
        ui.finish_preview(ticket, Err("long error description ".repeat(30)));
        let mut terminal = Terminal::new(TestBackend::new(35, 8)).unwrap();
        terminal.draw(|frame| ui.render(frame)).unwrap();
        ui.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), 8);
        terminal.draw(|frame| ui.render(frame)).unwrap();
        assert!(ui.scroll > 0);
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("> Cancel"));
        assert!(text.contains("unavailable"));
        ui.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL), 8);
        assert_eq!(ui.scroll, 0);
    }
}
