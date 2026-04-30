use ratatui::style::Style;
use ratatui::text::Span;

use super::table_cell::TableCell;

#[derive(Debug, Default)]
pub(super) struct TableState {
    pub(super) rows: Vec<Vec<TableCell>>,
    current_row: Vec<TableCell>,
    current_cell: TableCell,
    in_cell: bool,
}

impl TableState {
    pub(super) fn start_row(&mut self) {
        self.current_row.clear();
    }

    pub(super) fn start_cell(&mut self) {
        self.current_cell = TableCell::default();
        self.in_cell = true;
    }

    pub(super) fn push_text(&mut self, text: &str, style: Style) {
        if self.in_cell {
            self.current_cell.push_text(text, style);
        }
    }

    pub(super) fn push_span(&mut self, span: Span<'static>) {
        if self.in_cell {
            self.current_cell.push_span(span);
        }
    }

    pub(super) fn push_html(&mut self, html: &str, style: Style) {
        let trimmed = html.trim();
        if matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "<br>" | "<br/>" | "<br />"
        ) {
            if self.in_cell {
                self.current_cell.push_line_break();
            }
        } else {
            self.push_text(html, style);
        }
    }

    pub(super) fn end_cell(&mut self) {
        self.current_row
            .push(std::mem::take(&mut self.current_cell));
        self.in_cell = false;
    }

    pub(super) fn end_row(&mut self) {
        if !self.current_row.is_empty() {
            self.rows.push(std::mem::take(&mut self.current_row));
        }
    }
}
