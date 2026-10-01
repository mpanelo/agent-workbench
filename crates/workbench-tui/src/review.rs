use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, List, ListItem, ListState, Paragraph},
};
use workbench_core::{ReviewSession, WorkItemDiff};

use crate::ui::{display_path, visible};

#[derive(Default)]
pub(crate) struct ReviewUi {
    sessions: HashMap<String, ReviewSession>,
    current: Option<String>,
    generation: u64,
    loading: bool,
    error: Option<String>,
    selected: usize,
    scroll: u16,
    horizontal: u16,
    pub base: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReviewIntent {
    None,
    Reload,
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

    pub fn fail_loading(&mut self, error: String) {
        if self.is_open() && self.loading {
            self.finish(self.generation, Err(error));
        }
    }

    pub fn open(&mut self, id: String) -> Option<u64> {
        self.generation += 1;
        self.loading = !self.sessions.contains_key(&id);
        self.current = Some(id);
        self.error = None;
        self.selected = 0;
        self.scroll = 0;
        self.horizontal = 0;
        self.loading.then_some(self.generation)
    }

    pub fn reload(&mut self) -> Option<(u64, String)> {
        let id = self.current.clone()?;
        self.sessions.remove(&id);
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.selected = 0;
        self.scroll = 0;
        self.horizontal = 0;
        Some((self.generation, id))
    }

    pub fn finish(&mut self, ticket: u64, result: Result<WorkItemDiff, String>) {
        if !self.is_open() || ticket != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(diff) if self.current.as_deref() == Some(diff.work_item_id.as_str()) => {
                self.sessions
                    .insert(diff.work_item_id.clone(), ReviewSession::new(diff));
                self.error = None;
            }
            Ok(_) => {
                self.error = Some("Diff result belongs to a different work item; reload.".into())
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn key(&mut self, key: KeyEvent, height: u16) -> ReviewIntent {
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
        let Some(session) = self
            .current
            .as_ref()
            .and_then(|id| self.sessions.get_mut(id))
        else {
            return ReviewIntent::None;
        };
        let previous = self.selected;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(session.diff.files.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Tab if !session.diff.files.is_empty() => {
                if let Some(next) = (1..=session.diff.files.len())
                    .map(|step| (self.selected + step) % session.diff.files.len())
                    .find(|index| !session.is_reviewed(&session.diff.files[*index].path))
                {
                    self.selected = next;
                }
            }
            KeyCode::Char(' ') => {
                if let Some(file) = session.diff.files.get(self.selected) {
                    let path = file.path.clone();
                    session.toggle_reviewed(&path);
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
        let [header, summary, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        let id = self.current.as_deref().unwrap_or("—");
        frame.render_widget(
            Paragraph::new(format!("AGENT WORKBENCH — REVIEW — {}", visible(id)))
                .style(Style::default().fg(Color::Cyan)),
            header,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(
                    "j/k: files | Space: reviewed | Ctrl+d/u: scroll | h/l: pan | r: reload",
                ),
                Line::from("Tab: unreviewed | PgUp/Dn: page | Home: top | Esc: back | q: quit"),
            ]),
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
                    .style(Style::default().fg(Color::Red))
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                body,
            );
            return;
        }
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let (reviewed, total) = session.progress();
        let (added, deleted) = session.diff.line_totals();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    "{reviewed} / {total} reviewed • +{added} -{deleted} (text) • base {} ({})",
                    visible(&session.diff.base),
                    &session.diff.base_revision[..session.diff.base_revision.len().min(12)]
                )),
                Line::from(format!(
                    "Captured diff • reload resets marks • {}",
                    visible(&display_path(&session.diff.workspace))
                )),
            ]),
            summary,
        );
        if session.diff.files.is_empty() {
            frame.render_widget(
                Paragraph::new(
                    "No changes against this base (including untracked files). Esc: back.",
                ),
                body,
            );
            return;
        }
        self.selected = self.selected.min(session.diff.files.len() - 1);
        let [files, patch] = if body.width < 60 {
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
        let items: Vec<_> = session
            .diff
            .files
            .iter()
            .map(|file| {
                let mark = if session.is_reviewed(&file.path) {
                    "✓"
                } else {
                    "○"
                };
                ListItem::new(format!(
                    "{mark} {}  {}",
                    visible(&file.path.to_string_lossy()),
                    file.kind
                ))
            })
            .collect();
        frame.render_stateful_widget(
            List::new(items)
                .block(if files.height >= 3 {
                    Block::bordered().title("Changed files")
                } else {
                    Block::default()
                })
                .highlight_symbol("> ")
                .highlight_style(Style::default().fg(Color::Yellow)),
            files,
            &mut ListState::default().with_selected(Some(self.selected)),
        );
        let file = &session.diff.files[self.selected];
        let title = format!(
            "{} • {}",
            visible(&file.path.to_string_lossy()),
            if file.additions.is_none() {
                "binary; external viewer needed".into()
            } else {
                format!(
                    "+{} -{}",
                    file.additions.unwrap_or(0),
                    file.deletions.unwrap_or(0)
                )
            }
        );
        let block = Block::bordered().title(title);
        let inner = block.inner(patch);
        let lines: Vec<_> = file
            .patch
            .lines()
            .map(|line| {
                Line::styled(
                    visible(line),
                    Style::default().fg(if line.starts_with("@@") {
                        Color::Cyan
                    } else if line.starts_with('+') {
                        Color::Green
                    } else if line.starts_with('-') {
                        Color::Red
                    } else {
                        Color::Reset
                    }),
                )
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
            Paragraph::new(lines).scroll((self.scroll, self.horizontal)),
            inner,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use workbench_core::{ChangeKind, ChangedFile};

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
        review.finish(ticket, Ok(diff("A")));
        review
    }

    fn key(review: &mut ReviewUi, code: KeyCode) -> ReviewIntent {
        review.key(KeyEvent::new(code, KeyModifiers::NONE), 24)
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
    fn file_navigation_marking_and_back_preserve_only_in_memory_progress() {
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
        assert_eq!(review.open("A".into()), None);
        assert_eq!(review.sessions["A"].progress(), (1, 2));
        assert_eq!(key(&mut review, KeyCode::Char('r')), ReviewIntent::Reload);
        let (ticket, _) = review.reload().unwrap();
        assert!(!review.sessions.contains_key("A"));
        review.finish(ticket, Ok(diff("A")));
        assert_eq!(review.sessions["A"].progress(), (0, 2));
    }

    #[test]
    fn asynchronous_results_cannot_redirect_review_after_back_or_reopening() {
        let mut review = ReviewUi::default();
        let old = review.open("A".into()).unwrap();
        key(&mut review, KeyCode::Esc);
        let current = review.open("B".into()).unwrap();
        review.finish(old, Ok(diff("A")));
        assert!(review.loading);
        assert!(!review.sessions.contains_key("A"));
        review.finish(current, Ok(diff("B")));
        assert_eq!(review.current.as_deref(), Some("B"));
        let (reload, _) = review.reload().unwrap();
        review.finish(reload, Err("Workspace removed".into()));
        assert!(!review.sessions.contains_key("B"));
        assert!(screen(&mut review, 80, 12).contains("Workspace removed"));
        assert!(!screen(&mut review, 80, 12).contains("reviewed •"));
        let (ticket, _) = review.reload().unwrap();
        review.finish(ticket, Ok(diff("A")));
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
            "reload resets marks",
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
        review.finish(ticket, Ok(empty));
        assert!(screen(&mut review, 100, 12).contains("No changes against this base"));
        key(&mut review, KeyCode::Tab);
        key(&mut review, KeyCode::Char(' '));
        let (ticket, _) = review.reload().unwrap();
        let mut binary = diff("A");
        binary.files[0].additions = None;
        binary.files[0].deletions = None;
        binary.files[0].patch = "Binary files a/source.rs and b/source.rs differ\n".into();
        review.finish(ticket, Ok(binary));
        assert!(screen(&mut review, 160, 12).contains("binary; external viewer needed"));
    }
}
