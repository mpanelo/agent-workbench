//! Go highlighting of captured source, never live workspace files or raw ANSI.
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use workbench_core::{ChangedFile, Engine, ReviewSession, ReviewText};

use crate::{theme, ui::visible};

const MAX_BYTES: usize = 512 * 1024;
const MAX_SPANS: usize = 100_000;
const NAMES: &[&str] = &[
    "keyword",
    "function",
    "function.builtin",
    "type",
    "property",
    "variable",
    "variable.parameter",
    "string",
    "escape",
    "number",
    "constant",
    "operator",
    "comment",
];

#[derive(Default)]
pub(crate) struct SyntaxUi {
    key: Option<(u64, PathBuf, bool)>,
    ticket: u64,
    requested: bool,
    cancel: Arc<AtomicUsize>,
    lines: Option<Vec<Line<'static>>>,
}

pub(crate) struct Request {
    pub ticket: u64,
    session: ReviewSession,
    file: ChangedFile,
    since_review: bool,
    cancel: Arc<AtomicUsize>,
}

impl SyntaxUi {
    pub fn clear(&mut self) {
        if self.key.take().is_none() {
            return;
        }
        self.cancel.store(1, Ordering::Relaxed);
        self.cancel = Arc::default();
        self.lines = None;
        self.requested = false;
        self.ticket += 1;
    }

    pub fn request(
        &mut self,
        generation: u64,
        session: &ReviewSession,
        file: &ChangedFile,
        since_review: bool,
    ) -> Option<Request> {
        let key = (generation, file.path.clone(), since_review);
        if self.key.as_ref() != Some(&key) {
            self.clear();
            self.key = Some(key);
        }
        if self.requested {
            return None;
        }
        self.requested = true;
        if file
            .path
            .extension()
            .is_none_or(|extension| extension != "go")
            || file.additions.is_none()
            || file.patch.len() > MAX_BYTES
        {
            return None;
        }
        Some(Request {
            ticket: self.ticket,
            session: session.clone(),
            file: file.clone(),
            since_review,
            cancel: Arc::clone(&self.cancel),
        })
    }

    pub fn finish(&mut self, ticket: u64, lines: Option<Vec<Line<'static>>>) {
        if ticket == self.ticket {
            self.lines = lines;
        }
    }

    pub fn lines(
        &self,
        generation: u64,
        path: &Path,
        since_review: bool,
    ) -> Option<&[Line<'static>]> {
        let key = self.key.as_ref()?;
        (key.0 == generation && key.1 == path && key.2 == since_review)
            .then_some(self.lines.as_deref())
            .flatten()
    }
}

impl Drop for SyntaxUi {
    fn drop(&mut self) {
        self.cancel.store(1, Ordering::Relaxed);
    }
}

impl Request {
    pub async fn run(self, engine: &Engine) -> Option<Vec<Line<'static>>> {
        if self.cancel.load(Ordering::Relaxed) != 0 {
            return None;
        }
        let source = engine
            .review_text(&self.session, &self.file.path, self.since_review)
            .await
            .ok()?;
        if self.cancel.load(Ordering::Relaxed) != 0 {
            return None;
        }
        tokio::task::spawn_blocking(move || highlight(&self.file.patch, &source, &self.cancel))
            .await
            .ok()
            .flatten()
    }
}

fn config() -> Option<&'static HighlightConfiguration> {
    static GO: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    GO.get_or_init(|| {
        // Later patterns override the bundled query's broad identifier captures.
        let query = format!(
            "{}\n{}",
            tree_sitter_go::HIGHLIGHTS_QUERY,
            include_str!("go.scm")
        );
        let mut config =
            HighlightConfiguration::new(tree_sitter_go::LANGUAGE.into(), "go", &query, "", "")
                .ok()?;
        config.configure(NAMES);
        Some(config)
    })
    .as_ref()
}

fn color(index: usize) -> Color {
    match NAMES[index] {
        "keyword" => theme::MAUVE,
        "function" => theme::BLUE,
        "function.builtin" => theme::PEACH,
        "type" => theme::YELLOW,
        "property" => theme::LAVENDER,
        "variable.parameter" => theme::PEACH,
        "string" => theme::GREEN,
        "escape" => theme::PINK,
        "number" | "constant" => theme::PEACH,
        "operator" => theme::TEAL,
        "comment" => theme::BORDER,
        _ => theme::TEXT,
    }
}

struct Token {
    range: Range<usize>,
    color: Color,
}

fn tokens(source: &str, cancel: &AtomicUsize) -> Option<Vec<Token>> {
    if source.len() > MAX_BYTES || cancel.load(Ordering::Relaxed) != 0 {
        return None;
    }
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config()?, source.as_bytes(), Some(cancel), |_| None)
        .ok()?;
    let mut stack = Vec::new();
    let mut tokens = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if tokens.len() >= MAX_SPANS {
                    return None;
                }
                tokens.push(Token {
                    range: start..end,
                    color: stack.last().map_or(theme::TEXT, |&index| color(index)),
                });
            }
        }
    }
    Some(tokens)
}

struct Source<'a> {
    lines: Vec<&'a str>,
    offsets: Vec<usize>,
    tokens: Vec<Token>,
}

impl<'a> Source<'a> {
    fn new(source: &'a str, cancel: &AtomicUsize) -> Option<Self> {
        let mut offset = 0;
        let offsets = source
            .split_inclusive('\n')
            .map(|line| {
                let start = offset;
                offset += line.len();
                start
            })
            .collect();
        Some(Self {
            lines: source.lines().collect(),
            offsets,
            tokens: tokens(source, cancel)?,
        })
    }

    fn line(&self, index: usize, text: &str, marker: char, style: Style) -> Option<Line<'static>> {
        if *self.lines.get(index)? != text {
            return None;
        }
        let start = *self.offsets.get(index)?;
        let end = start + text.len();
        let mut spans = vec![Span::styled(marker.to_string(), style)];
        let mut column = 0;
        let first = self
            .tokens
            .partition_point(|token| token.range.end <= start);
        for token in self.tokens[first..]
            .iter()
            .take_while(|token| token.range.start < end)
        {
            let left = token.range.start.max(start) - start;
            let right = token.range.end.min(end) - start;
            let text = text.get(left..right)?;
            let mut displayed = String::new();
            for (index, part) in text.split('\t').enumerate() {
                if index > 0 {
                    let spaces = 4 - column % 4;
                    displayed.extend(std::iter::repeat_n(' ', spaces));
                    column += spaces;
                }
                let part = visible(part);
                column += Line::from(part.as_str()).width();
                displayed.push_str(&part);
            }
            spans.push(Span::styled(displayed, style.fg(token.color)));
        }
        Some(Line::from(spans).style(style))
    }
}

/// Validate every mapped line. Any mismatch falls back to the original diff.
fn highlight(patch: &str, source: &ReviewText, cancel: &AtomicUsize) -> Option<Vec<Line<'static>>> {
    if patch.len() > MAX_BYTES {
        return None;
    }
    let before = Source::new(&source.before, cancel)?;
    let after = Source::new(&source.after, cancel)?;
    let (mut old, mut new) = (0, 0);
    let mut remaining = None;
    let mut lines = Vec::new();
    for line in patch.lines() {
        if cancel.load(Ordering::Relaxed) != 0 {
            return None;
        }
        if line.starts_with("diff ") {
            if remaining.is_some_and(|counts| counts != (0, 0)) {
                return None;
            }
            remaining = None;
        }
        if let Some(header) = line.strip_prefix("@@ -") {
            if remaining.is_some_and(|counts| counts != (0, 0)) {
                return None;
            }
            let (left, rest) = header.split_once(" +")?;
            let (right, _) = rest.split_once(" @@")?;
            let left = range(left)?;
            let right = range(right)?;
            old = left.0;
            new = right.0;
            remaining = Some((left.1, right.1));
        }
        let in_hunk = remaining.is_some();
        let style = theme::diff_line(line, in_hunk);
        let rendered = match (remaining.as_mut(), line.as_bytes().first()) {
            (Some((old_count, new_count)), Some(b' ' | b'+' | b'-')) => {
                let marker = line.chars().next()?;
                let text = &line[1..];
                let rendered = if marker == '-' {
                    before.line(old, text, marker, style)?
                } else {
                    after.line(new, text, marker, style)?
                };
                if marker != '+' {
                    if before.lines.get(old).copied() != Some(text) {
                        return None;
                    }
                    *old_count = old_count.checked_sub(1)?;
                    old += 1;
                }
                if marker != '-' {
                    *new_count = new_count.checked_sub(1)?;
                    new += 1;
                }
                rendered
            }
            _ => Line::styled(crate::review::patch_text(line, in_hunk), style),
        };
        lines.push(rendered);
    }
    if remaining.is_some_and(|counts| counts != (0, 0)) {
        return None;
    }
    Some(lines)
}

fn range(value: &str) -> Option<(usize, usize)> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let start = start.parse::<usize>().ok()?;
    let count = count.parse().ok()?;
    if start == 0 && count != 0 {
        return None;
    }
    Some((start.saturating_sub(1), count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use workbench_core::{ChangeKind, WorkItemDiff};

    fn source() -> ReviewText {
        ReviewText {
            before: "package demo\n\nfunc Before(value int) int {\n\treturn value + 1\n}\n".into(),
            after: "package demo\n\nfunc After(value int) int {\n\treturn value + 1\n}\n".into(),
        }
    }

    fn file() -> ChangedFile {
        ChangedFile {
            path: "demo.go".into(), old_path: None, kind: ChangeKind::Modified,
            additions: Some(1), deletions: Some(1),
            patch: "diff --git a/demo.go b/demo.go\n--- a/demo.go\n+++ b/demo.go\n@@ -1,5 +1,5 @@\n package demo\n \n-func Before(value int) int {\n+func After(value int) int {\n \treturn value + 1\n }\n".into(),
        }
    }

    fn session() -> ReviewSession {
        ReviewSession::new(WorkItemDiff {
            work_item_id: "A".into(),
            title: String::new(),
            workspace: "/missing".into(),
            base: "HEAD".into(),
            base_revision: "a".repeat(40),
            files: vec![file()],
        })
    }

    fn foreground(line: &Line<'_>, text: &str) -> Color {
        line.spans
            .iter()
            .find(|span| span.content.contains(text))
            .unwrap()
            .style
            .fg
            .unwrap()
    }

    #[test]
    fn test_go_colors() {
        let file = file();
        let lines = highlight(&file.patch, &source(), &AtomicUsize::new(0)).unwrap();
        let removed = &lines[6];
        let added = &lines[7];
        assert_eq!(foreground(removed, "Before"), theme::BLUE);
        assert_eq!(foreground(added, "After"), theme::BLUE);
        assert_eq!(foreground(added, "func"), theme::MAUVE);
        assert_eq!(foreground(added, "int"), theme::YELLOW);
        assert_eq!(foreground(added, "value"), theme::PEACH);
        assert_eq!(foreground(&lines[8], "value"), theme::TEXT);
        assert_eq!(foreground(&lines[8], "1"), theme::PEACH);
        for (line, patch) in [(removed, "-removed"), (added, "+added")] {
            let style = theme::diff_line(patch, true);
            assert!(line.spans.iter().all(|span| span.style.bg == style.bg));
            assert_eq!(line.spans[0].style.fg, style.fg);
        }
        for (displayed, original) in lines.iter().zip(file.patch.lines()) {
            assert_eq!(
                displayed.to_string(),
                crate::review::patch_text(original, true)
            );
        }
    }

    #[test]
    fn test_go_methods() {
        let code = "package demo\ntype Item struct { Value int }\nfunc (item Item) Read() int { return item.Value }\nfunc Use() { println(\"hello\", item.Read()) }\n";
        let patch = format!(
            "@@ -0,0 +1,4 @@\n{}",
            code.lines()
                .map(|line| format!("+{line}\n"))
                .collect::<String>()
        );
        let lines = highlight(
            &patch,
            &ReviewText {
                before: String::new(),
                after: code.into(),
            },
            &AtomicUsize::new(0),
        )
        .unwrap();
        assert_eq!(foreground(&lines[2], "Item"), theme::YELLOW);
        assert_eq!(foreground(&lines[2], "Value"), theme::LAVENDER);
        assert_eq!(foreground(&lines[3], "Read"), theme::BLUE);
        assert_eq!(foreground(&lines[4], "println"), theme::PEACH);
        assert_eq!(foreground(&lines[4], "hello"), theme::GREEN);
        assert_eq!(foreground(&lines[4], "Read"), theme::BLUE);
    }

    #[test]
    fn test_multiline_context() {
        for (open, close, expected) in [
            ("var text = `", "`", theme::GREEN),
            ("/*", "*/", theme::BORDER),
        ] {
            let before = format!(
                "package demo\n{open}\na\nb\nc\nfunc NotAFunction() {{}}\nd\ne\nf\n{close}\n"
            );
            let after = before.replace("NotAFunction", "StillNotCode");
            let patch = "@@ -3,7 +3,7 @@\n a\n b\n c\n-func NotAFunction() {}\n+func StillNotCode() {}\n d\n e\n f\n";
            let lines =
                highlight(patch, &ReviewText { before, after }, &AtomicUsize::new(0)).unwrap();
            assert_eq!(foreground(&lines[4], "NotAFunction"), expected);
            assert_eq!(foreground(&lines[5], "StillNotCode"), expected);
        }
    }

    #[test]
    fn test_display_safety() {
        for ending in ["\n", "\r\n", ""] {
            let code = format!("package demo\nvar café = \"界\\t\x1b[2J\"{ending}");
            let patch =
                format!("@@ -0,0 +1,2 @@\n+package demo\n+var café = \"界\\t\x1b[2J\"{ending}");
            let lines = highlight(
                &patch,
                &ReviewText {
                    before: String::new(),
                    after: code,
                },
                &AtomicUsize::new(0),
            )
            .unwrap();
            assert_eq!(foreground(&lines[2], "界"), theme::GREEN);
            assert!(lines[2].to_string().contains("界\\t\\u{1b}[2J"));
            assert!(
                lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .all(|span| !span.content.chars().any(char::is_control))
            );
        }
        let code = "package demo\nfunc 界(value int) {\n\tvalue\t+=\t1\n}\n";
        let patch = format!(
            "@@ -0,0 +1,4 @@\n{}",
            code.lines()
                .map(|line| format!("+{line}\n"))
                .collect::<String>()
        );
        let lines = highlight(
            &patch,
            &ReviewText {
                before: String::new(),
                after: code.into(),
            },
            &AtomicUsize::new(0),
        )
        .unwrap();
        for (line, original) in lines.iter().zip(patch.lines()) {
            assert_eq!(line.to_string(), crate::review::patch_text(original, true));
        }
    }

    #[test]
    fn test_hunk_mapping() {
        let source = ReviewText {
            before: "package old\n\nvar a = 1\n\nvar b = 2\n".into(),
            after: "package new\n\nvar a = 3\n\nvar b = 4\n".into(),
        };
        let patch = "@@ -1 +1 @@\n-package old\n+package new\n@@ -3 +3 @@\n-var a = 1\n+var a = 3\n@@ -5 +5 @@\n-var b = 2\n+var b = 4\n";
        assert!(highlight(patch, &source, &AtomicUsize::new(0)).is_some());
        let source = ReviewText {
            before: "package old".into(),
            after: "package new".into(),
        };
        let patch = "diff --git a/a.go b/a.go\n@@ -1 +0,0 @@\n-package old\n\\ No newline at end of file\ndiff --git a/a.go b/a.go\n@@ -0,0 +1 @@\n+package new\n\\ No newline at end of file\n";
        assert!(highlight(patch, &source, &AtomicUsize::new(0)).is_some());
    }

    #[test]
    fn test_fallback() {
        let file = file();
        let cancel = AtomicUsize::new(0);
        assert!(highlight(&file.patch.replace("Before", "Wrong"), &source(), &cancel).is_none());
        assert!(highlight(&file.patch.replace("-1,5", "-1,6"), &source(), &cancel).is_none());
        let large = ReviewText {
            before: " ".repeat(MAX_BYTES + 1),
            after: String::new(),
        };
        assert!(highlight(&file.patch, &large, &cancel).is_none());
        assert!(highlight(&" ".repeat(MAX_BYTES + 1), &source(), &cancel).is_none());
        cancel.store(1, Ordering::Relaxed);
        assert!(highlight(&file.patch, &source(), &cancel).is_none());
    }

    #[test]
    fn test_large_go() {
        let mut before = String::from("package demo\n");
        for index in 0..2000 {
            before.push_str(&format!(
                "func Function{index}(value int) int {{ return value + {index} }}\n"
            ));
        }
        let after = before.replace("return value + 1999", "return value + 2000");
        let patch = "@@ -2001 +2001 @@\n-func Function1999(value int) int { return value + 1999 }\n+func Function1999(value int) int { return value + 2000 }\n";
        let lines = highlight(patch, &ReviewText { before, after }, &AtomicUsize::new(0)).unwrap();
        assert_eq!(foreground(&lines[2], "Function1999"), theme::BLUE);
    }

    #[test]
    fn test_cache() {
        let mut cache = SyntaxUi::default();
        let session = session();
        let file = &session.diff.files[0];
        let first = cache.request(1, &session, file, false).unwrap();
        assert!(cache.request(1, &session, file, false).is_none());
        cache.finish(first.ticket, Some(vec![Line::from("first")]));
        assert!(cache.lines(1, &file.path, false).is_some());
        assert!(cache.lines(1, Path::new("different.go"), false).is_none());
        assert!(cache.lines(1, &file.path, true).is_none());
        let second = cache.request(1, &session, file, true).unwrap();
        assert_eq!(first.cancel.load(Ordering::Relaxed), 1);
        cache.finish(first.ticket, Some(vec![Line::from("stale")]));
        assert!(cache.lines(1, &file.path, true).is_none());
        cache.finish(second.ticket, None);
        assert!(cache.request(1, &session, file, true).is_none());
        let third = cache.request(2, &session, file, true).unwrap();
        assert_eq!(second.cancel.load(Ordering::Relaxed), 1);
        cache.clear();
        cache.finish(third.ticket, Some(vec![Line::from("closed")]));
        assert!(cache.lines(2, &file.path, true).is_none());
        assert_eq!(third.cancel.load(Ordering::Relaxed), 1);
        for path in ["demo.rs", "demo.py", "demo"] {
            let mut unsupported = file.clone();
            unsupported.path = path.into();
            assert!(cache.request(2, &session, &unsupported, false).is_none());
        }
    }
}
