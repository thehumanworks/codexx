use std::borrow::Cow;

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TableCell {
    lines: Vec<Line<'static>>,
}

impl TableCell {
    pub(super) fn push_text(&mut self, text: &str, style: Style) {
        self.push_span(Span::styled(text.to_string(), style));
    }

    pub(super) fn push_span(&mut self, span: Span<'static>) {
        let content = span.content.to_string();
        for (index, segment) in content.split('\n').enumerate() {
            if index > 0 {
                self.push_line_break();
            }
            if segment.is_empty() {
                self.ensure_line();
                continue;
            }
            let mut segment_span = span.clone();
            segment_span.content = Cow::Owned(segment.to_string());
            self.ensure_line().push_span(segment_span);
        }
    }

    pub(super) fn push_line_break(&mut self) {
        self.lines.push(Line::default());
    }

    pub(super) fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(line_plain_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn trimmed_plain_text(&self) -> String {
        self.plain_text().trim().to_string()
    }

    pub(super) fn width(&self) -> usize {
        self.lines.iter().map(Line::width).max().unwrap_or(0)
    }

    pub(super) fn is_blank(&self) -> bool {
        self.lines
            .iter()
            .all(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
    }

    pub(super) fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    fn ensure_line(&mut self) -> &mut Line<'static> {
        if self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        let last_index = self.lines.len() - 1;
        &mut self.lines[last_index]
    }
}

fn line_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}
