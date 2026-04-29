use std::borrow::Cow;

use unicode_width::UnicodeWidthStr;

#[derive(Debug, Default)]
pub(super) struct TableState {
    pub(super) rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_cell: bool,
}

impl TableState {
    pub(super) fn start_row(&mut self) {
        self.current_row.clear();
    }

    pub(super) fn start_cell(&mut self) {
        self.current_cell.clear();
        self.in_cell = true;
    }

    pub(super) fn push_text(&mut self, text: &str) {
        if self.in_cell {
            self.current_cell.push_str(text);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableLayoutMode {
    Normal,
    Compact,
}

pub(super) fn render_table_lines(rows: &[Vec<String>], width: Option<usize>) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }

    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count == 0 {
        return Vec::new();
    }

    let available_width = width.unwrap_or(usize::MAX / 4).max(1);
    let normalized_rows = normalize_table_rows(rows, column_count);
    let widths = desired_column_widths(&normalized_rows, column_count);

    let layout = choose_table_layout(&widths, available_width, column_count);
    match layout {
        Some((mode, column_widths, padding)) => {
            render_box_table(&normalized_rows, &column_widths, padding, mode)
        }
        None => render_vertical_table(&normalized_rows, available_width),
    }
}

pub(super) fn normalize_table_boundaries(input: &str) -> Cow<'_, str> {
    if !input.contains('|') {
        return Cow::Borrowed(input);
    }

    let lines = input.split_inclusive('\n').collect::<Vec<_>>();
    let mut out = String::with_capacity(input.len());
    let mut changed = false;
    let mut index = 0;
    while index < lines.len() {
        if index + 1 < lines.len()
            && is_table_row_source(lines[index])
            && is_table_delimiter_source(lines[index + 1])
        {
            out.push_str(lines[index]);
            out.push_str(lines[index + 1]);
            index += 2;

            while index < lines.len() && is_table_row_source(lines[index]) {
                out.push_str(lines[index]);
                index += 1;
            }

            if index < lines.len() && !lines[index].trim().is_empty() {
                out.push('\n');
                changed = true;
            }
        } else {
            out.push_str(lines[index]);
            index += 1;
        }
    }

    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(input)
    }
}

fn is_table_row_source(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.contains('|')
}

fn is_table_delimiter_source(line: &str) -> bool {
    let trimmed = line.trim().trim_matches('|').trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.split('|').all(|cell| {
        let cell = cell.trim();
        let dash_count = cell.chars().filter(|ch| *ch == '-').count();
        dash_count >= 3 && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
    })
}

fn normalize_table_rows(rows: &[Vec<String>], column_count: usize) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            let mut normalized = row.clone();
            normalized.resize(column_count, String::new());
            normalized
        })
        .collect()
}

fn desired_column_widths(rows: &[Vec<String>], column_count: usize) -> Vec<usize> {
    let mut widths = vec![3; column_count];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.width());
        }
    }
    widths
}

fn choose_table_layout(
    desired_widths: &[usize],
    available_width: usize,
    column_count: usize,
) -> Option<(TableLayoutMode, Vec<usize>, usize)> {
    if available_width < 20 {
        return None;
    }

    if let Some(widths) = allocate_table_widths(
        desired_widths,
        available_width,
        column_count,
        /*padding*/ 1,
    ) {
        return Some((TableLayoutMode::Normal, widths, 1));
    }

    allocate_table_widths(
        desired_widths,
        available_width,
        column_count,
        /*padding*/ 0,
    )
    .map(|widths| (TableLayoutMode::Compact, widths, 0))
}

fn allocate_table_widths(
    desired_widths: &[usize],
    available_width: usize,
    column_count: usize,
    padding: usize,
) -> Option<Vec<usize>> {
    let border_width = column_count + 1;
    let padding_width = padding * 2 * column_count;
    let available_content_width = available_width.checked_sub(border_width + padding_width)?;
    let min_total = 3 * column_count;
    if available_content_width < min_total {
        return None;
    }

    let mut widths = vec![3; column_count];
    let mut remaining = available_content_width - min_total;
    while remaining > 0 {
        let mut changed = false;
        for index in 0..column_count {
            if remaining == 0 {
                break;
            }
            if widths[index] < desired_widths[index] {
                widths[index] += 1;
                remaining -= 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    Some(widths)
}

fn render_box_table(
    rows: &[Vec<String>],
    column_widths: &[usize],
    padding: usize,
    _mode: TableLayoutMode,
) -> Vec<String> {
    let mut rendered =
        render_box_table_with_wrap(rows, column_widths, padding, /*hard_wrap*/ false);
    let available_width = table_total_width(column_widths, padding);
    let needs_hard_wrap = rendered.iter().any(|line| line.width() > available_width)
        || any_row_too_tall(rows, column_widths, padding, /*hard_wrap*/ false);

    if needs_hard_wrap {
        rendered =
            render_box_table_with_wrap(rows, column_widths, padding, /*hard_wrap*/ true);
        if any_row_too_tall(rows, column_widths, padding, /*hard_wrap*/ true) {
            return render_vertical_table(rows, available_width);
        }
    }

    rendered
}

fn render_box_table_with_wrap(
    rows: &[Vec<String>],
    column_widths: &[usize],
    padding: usize,
    hard_wrap: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    out.push(border_line("┌", "┬", "┐", column_widths, padding));

    for (index, row) in rows.iter().enumerate() {
        out.extend(render_table_row(row, column_widths, padding, hard_wrap));
        if index == 0 {
            out.push(border_line("├", "┼", "┤", column_widths, padding));
        }
    }

    out.push(border_line("└", "┴", "┘", column_widths, padding));
    out
}

fn render_table_row(
    row: &[String],
    column_widths: &[usize],
    padding: usize,
    hard_wrap: bool,
) -> Vec<String> {
    let wrapped_cells = row
        .iter()
        .zip(column_widths)
        .map(|(cell, width)| wrap_table_cell(cell, *width, hard_wrap))
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut out = Vec::with_capacity(row_height);

    for line_index in 0..row_height {
        let mut line = String::from("│");
        for (cell_lines, width) in wrapped_cells.iter().zip(column_widths) {
            let content = cell_lines.get(line_index).map(String::as_str).unwrap_or("");
            line.push_str(&" ".repeat(padding));
            line.push_str(content);
            line.push_str(&" ".repeat(width.saturating_sub(content.width())));
            line.push_str(&" ".repeat(padding));
            line.push('│');
        }
        out.push(line);
    }

    out
}

fn border_line(
    left: &str,
    separator: &str,
    right: &str,
    column_widths: &[usize],
    padding: usize,
) -> String {
    let cell_segments = column_widths
        .iter()
        .map(|width| "─".repeat(width + padding * 2))
        .collect::<Vec<_>>();
    format!("{left}{}{right}", cell_segments.join(separator))
}

fn wrap_table_cell(cell: &str, width: usize, hard_wrap: bool) -> Vec<String> {
    if cell.is_empty() {
        return vec![String::new()];
    }
    let options = textwrap::Options::new(width)
        .break_words(hard_wrap)
        .word_separator(textwrap::WordSeparator::AsciiSpace)
        .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit);
    let wrapped = textwrap::wrap(cell, options)
        .into_iter()
        .map(std::borrow::Cow::into_owned)
        .collect::<Vec<_>>();
    if wrapped.is_empty() {
        vec![String::new()]
    } else {
        wrapped
    }
}

fn any_row_too_tall(
    rows: &[Vec<String>],
    column_widths: &[usize],
    _padding: usize,
    hard_wrap: bool,
) -> bool {
    rows.iter().skip(1).any(|row| {
        row.iter()
            .zip(column_widths)
            .map(|(cell, width)| wrap_table_cell(cell, *width, hard_wrap).len())
            .max()
            .unwrap_or(1)
            > 20
    })
}

fn table_total_width(column_widths: &[usize], padding: usize) -> usize {
    column_widths.iter().sum::<usize>()
        + column_widths.len()
        + 1
        + padding * 2 * column_widths.len()
}

fn render_vertical_table(rows: &[Vec<String>], available_width: usize) -> Vec<String> {
    let Some((headers, body_rows)) = rows.split_first() else {
        return Vec::new();
    };
    let wrap_width = available_width.max(1);
    let mut out = Vec::new();
    for (row_index, row) in body_rows.iter().enumerate() {
        if row_index > 0 {
            out.push(String::new());
        }
        out.push(format!("Row {}", row_index + 1));
        for (header, cell) in headers.iter().zip(row) {
            let label = if header.is_empty() { "Column" } else { header };
            let line = format!("{label}: {cell}");
            out.extend(
                textwrap::wrap(&line, textwrap::Options::new(wrap_width))
                    .into_iter()
                    .map(std::borrow::Cow::into_owned),
            );
        }
    }
    out
}
