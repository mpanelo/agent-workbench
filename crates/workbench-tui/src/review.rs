use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph},
};
use workbench_core::{ChangedFile, ReviewSession, ReviewStatus};

use crate::theme;
use crate::ui::{display_path, visible};

#[derive(Default)]
pub(crate) struct ReviewUi {
    sessions: HashMap<String, ReviewSession>,
    current: Option<String>,
    generation: u64,
    loading: bool,
    error: Option<String>,
    save_error: Option<String>,
    saving: bool,
    selected: usize,
    scroll: u16,
    horizontal: u16,
    since_review: bool,
    pub base: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReviewIntent {
    None,
    Reload,
    Save {
        ticket: u64,
        session: Box<ReviewSession>,
        path: PathBuf,
        reviewed: bool,
    },
}

impl ReviewUi {
    pub fn new(base: Option<String>) -> Self {
        Self {
            base,
            ..Self::default()
        }
    }
    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn fail_loading(&mut self, error: String) {
        if self.is_open() && self.loading {
            self.finish(self.generation, Err(error));
        }
    }

    pub fn open(&mut self, id: String) -> Option<u64> {
        self.generation += 1;
        self.sessions.remove(&id);
        self.loading = true;
        self.current = Some(id);
        self.error = None;
        self.save_error = None;
        self.selected = 0;
        self.scroll = 0;
        self.horizontal = 0;
        self.since_review = false;
        self.loading.then_some(self.generation)
    }

    pub fn reload(&mut self) -> Option<(u64, String)> {
        let id = self.current.clone()?;
        self.sessions.remove(&id);
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.save_error = None;
        self.selected = 0;
        self.scroll = 0;
        self.horizontal = 0;
        Some((self.generation, id))
    }

    pub fn finish(&mut self, ticket: u64, result: Result<ReviewSession, String>) {
        if !self.is_open() || ticket != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(session) if self.current.as_deref() == Some(session.diff.work_item_id.as_str()) => {
                self.since_review = !session.changes_since_review().is_empty();
                self.sessions
                    .insert(session.diff.work_item_id.clone(), session);
                self.error = None;
            }
            Ok(_) => {
                self.error = Some("Diff result belongs to a different work item; reload.".into())
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn finish_save(&mut self, ticket: u64, result: Result<ReviewSession, String>) {
        if !self.is_open() || ticket != self.generation {
            return;
        }
        self.saving = false;
        match result {
            Ok(session) if self.current.as_deref() == Some(session.diff.work_item_id.as_str()) => {
                self.selected = self
                    .selected
                    .min(files(&session, self.since_review).len().saturating_sub(1));
                self.scroll = 0;
                self.horizontal = 0;
                self.sessions
                    .insert(session.diff.work_item_id.clone(), session);
                self.save_error = None;
            }
            Ok(_) => {
                self.save_error = Some("Review save returned another item; reopen review.".into())
            }
            Err(error) => self.save_error = Some(error),
        }
    }

    pub fn fail_saving(&mut self, error: String) {
        if self.saving {
            self.finish_save(self.generation, Err(error));
        }
    }

    #[cfg(test)]
    fn finish_diff(&mut self, ticket: u64, result: Result<workbench_core::WorkItemDiff, String>) {
        self.finish(ticket, result.map(ReviewSession::new));
    }

    pub fn key(&mut self, key: KeyEvent, height: u16) -> ReviewIntent {
        if self.saving {
            return ReviewIntent::None;
        }
        if key.code == KeyCode::Esc {
            self.current = None;
            self.loading = false;
            self.generation += 1;
            return ReviewIntent::None;
        }
        if key.modifiers == KeyModifiers::CONTROL {
            let amount = (height.saturating_sub(4) / 2).max(1);
            match key.code {
                KeyCode::Char('d') => self.scroll = self.scroll.saturating_add(amount),
                KeyCode::Char('u') => self.scroll = self.scroll.saturating_sub(amount),
                _ => {}
            }
            return ReviewIntent::None;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return ReviewIntent::None;
        }
        if key.code == KeyCode::Char('r') {
            return ReviewIntent::Reload;
        }
        if key.code == KeyCode::Char('c') {
            self.since_review = !self.since_review;
            self.selected = 0;
            self.scroll = 0;
            self.horizontal = 0;
            return ReviewIntent::None;
        }
        let Some(session) = self
            .current
            .as_ref()
            .and_then(|id| self.sessions.get_mut(id))
        else {
            return ReviewIntent::None;
        };
        let files = files(session, self.since_review);
        let previous = self.selected;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(files.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Tab if !files.is_empty() => {
                if let Some(next) = (1..=files.len())
                    .map(|step| (self.selected + step) % files.len())
                    .find(|index| !session.is_reviewed(&files[*index].path))
                {
                    self.selected = next;
                }
            }
            KeyCode::Char(' ') => {
                if let Some(file) = files.get(self.selected) {
                    let path = file.path.clone();
                    let reviewed = !session.is_reviewed(&path);
                    self.saving = true;
                    self.save_error = None;
                    return ReviewIntent::Save {
                        ticket: self.generation,
                        session: Box::new(session.clone()),
                        path,
                        reviewed,
                    };
                }
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(height.saturating_sub(4).max(1))
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(height.saturating_sub(4).max(1))
            }
            KeyCode::Home => {
                self.scroll = 0;
                self.horizontal = 0;
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.horizontal = self.horizontal.saturating_sub(8)
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.horizontal = self.horizontal.saturating_add(8)
            }
            _ => {}
        }
        if previous != self.selected {
            self.scroll = 0;
            self.horizontal = 0;
        }
        ReviewIntent::None
    }

    pub fn render(&mut self, frame: &mut Frame<'_>) {
        theme::paint(frame);
        let [header, summary, notice, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(if self.save_error.is_some() { 3 } else { 0 }),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        let id = self.current.as_deref().unwrap_or("—");
        frame.render_widget(
            Paragraph::new(format!(
                "AGENT WORKBENCH — {} — {}",
                if self.since_review
                    && self
                        .sessions
                        .get(id)
                        .is_some_and(|session| !session.changes_since_review().is_empty())
                {
                    "RE-REVIEW REQUIRED"
                } else if self.since_review {
                    "SINCE REVIEW"
                } else {
                    "REVIEW"
                },
                visible(id)
            ))
            .style(theme::header()),
            header,
        );
        frame.render_widget(
            theme::footer("j/k: files | Space: reviewed | <c-d>/<c-u>: scroll | h/l: pan | r: reload\nc: full/since | Tab: unreviewed | PgUp/Dn: page | Home: top | Esc: back | q: quit"),
            footer,
        );
        if self.loading {
            frame.render_widget(
                Paragraph::new("Loading local Git diff… Esc returns without waiting."),
                body,
            );
            return;
        }
        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(visible(error))
                    .style(theme::error())
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                body,
            );
            return;
        }
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let (reviewed, total) = session.progress();
        let files = files(session, self.since_review);
        let (added, deleted) = files.iter().fold((0, 0), |(a, d), file| {
            (
                a + file.additions.unwrap_or(0),
                d + file.deletions.unwrap_or(0),
            )
        });
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{reviewed} / {total} reviewed"),
                        theme::review_status(ReviewStatus::Reviewed),
                    ),
                    Span::raw(" • "),
                    Span::styled(
                        format!("+{added}"),
                        theme::review_status(ReviewStatus::Reviewed),
                    ),
                    Span::raw(" "),
                    Span::styled(format!("-{deleted}"), theme::error()),
                    Span::styled(
                        format!(
                            " {} • base {} ({})",
                            if self.since_review {
                                "since review (text)"
                            } else {
                                "(text)"
                            },
                            visible(&session.diff.base),
                            &session.diff.base_revision[..session.diff.base_revision.len().min(12)]
                        ),
                        theme::muted(),
                    ),
                ]),
                Line::from(if self.saving {
                    "Saving review mark… Please wait (input temporarily disabled).".to_owned()
                } else if self.save_error.is_some() {
                    "Save failed; previous marks unchanged. Space: retry | r: reload".to_owned()
                } else {
                    format!(
                        "Saved marks • {} changed after review • {}",
                        session.changed_after_review(),
                        visible(&display_path(&session.diff.workspace))
                    )
                })
                .style(if self.save_error.is_some() {
                    theme::error()
                } else if self.saving {
                    theme::notice()
                } else {
                    theme::muted()
                }),
            ]),
            summary,
        );
        if let Some(error) = &self.save_error {
            frame.render_widget(
                Paragraph::new(visible(error))
                    .style(theme::error())
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                notice,
            );
        }
        if files.is_empty() {
            frame.render_widget(
                Paragraph::new(
                    if self.since_review { "No changes since review. c: full diff (includes never-reviewed files). Esc: back." }
                    else { "No changes against this base (including untracked files). Esc: back." },
                ),
                body,
            );
            return;
        }
        self.selected = self.selected.min(files.len() - 1);
        let [file_area, patch] = if body.width < 60 {
            Layout::vertical([
                Constraint::Length((body.height / 3).clamp(1, 5)),
                Constraint::Min(0),
            ])
            .areas(body)
        } else {
            Layout::horizontal([
                Constraint::Length((body.width / 3).clamp(24, 40)),
                Constraint::Min(0),
            ])
            .areas(body)
        };
        let items: Vec<_> = files
            .iter()
            .map(|file| {
                let mark = match session.status(&file.path) {
                    ReviewStatus::Reviewed => "✓",
                    ReviewStatus::Unreviewed => "○",
                    ReviewStatus::ChangedAfterReview => "⚠",
                };
                let name = visible(&file.path.to_string_lossy());
                let name_line = Line::from(vec![
                    Span::styled(
                        format!("{mark} "),
                        theme::review_status(session.status(&file.path)),
                    ),
                    Span::raw(name.clone()),
                ]);
                if self.since_review {
                    ListItem::new(vec![
                        name_line,
                        Line::from(format!(
                            "  {}",
                            file.additions
                                .zip(file.deletions)
                                .map(|(a, d)| format!("{} lines (+{a} -{d})", a + d))
                                .unwrap_or_else(|| "non-text / unavailable".into())
                        ))
                        .style(theme::muted()),
                    ])
                } else {
                    let mut name_line = name_line;
                    name_line
                        .spans
                        .push(Span::styled(format!("  {}", file.kind), theme::muted()));
                    ListItem::new(name_line)
                }
            })
            .collect();
        frame.render_stateful_widget(
            List::new(items)
                .block(if file_area.height >= 3 {
                    Block::bordered()
                        .title(if self.since_review {
                            "Since review"
                        } else {
                            "Changed files"
                        })
                        .border_style(theme::border(true))
                        .title_style(theme::accent())
                } else {
                    Block::default()
                })
                .highlight_symbol("> ")
                .highlight_style(theme::selection()),
            file_area,
            &mut ListState::default().with_selected(Some(self.selected)),
        );
        let file = &files[self.selected];
        let title = format!(
            "{} • {}",
            visible(&file.path.to_string_lossy()),
            if file.additions.is_none() {
                if self.since_review {
                    "non-text / unavailable; see notice".into()
                } else {
                    "binary; external viewer needed".into()
                }
            } else {
                format!(
                    "+{} -{}",
                    file.additions.unwrap_or(0),
                    file.deletions.unwrap_or(0)
                )
            }
        );
        let block = Block::bordered()
            .title(title)
            .style(theme::panel())
            .border_style(theme::border(false))
            .title_style(theme::accent());
        let inner = block.inner(patch);
        let mut in_hunk = false;
        let lines: Vec<_> = file
            .patch
            .lines()
            .map(|line| {
                if line.starts_with("diff ") {
                    in_hunk = false;
                }
                let style = theme::diff_line(line, in_hunk);
                if line.starts_with("@@") {
                    in_hunk = true;
                }
                Line::styled(visible(line), style)
            })
            .collect();
        let max_scroll = lines
            .len()
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;
        let max_horizontal = lines
            .iter()
            .map(Line::width)
            .max()
            .unwrap_or(0)
            .saturating_sub(inner.width as usize)
            .min(u16::MAX as usize) as u16;
        self.scroll = self.scroll.min(max_scroll);
        self.horizontal = self.horizontal.min(max_horizontal);
        frame.render_widget(block, patch);
        frame.render_widget(
            Paragraph::new(lines)
                .style(theme::panel())
                .scroll((self.scroll, self.horizontal)),
            inner,
        );
    }
}

fn files(session: &ReviewSession, since_review: bool) -> &[ChangedFile] {
    if since_review {
        session.changes_since_review()
    } else {
        &session.diff.files
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{ChangeKind, ChangedFile, Engine, WorkItem, WorkItemDiff, WorkItemKind};

    fn diff(id: &str) -> WorkItemDiff {
        WorkItemDiff {
            work_item_id: id.into(), title: "Task".into(), workspace: "/work/task".into(),
            base: "HEAD".into(), base_revision: "a".repeat(40),
            files: ["source.rs", "λ\t\x1b.rs"].into_iter().map(|name| ChangedFile {
                path: name.into(), old_path: None, kind: ChangeKind::Modified,
                additions: Some(1), deletions: Some(1),
                patch: format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-before\n+after\n"),
            }).collect(),
        }
    }

    fn ready() -> ReviewUi {
        let mut review = ReviewUi::default();
        let ticket = review.open("A".into()).unwrap();
        review.finish_diff(ticket, Ok(diff("A")));
        review
    }

    fn key(review: &mut ReviewUi, code: KeyCode) -> ReviewIntent {
        let intent = review.key(KeyEvent::new(code, KeyModifiers::NONE), 24);
        // UI-only navigation fixtures acknowledge saves. Durable behavior is
        // tested against the real engine/store below and in the core.
        if let ReviewIntent::Save {
            ticket,
            mut session,
            path,
            reviewed,
        } = intent
        {
            let directory = tempfile::tempdir().unwrap();
            let engine = Engine::new(directory.path().join("items.json"));
            let already: Vec<_> = session
                .diff
                .files
                .iter()
                .filter(|file| session.is_reviewed(&file.path))
                .map(|file| file.path.clone())
                .collect();
            session.diff.workspace = std::fs::canonicalize(directory.path()).unwrap();
            engine
                .register_work_item(WorkItem {
                    id: session.diff.work_item_id.clone(),
                    title: "Test".into(),
                    repository: session.diff.workspace.clone(),
                    workspace: session.diff.workspace.clone(),
                    branch: None,
                    kind: WorkItemKind::Implementation,
                    pane_id: "%1".into(),
                })
                .unwrap();
            for file in already {
                engine.set_file_reviewed(&mut session, &file, true).unwrap();
            }
            engine
                .set_file_reviewed(&mut session, &path, reviewed)
                .unwrap();
            review.finish_save(ticket, Ok(*session));
            ReviewIntent::None
        } else {
            intent
        }
    }

    fn screen(review: &mut ReviewUi, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| review.render(frame)).unwrap();
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
    fn file_navigation_and_explicit_saves_restore_progress_on_reopen_and_reload() {
        let mut review = ready();
        key(&mut review, KeyCode::Char(' '));
        assert_eq!(review.sessions["A"].progress(), (1, 2));
        key(&mut review, KeyCode::Tab);
        assert_eq!(review.selected, 1);
        key(&mut review, KeyCode::Char(' '));
        assert_eq!(review.sessions["A"].progress(), (2, 2));
        key(&mut review, KeyCode::Char(' '));
        assert_eq!(review.sessions["A"].progress(), (1, 2));
        key(&mut review, KeyCode::Char('k'));
        assert_eq!(review.selected, 0);
        key(&mut review, KeyCode::Esc);
        assert!(!review.is_open());
        let restored = review.sessions["A"].clone();
        let ticket = review.open("A".into()).unwrap();
        assert!(review.loading);
        review.finish(ticket, Ok(restored.clone()));
        assert_eq!(review.sessions["A"].progress(), (1, 2));
        assert_eq!(key(&mut review, KeyCode::Char('r')), ReviewIntent::Reload);
        let (ticket, _) = review.reload().unwrap();
        assert!(!review.sessions.contains_key("A"));
        review.finish(ticket, Ok(restored));
        assert_eq!(review.sessions["A"].progress(), (1, 2));
    }

    #[test]
    fn asynchronous_results_cannot_redirect_review_after_back_or_reopening() {
        let mut review = ReviewUi::default();
        let old = review.open("A".into()).unwrap();
        key(&mut review, KeyCode::Esc);
        let current = review.open("B".into()).unwrap();
        review.finish_diff(old, Ok(diff("A")));
        assert!(review.loading);
        assert!(!review.sessions.contains_key("A"));
        review.finish_diff(current, Ok(diff("B")));
        assert_eq!(review.current.as_deref(), Some("B"));
        let (reload, _) = review.reload().unwrap();
        review.finish(reload, Err("Workspace removed".into()));
        assert!(!review.sessions.contains_key("B"));
        assert!(screen(&mut review, 80, 12).contains("Workspace removed"));
        assert!(!screen(&mut review, 80, 12).contains("reviewed •"));
        let (ticket, _) = review.reload().unwrap();
        review.finish_diff(ticket, Ok(diff("A")));
        assert!(!review.sessions.contains_key("A"));
        assert!(screen(&mut review, 80, 12).contains("different work item"));
        review.reload().unwrap();
        review.fail_loading("Capture task stopped".into());
        assert!(!review.loading);
        assert!(screen(&mut review, 80, 12).contains("Capture task stopped"));
    }

    #[test]
    fn review_renders_progress_unified_patch_and_sanitized_metadata_at_all_sizes() {
        let mut review = ready();
        key(&mut review, KeyCode::Char(' '));
        let text = screen(&mut review, 120, 24);
        for expected in [
            "REVIEW — A",
            "1 / 2 reviewed",
            "+2 -2",
            "source.rs",
            "λ\\t\\u{1b}.rs",
            "@@ -1 +1 @@",
            "-before",
            "+after",
            "Space: reviewed",
            "Esc: back",
            "Saved marks",
        ] {
            assert!(text.contains(expected), "missing {expected:?}: {text}");
        }
        assert!(!text.contains('\x1b'));
        for (width, height) in [(80, 10), (45, 12), (1, 1), (0, 0)] {
            screen(&mut review, width, height.max(1));
        }
        key(&mut review, KeyCode::Home);
        let narrow = screen(&mut review, 45, 12);
        assert!(narrow.contains("source.rs"));
        assert!(narrow.contains("diff --git"));
        review.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), 12);
        let scrolled = screen(&mut review, 45, 12);
        assert!(scrolled.contains("@@ -1 +1 @@"));
        assert!(scrolled.contains("+after"));
    }

    #[test]
    fn review_colors_marks_hunks_and_content_without_losing_selected_mark_color() {
        let mut review = ready();
        key(&mut review, KeyCode::Char(' '));
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| review.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        theme::assert_text_style(buffer, "AGENT WORKBENCH", theme::LAVENDER, theme::MANTLE);
        theme::assert_text_style(buffer, "✓ source.rs", theme::GREEN, theme::SURFACE);
        theme::assert_text_style(buffer, "+++ b/source.rs", theme::PINK, theme::MANTLE);
        theme::assert_text_style(buffer, "@@ -1 +1 @@", theme::PEACH, theme::MANTLE);
        theme::assert_text_style(
            buffer,
            "+after",
            theme::GREEN,
            ratatui::style::Color::Rgb(52, 64, 61),
        );
        theme::assert_text_style(
            buffer,
            "-before",
            theme::RED,
            ratatui::style::Color::Rgb(67, 47, 63),
        );
    }

    #[test]
    fn diff_scrolling_and_horizontal_pan_do_not_move_file_selection() {
        let mut review = ready();
        let file = &mut review.sessions.get_mut("A").unwrap().diff.files[0];
        file.patch = format!("{}\n", "very long code line ".repeat(50)).repeat(50);
        screen(&mut review, 80, 10);
        review.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), 10);
        assert_eq!(review.scroll, 3);
        review.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL), 10);
        assert_eq!(review.scroll, 0);
        key(&mut review, KeyCode::PageDown);
        key(&mut review, KeyCode::Char('l'));
        assert!(review.scroll > 0 && review.horizontal > 0);
        assert_eq!(review.selected, 0);
        key(&mut review, KeyCode::Char('j'));
        assert_eq!(
            (review.selected, review.scroll, review.horizontal),
            (1, 0, 0)
        );
        review.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL), 24);
        assert!(!review.loading);
    }

    #[test]
    fn loading_empty_and_binary_states_remain_navigable() {
        let mut review = ReviewUi::default();
        let ticket = review.open("A".into()).unwrap();
        assert!(screen(&mut review, 80, 12).contains("Loading local Git diff"));
        key(&mut review, KeyCode::Char(' '));
        let mut empty = diff("A");
        empty.files.clear();
        review.finish_diff(ticket, Ok(empty));
        assert!(screen(&mut review, 100, 12).contains("No changes against this base"));
        key(&mut review, KeyCode::Tab);
        key(&mut review, KeyCode::Char(' '));
        let (ticket, _) = review.reload().unwrap();
        let mut binary = diff("A");
        binary.files[0].additions = None;
        binary.files[0].deletions = None;
        binary.files[0].patch = "Binary files a/source.rs and b/source.rs differ\n".into();
        review.finish_diff(ticket, Ok(binary));
        assert!(screen(&mut review, 160, 12).contains("binary; external viewer needed"));
    }

    #[test]
    fn pending_failed_and_stale_saves_never_claim_an_unsaved_mark() {
        let mut review = ready();
        let intent = review.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), 24);
        let ReviewIntent::Save {
            ticket,
            session,
            path,
            reviewed,
        } = intent
        else {
            panic!("expected save intent")
        };
        assert!(reviewed);
        assert_eq!(path, PathBuf::from("source.rs"));
        assert_eq!(session.progress(), (0, 2));
        assert!(review.is_saving());
        assert_eq!(review.sessions["A"].progress(), (0, 2));
        for code in [KeyCode::Char(' '), KeyCode::Char('r'), KeyCode::Esc] {
            assert_eq!(key(&mut review, code), ReviewIntent::None);
        }
        assert!(review.is_open());
        assert!(screen(&mut review, 120, 24).contains("Saving review mark"));
        review.finish_save(
            ticket,
            Err("State directory is read-only; no mark changed".into()),
        );
        assert!(!review.is_saving());
        assert_eq!(review.sessions["A"].progress(), (0, 2));
        let text = screen(&mut review, 120, 24);
        assert!(text.contains("read-only"));
        assert!(text.contains("diff --git"));
        key(&mut review, KeyCode::Esc);
        let newer = review.open("B".into()).unwrap();
        review.finish_save(ticket, Ok(*session));
        assert!(review.loading);
        assert_eq!(review.current.as_deref(), Some("B"));
        review.finish_diff(newer, Ok(diff("B")));
        review.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), 24);
        review.fail_saving("Save task stopped".into());
        assert!(!review.saving);
        assert!(screen(&mut review, 120, 24).contains("Save task stopped"));
    }

    #[test]
    fn changed_after_review_is_visible_and_space_requests_reviewing_the_new_capture() {
        let mut review = ready();
        key(&mut review, KeyCode::Char(' '));
        review.sessions.get_mut("A").unwrap().diff.files[0]
            .patch
            .push_str("+new agent edit\n");
        let text = screen(&mut review, 120, 24);
        assert!(text.contains("⚠ source.rs"));
        assert!(text.contains("1 changed after review"));
        assert!(text.contains("0 / 2 reviewed"));
        let intent = review.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), 24);
        assert!(matches!(intent, ReviewIntent::Save { reviewed: true, .. }));
    }

    async fn reviewed_fixture() -> (tempfile::TempDir, Engine) {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        // Synthetic fixture commits use isolated config, never the user's
        // signing key or hooks. Actual project commits remain signed.
        for args in [
            vec!["init", "-q"],
            vec!["add", "--", "source.txt"],
            vec!["commit", "-q", "-m", "fixture"],
        ] {
            if args[0] == "add" {
                std::fs::write(workspace.join("source.txt"), "base\n").unwrap();
            }
            let result = std::process::Command::new("git")
                .current_dir(&workspace)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let workspace = std::fs::canonicalize(workspace).unwrap();
        let engine = Engine::new(directory.path().join("items.json"));
        engine
            .register_work_item(WorkItem {
                id: "A".into(),
                title: "Task".into(),
                repository: workspace.clone(),
                workspace: workspace.clone(),
                branch: None,
                kind: WorkItemKind::Implementation,
                pane_id: "%1".into(),
            })
            .unwrap();
        std::fs::write(
            workspace.join("source.txt"),
            "reviewed implementation\nfix this\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("reviewed-new.txt"),
            "new reviewed implementation\n",
        )
        .unwrap();
        let mut session = engine.open_review("A", None).await.unwrap();
        for path in ["source.txt", "reviewed-new.txt"] {
            engine
                .set_file_reviewed(&mut session, std::path::Path::new(path), true)
                .unwrap();
        }
        (directory, engine)
    }

    fn save_with_engine(ui: &mut ReviewUi, engine: &Engine) {
        let ReviewIntent::Save {
            ticket,
            mut session,
            path,
            reviewed,
        } = ui.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), 24)
        else {
            panic!("expected save");
        };
        engine
            .set_file_reviewed(&mut session, &path, reviewed)
            .unwrap();
        ui.finish_save(ticket, Ok(*session));
    }

    #[tokio::test]
    async fn correction_view_filters_counts_toggles_and_saves_real_snapshots() {
        let (directory, engine) = reviewed_fixture().await;
        let workspace = directory.path().join("workspace");
        std::fs::write(
            workspace.join("source.txt"),
            "reviewed implementation\ncorrected\n",
        )
        .unwrap();
        std::fs::write(workspace.join("never-reviewed.txt"), "brand new\n").unwrap();
        let mut ui = ReviewUi::default();
        let ticket = ui.open("A".into()).unwrap();
        ui.finish(ticket, Ok(engine.open_review("A", None).await.unwrap()));
        assert!(ui.since_review);
        let text = screen(&mut ui, 120, 24);
        for expected in [
            "RE-REVIEW REQUIRED",
            "2 lines (+1 -1)",
            "+1 -1 since review",
            "-fix this",
            "+corrected",
            "c: full/since",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(!text.contains("never-reviewed.txt"));
        assert!(!text.contains("-base"));
        for (w, h) in [(80, 12), (45, 12), (1, 1)] {
            screen(&mut ui, w, h);
        }
        key(&mut ui, KeyCode::Char('c'));
        assert!(!ui.since_review);
        assert!(screen(&mut ui, 120, 24).contains("○ never-reviewed.txt"));
        key(&mut ui, KeyCode::Char('c'));
        assert_eq!((ui.selected, ui.scroll, ui.horizontal), (0, 0, 0));
        save_with_engine(&mut ui, &engine);
        assert!(screen(&mut ui, 120, 24).contains("No changes since review"));
        key(&mut ui, KeyCode::Char('c'));
        assert!(screen(&mut ui, 120, 24).contains("✓ source.txt"));
        let (ticket, _) = ui.reload().unwrap();
        ui.finish(ticket, Ok(engine.open_review("A", None).await.unwrap()));
        assert!(!ui.since_review);
    }

    #[tokio::test]
    async fn removed_files_remain_navigable_when_full_diff_is_empty() {
        let (directory, engine) = reviewed_fixture().await;
        let workspace = directory.path().join("workspace");
        std::fs::write(workspace.join("source.txt"), "base\n").unwrap();
        std::fs::remove_file(workspace.join("reviewed-new.txt")).unwrap();
        let mut ui = ReviewUi::default();
        let ticket = ui.open("A".into()).unwrap();
        ui.finish(ticket, Ok(engine.open_review("A", None).await.unwrap()));
        assert!(ui.sessions["A"].diff.files.is_empty());
        assert!(screen(&mut ui, 120, 24).contains("2 changed after review"));
        key(&mut ui, KeyCode::Char('j'));
        assert_eq!(ui.selected, 1);
        save_with_engine(&mut ui, &engine);
        assert_eq!(ui.selected, 0);
        assert_eq!(ui.sessions["A"].changed_after_review(), 1);
        assert!(screen(&mut ui, 120, 24).contains("reviewed-new.txt"));
        save_with_engine(&mut ui, &engine);
        assert!(screen(&mut ui, 120, 24).contains("No changes since review"));
        key(&mut ui, KeyCode::Char('c'));
        assert!(screen(&mut ui, 120, 24).contains("No changes against this base"));
    }
}
