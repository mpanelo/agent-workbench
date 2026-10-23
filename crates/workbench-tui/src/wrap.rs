//! Display-only wrapping. Preserve styled graphemes and indentation, not patches.
use std::collections::VecDeque;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

fn rows<'a>(lines: &'a [Line<'_>], width: u16) -> impl Iterator<Item = Line<'static>> + 'a {
    lines.iter().flat_map(move |line| {
        let mut graphemes = line.styled_graphemes(Style::default()).peekable();
        let mut first = width > 0;
        std::iter::from_fn(move || {
            if width == 0 || (!first && graphemes.peek().is_none()) {
                return None;
            }
            first = false;
            let mut row = Line::default().style(line.style);
            row.alignment = line.alignment;
            let mut columns = 0;
            while let Some(grapheme) = graphemes.peek() {
                let cells = Span::raw(grapheme.symbol).width();
                if columns > 0 && columns + cells > usize::from(width) {
                    break;
                }
                // A double-width glyph cannot fit a one-cell viewport. Make
                // that explicit rather than losing it or corrupting a border.
                let symbol = if cells > usize::from(width) {
                    "�"
                } else {
                    grapheme.symbol
                };
                columns += cells.min(usize::from(width));
                if let Some(span) = row
                    .spans
                    .last_mut()
                    .filter(|span| span.style == grapheme.style)
                {
                    span.content.to_mut().push_str(symbol);
                } else {
                    row.spans
                        .push(Span::styled(symbol.to_owned(), grapheme.style));
                }
                graphemes.next();
            }
            Some(row)
        })
    })
}

/// Keep only a viewport and a small tail while counting wrapped rows. Even a
/// narrow view of a large patch must not allocate every visual row at once.
pub(crate) fn viewport(
    lines: &[Line<'_>],
    width: u16,
    height: u16,
    offset: usize,
) -> (Vec<Line<'static>>, usize) {
    if width == 0 || height == 0 {
        return (Vec::new(), 0);
    }
    let height = usize::from(height);
    let mut selected = Vec::new();
    let mut tail = VecDeque::new();
    let mut count = 0;
    for row in rows(lines, width) {
        if count >= offset && selected.len() < height {
            selected.push(row.clone());
        }
        tail.push_back(row);
        if tail.len() > height {
            tail.pop_front();
        }
        count += 1;
    }
    let clamped = offset.min(count.saturating_sub(height));
    if clamped == offset {
        (selected, offset)
    } else {
        (tail.into(), clamped)
    }
}

pub(crate) fn tail(lines: &[Line<'_>], width: u16, height: u16) -> Vec<Line<'static>> {
    // Each source line occupies at least one visual row. Earlier lines cannot
    // contribute to the bottom viewport, however much these last lines wrap.
    let start = lines.len().saturating_sub(usize::from(height));
    viewport(&lines[start..], width, height, usize::MAX).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn test_styles() {
        let line = Line::from(vec![
            Span::styled("  abc", Style::default().fg(Color::Red)),
            Span::styled("defgh", Style::default().fg(Color::Blue)),
        ])
        .style(Style::default().bg(Color::Black));
        let (rows, offset) = viewport(&[line], 4, 10, 0);
        assert_eq!(offset, 0);
        assert_eq!(
            rows.iter().map(Line::to_string).collect::<Vec<_>>(),
            ["  ab", "cdef", "gh"]
        );
        assert!(rows.iter().all(|row| row.style.bg == Some(Color::Black)));
        assert_eq!(rows[1].spans[0].style.fg, Some(Color::Red));
        assert_eq!(rows[1].spans[1].style.fg, Some(Color::Blue));
        assert_eq!(
            rows.iter().map(Line::to_string).collect::<String>(),
            "  abcdefgh"
        );
    }

    #[test]
    fn test_unicode() {
        let source = "e\u{301}界👩‍💻 tail";
        let (rows, _) = viewport(&[Line::from(source)], 3, 10, 0);
        assert_eq!(rows.iter().map(Line::to_string).collect::<String>(), source);
        assert!(rows.iter().all(|row| row.width() <= 3));
        assert!(
            rows.iter()
                .flat_map(|row| &row.spans)
                .any(|span| span.content.contains("👩‍💻"))
        );
        assert_eq!(viewport(&[Line::from("界")], 1, 1, 0).0[0].to_string(), "�");
    }

    #[test]
    fn test_viewport() {
        let lines = [
            Line::from("abcdefghijkl"),
            Line::default(),
            Line::from("tail"),
        ];
        let (rows, offset) = viewport(&lines, 4, 2, 1);
        assert_eq!(offset, 1);
        assert_eq!(
            rows.iter().map(Line::to_string).collect::<Vec<_>>(),
            ["efgh", "ijkl"]
        );
        let (rows, offset) = viewport(&lines, 4, 2, usize::MAX);
        assert_eq!(offset, 3);
        assert_eq!(
            rows.iter().map(Line::to_string).collect::<Vec<_>>(),
            ["", "tail"]
        );
        assert_eq!(
            tail(&[Line::from("abcdefghijkl")], 4, 2)
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>(),
            ["efgh", "ijkl"]
        );
        for (width, height) in [(0, 0), (0, 1), (1, 0)] {
            assert!(viewport(&lines, width, height, 0).0.is_empty());
        }
        assert!(viewport(&[], 10, 2, 9).0.is_empty());
    }
}
