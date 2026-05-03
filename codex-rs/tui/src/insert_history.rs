//! Inserts finalized history rows into terminal scrollback.
//!
//! Codex uses the terminal scrollback itself for finalized chat history, so inserting a history
//! cell is an escape-sequence operation rather than a normal ratatui render. The mode determines
//! how to create room for new history above the inline viewport.

use std::fmt;
use std::io;
use std::io::Write;

use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_line;
use crate::wrapping::line_contains_url_like;
use crate::wrapping::line_has_mixed_url_and_non_url_tokens;
use crossterm::Command;
use crossterm::cursor::MoveDown;
use crossterm::cursor::MoveTo;
use crossterm::cursor::MoveToColumn;
use crossterm::cursor::RestorePosition;
use crossterm::cursor::SavePosition;
use crossterm::queue;
use crossterm::style::Color as CColor;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::prelude::Backend;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Selects the terminal escape strategy for inserting history lines above the viewport.
///
/// Standard terminals support `DECSTBM` scroll regions and Reverse Index (`ESC M`),
/// which let us slide existing content down without redrawing it. Some terminals
/// or terminal-like surfaces mishandle those sequences for normal scrollback, so
/// `Newline` mode falls back to emitting newlines at the bottom of the screen
/// and writing lines at absolute positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertHistoryMode {
    Standard,
    Newline,
}

impl InsertHistoryMode {
    pub fn new(use_newline_insert: bool) -> Self {
        if use_newline_insert {
            Self::Newline
        } else {
            Self::Standard
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryLineWrapPolicy {
    PreWrap,
    Terminal,
}

/// Insert `lines` above the viewport using the terminal's backend writer
/// (avoids direct stdout references).
pub fn insert_history_lines<B>(
    terminal: &mut crate::custom_terminal::Terminal<B>,
    lines: Vec<Line>,
) -> io::Result<()>
where
    B: Backend + Write,
{
    insert_history_lines_with_mode(terminal, lines, InsertHistoryMode::Standard)
}

/// Insert `lines` above the viewport, using the escape strategy selected by `mode`.
///
/// In `Standard` mode this manipulates DECSTBM scroll regions to slide existing
/// scrollback down and writes new lines into the freed space. In `Newline` mode
/// it renders the inserted history into a dense buffer and appends full-screen
/// lines to create real scrollback. Both modes update `terminal.viewport_area`
/// so subsequent draw passes know where the viewport moved to. Resize reflow
/// uses the same buffer renderer after clearing old scrollback.
pub fn insert_history_lines_with_mode<B>(
    terminal: &mut crate::custom_terminal::Terminal<B>,
    lines: Vec<Line>,
    mode: InsertHistoryMode,
) -> io::Result<()>
where
    B: Backend + Write,
{
    insert_history_lines_with_mode_and_wrap_policy(
        terminal,
        lines,
        mode,
        HistoryLineWrapPolicy::PreWrap,
    )
}

pub fn insert_history_lines_with_mode_and_wrap_policy<B>(
    terminal: &mut crate::custom_terminal::Terminal<B>,
    lines: Vec<Line>,
    mode: InsertHistoryMode,
    wrap_policy: HistoryLineWrapPolicy,
) -> io::Result<()>
where
    B: Backend + Write,
{
    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));

    let mut area = terminal.viewport_area;
    let mut should_update_area = false;
    let last_cursor_pos = terminal.last_known_cursor_pos;

    let wrap_width = area.width.max(1) as usize;
    let (wrapped, wrapped_lines) = wrap_history_lines(&lines, wrap_width, wrap_policy);

    match mode {
        InsertHistoryMode::Newline => {
            let history_buffer = render_history_buffer(&wrapped, area.width);
            let history_rows = history_buffer.area.height;
            if history_rows > 0 {
                terminal.insert_buffer_before_viewport_without_scroll_region(history_buffer)?;
                terminal
                    .backend_mut()
                    .set_cursor_position(last_cursor_pos)?;
                terminal.note_history_rows_inserted(history_rows);
            }
        }
        InsertHistoryMode::Standard => {
            let writer = terminal.backend_mut();
            let cursor_top = if area.bottom() < screen_size.height {
                let scroll_amount = wrapped_lines.min(screen_size.height - area.bottom());

                let top_1based = area.top() + 1;
                queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
                queue!(writer, MoveTo(/*x*/ 0, area.top()))?;
                for _ in 0..scroll_amount {
                    queue!(writer, Print("\x1bM"))?;
                }
                queue!(writer, ResetScrollRegion)?;

                let cursor_top = area.top().saturating_sub(1);
                area.y += scroll_amount;
                should_update_area = true;
                cursor_top
            } else {
                area.top().saturating_sub(1)
            };

            // Limit the scroll region to the lines from the top of the screen to the
            // top of the viewport. With this in place, when we add lines inside this
            // area, only the lines in this area will be scrolled. We place the cursor
            // at the end of the scroll region, and add lines starting there.
            //
            // ┌─Screen───────────────────────┐
            // │┌╌Scroll region╌╌╌╌╌╌╌╌╌╌╌╌╌╌┐│
            // │┆                            ┆│
            // │┆                            ┆│
            // │┆                            ┆│
            // │█╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┘│
            // │╭─Viewport───────────────────╮│
            // ││                            ││
            // │╰────────────────────────────╯│
            // └──────────────────────────────┘
            queue!(writer, SetScrollRegion(1..area.top()))?;

            // NB: we are using MoveTo instead of set_cursor_position here to avoid messing with the
            // terminal's last_known_cursor_position, which hopefully will still be accurate after we
            // fetch/restore the cursor position. insert_history_lines should be cursor-position-neutral :)
            queue!(writer, MoveTo(/*x*/ 0, cursor_top))?;

            for line in &wrapped {
                queue!(writer, Print("\r\n"))?;
                write_history_line(writer, line, wrap_width)?;
            }

            queue!(writer, ResetScrollRegion)?;

            // Restore the cursor position to where it was before we started.
            queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;

            let _ = writer;
            if should_update_area {
                terminal.set_viewport_area(area);
            }
            if wrapped_lines > 0 {
                terminal.note_history_rows_inserted(wrapped_lines);
            }
        }
    }

    Ok(())
}

/// Replay history rows after the caller has cleared terminal scrollback/screen.
///
/// Unlike [`insert_history_lines_with_mode`], this does not use scroll regions or
/// reverse-index insertion. Resize reflow already owns replacing the visible
/// transcript from source-backed cells, so this path writes the rebuilt rows
/// from the top of the terminal and leaves the inline viewport directly after
/// the visible history tail.
pub(crate) fn replay_history_lines_after_clear<B>(
    terminal: &mut crate::custom_terminal::Terminal<B>,
    lines: Vec<Line<'static>>,
) -> io::Result<()>
where
    B: Backend + Write,
{
    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));
    let mut area = terminal.viewport_area;
    area.width = screen_size.width;
    area.y = 0;
    terminal.set_viewport_area(area);

    let wrap_width = area.width.max(1) as usize;
    let (wrapped, _) =
        wrap_history_lines(&lines, wrap_width, HistoryLineWrapPolicy::PreWrap);
    let history_buffer = render_history_buffer(&wrapped, area.width);
    let rendered_rows = history_buffer.area.height;

    if rendered_rows == 0 {
        return Ok(());
    }

    terminal.insert_buffer_before_viewport_without_scroll_region(history_buffer)?;
    terminal.note_history_rows_inserted(rendered_rows);

    Ok(())
}

fn wrap_history_lines<'a>(
    lines: &'a [Line<'a>],
    wrap_width: usize,
    wrap_policy: HistoryLineWrapPolicy,
) -> (Vec<Line<'a>>, u16) {
    let mut wrapped = Vec::new();
    let mut wrapped_rows = 0usize;

    for line in lines {
        let line_wrapped = match wrap_policy {
            HistoryLineWrapPolicy::Terminal => vec![line.clone()],
            HistoryLineWrapPolicy::PreWrap
                if is_preformatted_box_table_line(line)
                    || (line_contains_url_like(line)
                        && !line_has_mixed_url_and_non_url_tokens(line)) =>
            {
                vec![line.clone()]
            }
            HistoryLineWrapPolicy::PreWrap => adaptive_wrap_line(line, RtOptions::new(wrap_width)),
        };
        wrapped_rows += line_wrapped
            .iter()
            .map(|wrapped_line| wrapped_line.width().max(1).div_ceil(wrap_width))
            .sum::<usize>();
        wrapped.extend(line_wrapped);
    }

    (wrapped, wrapped_rows as u16)
}

fn render_history_buffer(lines: &[Line<'_>], width: u16) -> Buffer {
    let width = width.max(1);
    let rows = lines
        .iter()
        .map(|line| rendered_history_line_rows(line, width))
        .sum();
    let mut buffer = Buffer::empty(Rect::new(
        /*x*/ 0, /*y*/ 0, width, /*height*/ rows,
    ));
    let mut y = 0;
    for line in lines {
        y = render_history_line_to_buffer(&mut buffer, line, width, y);
    }

    buffer
}

fn rendered_history_line_rows(line: &Line<'_>, width: u16) -> u16 {
    let mut rows = 1u16;
    let mut x = 0u16;
    for span in &line.spans {
        for symbol in UnicodeSegmentation::graphemes(span.content.as_ref(), true) {
            if symbol.contains(char::is_control) {
                continue;
            }
            let symbol_width = UnicodeWidthStr::width(symbol) as u16;
            if symbol_width == 0 {
                continue;
            }
            if x > 0 && x.saturating_add(symbol_width) > width {
                rows = rows.saturating_add(1);
                x = 0;
            }
            if symbol_width <= width {
                x = x.saturating_add(symbol_width);
            }
        }
    }
    rows
}

fn render_history_line_to_buffer(
    buffer: &mut Buffer,
    line: &Line<'_>,
    width: u16,
    mut y: u16,
) -> u16 {
    let mut x = 0u16;
    for span in &line.spans {
        let style = line.style.patch(span.style);
        for symbol in UnicodeSegmentation::graphemes(span.content.as_ref(), true) {
            if symbol.contains(char::is_control) {
                continue;
            }
            let symbol_width = UnicodeWidthStr::width(symbol) as u16;
            if symbol_width == 0 {
                continue;
            }
            if x > 0 && x.saturating_add(symbol_width) > width {
                y = y.saturating_add(1);
                x = 0;
            }
            if symbol_width > width {
                continue;
            }
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol(symbol).set_style(style);
            }
            let next_x = x.saturating_add(symbol_width);
            x = x.saturating_add(1);
            while x < next_x && x < width {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                }
                x = x.saturating_add(1);
            }
        }
    }
    y.saturating_add(1)
}

fn is_preformatted_box_table_line(line: &Line<'_>) -> bool {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let trimmed = text.trim_start();
    (trimmed.starts_with('│') && trimmed.matches('│').count() >= 2)
        || trimmed.starts_with('┌')
        || trimmed.starts_with('├')
        || trimmed.starts_with('└')
}

/// Render a single wrapped history line: clear continuation rows for wide lines,
/// set foreground/background colors, and write styled spans. Caller is responsible
/// for cursor positioning and any leading `\r\n`.
fn write_history_line<W: Write>(writer: &mut W, line: &Line, wrap_width: usize) -> io::Result<()> {
    let physical_rows = line.width().max(1).div_ceil(wrap_width) as u16;
    if physical_rows > 1 {
        queue!(writer, SavePosition)?;
        for _ in 1..physical_rows {
            queue!(writer, MoveDown(1), MoveToColumn(0))?;
            queue!(writer, Clear(ClearType::UntilNewLine))?;
        }
        queue!(writer, RestorePosition)?;
    }
    queue!(
        writer,
        SetColors(Colors::new(
            line.style
                .fg
                .map(std::convert::Into::into)
                .unwrap_or(CColor::Reset),
            line.style
                .bg
                .map(std::convert::Into::into)
                .unwrap_or(CColor::Reset)
        ))
    )?;
    queue!(writer, Clear(ClearType::UntilNewLine))?;
    // Merge line-level style into each span so that ANSI colors reflect
    // line styles (e.g., blockquotes with green fg).
    let merged_spans: Vec<Span> = line
        .spans
        .iter()
        .map(|s| Span {
            style: s.style.patch(line.style),
            content: s.content.clone(),
        })
        .collect();
    write_spans(writer, merged_spans.iter())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetScrollRegion(pub std::ops::Range<u16>);

impl Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("tried to execute SetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        // TODO(nornagon): is this supported on Windows?
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetScrollRegion;

impl Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("tried to execute ResetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        // TODO(nornagon): is this supported on Windows?
        true
    }
}

struct ModifierDiff {
    pub from: Modifier,
    pub to: Modifier,
}

impl ModifierDiff {
    fn queue<W>(self, mut w: W) -> io::Result<()>
    where
        W: io::Write,
    {
        use crossterm::style::Attribute as CAttribute;
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::RapidBlink))?;
        }

        Ok(())
    }
}

fn write_spans<'a, I>(mut writer: &mut impl Write, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();
    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            let diff = ModifierDiff {
                from: last_modifier,
                to: modifier,
            };
            diff.queue(&mut writer)?;
            last_modifier = modifier;
        }
        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(next_fg.into(), next_bg.into()))
            )?;
            fg = next_fg;
            bg = next_bg;
        }

        queue!(writer, Print(span.content.clone()))?;
    }

    queue!(
        writer,
        SetForegroundColor(CColor::Reset),
        SetBackgroundColor(CColor::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_render::render_markdown_text;
    use crate::test_backend::VT100Backend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    fn buffer_rows(buffer: &Buffer) -> Vec<String> {
        buffer
            .content
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    #[test]
    fn detects_preformatted_box_table_lines() {
        assert!(is_preformatted_box_table_line(&Line::from(
            "│       Link │ a (https://example.com/a) │"
        )));
        assert!(is_preformatted_box_table_line(&Line::from(
            "┌──────┬────────┐"
        )));
        assert!(!is_preformatted_box_table_line(&Line::from(
            "plain text with https://example.com"
        )));
        assert!(!is_preformatted_box_table_line(&Line::from(
            "  │ quoted text with https://example.com"
        )));
    }

    #[test]
    fn history_buffer_splits_preserved_box_rows_by_physical_width() {
        let line = Line::from("│ abcdefghij │");

        let buffer = render_history_buffer(&[line], /*width*/ 8);

        let rows = buffer_rows(&buffer);
        assert_eq!(buffer.area.height, 2);
        assert_eq!(rows[0].trim_end(), "│ abcdef");
        assert_eq!(rows[1].trim_end(), "ghij │");
    }

    #[test]
    fn writes_bold_then_regular_spans() {
        use ratatui::style::Stylize;

        let spans = ["A".bold(), "B".into()];

        let mut actual: Vec<u8> = Vec::new();
        write_spans(&mut actual, spans.iter()).unwrap();

        let mut expected: Vec<u8> = Vec::new();
        queue!(
            expected,
            SetAttribute(crossterm::style::Attribute::Bold),
            Print("A"),
            SetAttribute(crossterm::style::Attribute::NormalIntensity),
            Print("B"),
            SetForegroundColor(CColor::Reset),
            SetBackgroundColor(CColor::Reset),
            SetAttribute(crossterm::style::Attribute::Reset),
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(actual).unwrap(),
            String::from_utf8(expected).unwrap()
        );
    }

    #[test]
    fn vt100_blockquote_line_emits_green_fg() {
        // Set up a small off-screen terminal
        let width: u16 = 40;
        let height: u16 = 10;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        // Place viewport on the last line so history inserts scroll upward
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        // Build a blockquote-like line: apply line-level green style and prefix "> "
        let mut line: Line<'static> = Line::from(vec!["> ".into(), "Hello world".into()]);
        line = line.style(Color::Green);
        insert_history_lines(&mut term, vec![line])
            .expect("Failed to insert history lines in test");

        let mut saw_colored = false;
        'outer: for row in 0..height {
            for col in 0..width {
                if let Some(cell) = term.backend().vt100().screen().cell(row, col)
                    && cell.has_contents()
                    && cell.fgcolor() != vt100::Color::Default
                {
                    saw_colored = true;
                    break 'outer;
                }
            }
        }
        assert!(
            saw_colored,
            "expected at least one colored cell in vt100 output"
        );
    }

    #[test]
    fn vt100_blockquote_wrap_preserves_color_on_all_wrapped_lines() {
        // Force wrapping by using a narrow viewport width and a long blockquote line.
        let width: u16 = 20;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        // Viewport is the last line so history goes directly above it.
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        // Create a long blockquote with a distinct prefix and enough text to wrap.
        let mut line: Line<'static> = Line::from(vec![
            "> ".into(),
            "This is a long quoted line that should wrap".into(),
        ]);
        line = line.style(Color::Green);

        insert_history_lines(&mut term, vec![line])
            .expect("Failed to insert history lines in test");

        // Parse and inspect the final screen buffer.
        let screen = term.backend().vt100().screen();

        // Collect rows that are non-empty; these should correspond to our wrapped lines.
        let mut non_empty_rows: Vec<u16> = Vec::new();
        for row in 0..height {
            let mut any = false;
            for col in 0..width {
                if let Some(cell) = screen.cell(row, col)
                    && cell.has_contents()
                    && cell.contents() != "\0"
                    && cell.contents() != " "
                {
                    any = true;
                    break;
                }
            }
            if any {
                non_empty_rows.push(row);
            }
        }

        // Expect at least two rows due to wrapping.
        assert!(
            non_empty_rows.len() >= 2,
            "expected wrapped output to span >=2 rows, got {non_empty_rows:?}",
        );

        // For each non-empty row, ensure all non-space cells are using a non-default fg color.
        for row in non_empty_rows {
            for col in 0..width {
                if let Some(cell) = screen.cell(row, col) {
                    let contents = cell.contents();
                    if !contents.is_empty() && contents != " " {
                        assert!(
                            cell.fgcolor() != vt100::Color::Default,
                            "expected non-default fg on row {row} col {col}, got {:?}",
                            cell.fgcolor()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn vt100_colored_prefix_then_plain_text_resets_color() {
        let width: u16 = 40;
        let height: u16 = 6;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        // First span colored, rest plain.
        let line: Line<'static> = Line::from(vec![
            Span::styled("1. ", ratatui::style::Style::default().fg(Color::LightBlue)),
            Span::raw("Hello world"),
        ]);

        insert_history_lines(&mut term, vec![line])
            .expect("Failed to insert history lines in test");

        let screen = term.backend().vt100().screen();

        // Find the first non-empty row; verify first three cells are colored, following cells default.
        'rows: for row in 0..height {
            let mut has_text = false;
            for col in 0..width {
                if let Some(cell) = screen.cell(row, col)
                    && cell.has_contents()
                    && cell.contents() != " "
                {
                    has_text = true;
                    break;
                }
            }
            if !has_text {
                continue;
            }

            // Expect "1. Hello world" starting at col 0.
            for col in 0..3 {
                let cell = screen.cell(row, col).unwrap();
                assert!(
                    cell.fgcolor() != vt100::Color::Default,
                    "expected colored prefix at col {col}, got {:?}",
                    cell.fgcolor()
                );
            }
            for col in 3..(3 + "Hello world".len() as u16) {
                let cell = screen.cell(row, col).unwrap();
                assert_eq!(
                    cell.fgcolor(),
                    vt100::Color::Default,
                    "expected default color for plain text at col {col}, got {:?}",
                    cell.fgcolor()
                );
            }
            break 'rows;
        }
    }

    #[test]
    fn vt100_deep_nested_mixed_list_third_level_marker_is_colored() {
        // Markdown with five levels (ordered → unordered → ordered → unordered → unordered).
        let md = "1. First\n   - Second level\n     1. Third level (ordered)\n        - Fourth level (bullet)\n          - Fifth level to test indent consistency\n";
        let text = render_markdown_text(md);
        let lines: Vec<Line<'static>> = text.lines.clone();

        let width: u16 = 60;
        let height: u16 = 12;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = ratatui::layout::Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        insert_history_lines(&mut term, lines).expect("Failed to insert history lines in test");

        let screen = term.backend().vt100().screen();

        // Reconstruct screen rows as strings to locate the 3rd level line.
        let rows: Vec<String> = screen.rows(0, width).collect();

        let needle = "1. Third level (ordered)";
        let row_idx = rows
            .iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| {
                panic!("expected to find row containing {needle:?}, have rows: {rows:?}")
            });
        let col_start = rows[row_idx].find(needle).unwrap() as u16; // column where '1' starts

        // Verify that the numeric marker ("1.") at the third level is colored
        // (non-default fg) and the content after the following space resets to default.
        for c in [col_start, col_start + 1] {
            let cell = screen.cell(row_idx as u16, c).unwrap();
            assert!(
                cell.fgcolor() != vt100::Color::Default,
                "expected colored 3rd-level marker at row {row_idx} col {c}, got {:?}",
                cell.fgcolor()
            );
        }
        let content_col = col_start + 3; // skip '1', '.', and the space
        if let Some(cell) = screen.cell(row_idx as u16, content_col) {
            assert_eq!(
                cell.fgcolor(),
                vt100::Color::Default,
                "expected default color for 3rd-level content at row {row_idx} col {content_col}, got {:?}",
                cell.fgcolor()
            );
        }
    }

    #[test]
    fn vt100_prefixed_url_keeps_prefix_and_url_on_same_row() {
        let width: u16 = 48;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        let url = "http://a-long-url.com/this/that/blablablab/new.aspx/many_people_like_how";
        let line: Line<'static> = Line::from(vec!["  │ ".into(), url.into()]);

        insert_history_lines(&mut term, vec![line]).expect("insert history");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();

        assert!(
            rows.iter().any(|r| r.contains("│ http://a-long-url.com")),
            "expected prefix and URL on same row, rows: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.trim_end() == "│"),
            "unexpected orphan prefix row, rows: {rows:?}"
        );
    }

    #[test]
    fn vt100_prefixed_url_like_without_scheme_keeps_prefix_and_token_on_same_row() {
        let width: u16 = 48;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        let url_like =
            "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890";
        let line: Line<'static> = Line::from(vec!["  │ ".into(), url_like.into()]);

        insert_history_lines(&mut term, vec![line]).expect("insert history");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();

        assert!(
            rows.iter()
                .any(|r| r.contains("│ example.test/api/v1/projects")),
            "expected prefix and URL-like token on same row, rows: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.trim_end() == "│"),
            "unexpected orphan prefix row, rows: {rows:?}"
        );
    }

    #[test]
    fn vt100_prefixed_mixed_url_line_wraps_suffix_words_together() {
        let width: u16 = 24;
        let height: u16 = 10;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        let url = "https://example.test/path/abcdef12345";
        let line: Line<'static> = Line::from(vec![
            "  │ ".into(),
            "see ".into(),
            url.into(),
            " tail words".into(),
        ]);

        insert_history_lines(&mut term, vec![line]).expect("insert mixed history");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        assert!(
            rows.iter().any(|r| r.contains("│ see")),
            "expected prefixed prose before URL, rows: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("tail words")),
            "expected suffix words to wrap as a phrase, rows: {rows:?}"
        );
    }

    #[test]
    fn vt100_terminal_wrap_policy_does_not_pre_wrap_long_paragraph() {
        let width: u16 = 20;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        let line = Line::from("alpha beta gamma delta epsilon zeta");

        insert_history_lines_with_mode_and_wrap_policy(
            &mut term,
            vec![line],
            InsertHistoryMode::Standard,
            HistoryLineWrapPolicy::Terminal,
        )
        .expect("insert raw history");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        assert!(
            rows.iter()
                .any(|row| row.trim_end() == "alpha beta gamma del"),
            "expected terminal soft-wrap instead of Codex word pre-wrap, rows: {rows:?}"
        );
    }

    #[test]
    fn vt100_unwrapped_url_like_clears_continuation_rows() {
        let width: u16 = 20;
        let height: u16 = 10;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        let filler_line: Line<'static> = Line::from(vec![
            "  │ ".into(),
            "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".into(),
        ]);
        insert_history_lines(&mut term, vec![filler_line]).expect("insert filler history");

        let url_like = "example.test/api/v1/short";
        let url_line: Line<'static> = Line::from(vec!["  │ ".into(), url_like.into()]);
        insert_history_lines(&mut term, vec![url_line]).expect("insert url-like history");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        let first_row = rows
            .iter()
            .position(|row| row.contains("│ example.test/api"))
            .unwrap_or_else(|| panic!("expected url-like first row in screen rows: {rows:?}"));
        assert!(
            first_row + 1 < rows.len(),
            "expected a continuation row for wrapped URL-like line, rows: {rows:?}"
        );
        let continuation_row = rows[first_row + 1].trim_end();

        assert!(
            continuation_row.contains("/v1/short") || continuation_row.contains("short"),
            "expected continuation row to contain wrapped URL-like tail, got: {continuation_row:?}"
        );
        assert!(
            !continuation_row.contains('X'),
            "expected continuation row to be cleared before writing wrapped URL-like content, got: {continuation_row:?}"
        );
    }

    #[test]
    fn vt100_long_unwrapped_url_does_not_insert_extra_blank_gap_before_content() {
        let width: u16 = 56;
        let height: u16 = 24;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(0, height - 1, width, 1);
        term.set_viewport_area(viewport);

        let prompt = "Write a long URL as output for testing";
        insert_history_lines(&mut term, vec![Line::from(prompt)]).expect("insert prompt line");

        let long_url = format!(
            "https://example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/{}",
            "very-long-segment-".repeat(16),
        );
        let url_line: Line<'static> = Line::from(vec!["• ".into(), long_url.into()]);
        insert_history_lines(&mut term, vec![url_line]).expect("insert long url line");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        let prompt_row = rows
            .iter()
            .position(|row| row.contains("Write a long URL as output for testing"))
            .unwrap_or_else(|| panic!("expected prompt row in screen rows: {rows:?}"));
        let url_row = rows
            .iter()
            .position(|row| row.contains("• https://example.test/api"))
            .unwrap_or_else(|| panic!("expected URL first row in screen rows: {rows:?}"));

        assert!(
            url_row <= prompt_row + 2,
            "expected URL content to appear immediately after prompt (allowing at most one spacer row), got prompt_row={prompt_row}, url_row={url_row}, rows={rows:?}",
        );
    }

    #[test]
    fn vt100_newline_mode_inserts_history_and_updates_viewport() {
        let width: u16 = 32;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(/*x*/ 0, /*y*/ 4, width, /*height*/ 2);
        term.set_viewport_area(viewport);

        let line: Line<'static> = Line::from("zellij history");
        insert_history_lines_with_mode(&mut term, vec![line], InsertHistoryMode::Newline)
            .expect("insert zellij history");

        let start_row = 0;
        let rows: Vec<String> = term
            .backend()
            .vt100()
            .screen()
            .rows(start_row, width)
            .collect();
        assert!(
            rows.iter().any(|row| row.contains("zellij history")),
            "expected zellij history row in screen output, rows: {rows:?}"
        );
        assert_eq!(term.viewport_area, Rect::new(0, 5, width, 2));
        assert_eq!(term.visible_history_rows(), 1);
    }

    #[test]
    fn vt100_newline_mode_keeps_large_insert_tail_above_viewport() {
        let width: u16 = 48;
        let height: u16 = 12;
        let viewport_height: u16 = 2;
        let backend = VT100Backend::new_with_scrollback(width, height, /*scrollback_len*/ 128);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        let viewport = Rect::new(
            /*x*/ 0,
            height - viewport_height,
            width,
            viewport_height,
        );
        term.set_viewport_area(viewport);

        let lines = (1..=18)
            .map(|index| Line::from(format!("history row {index:02}")))
            .collect();
        insert_history_lines_with_mode(&mut term, lines, InsertHistoryMode::Newline)
            .expect("insert large newline-mode history");

        // A normal draw immediately repaints the inline viewport after history insertion.
        // The inserted history tail must remain above that viewport rather than being
        // written into rows that the draw will clear.
        let viewport = term.viewport_area;
        {
            let writer = term.backend_mut();
            for y in viewport.top()..viewport.bottom() {
                queue!(writer, MoveTo(/*x*/ 0, y), Clear(ClearType::UntilNewLine))
                    .expect("clear viewport row");
            }
        }

        let rows: Vec<String> = term
            .backend()
            .vt100()
            .screen()
            .rows(/*start*/ 0, width)
            .collect();
        let tail = rows[..viewport.top() as usize].join("\n");
        assert!(
            tail.contains("history row 18"),
            "expected final inserted row above viewport, rows={rows:?}, viewport={viewport:?}",
        );

        term.backend_mut()
            .vt100_mut()
            .screen_mut()
            .set_scrollback(usize::MAX);
        let scrolled_rows: Vec<String> = term
            .backend()
            .vt100()
            .screen()
            .rows(/*start*/ 0, width)
            .collect();
        let scrolled = scrolled_rows.join("\n");
        assert!(
            scrolled.contains("history row 01"),
            "expected oldest inserted rows in terminal scrollback, rows={scrolled_rows:?}",
        );
    }

    #[test]
    fn vt100_newline_mode_persists_rendered_markdown_table_in_scrollback() {
        let width: u16 = 72;
        let height: u16 = 10;
        let viewport_height: u16 = 2;
        let backend = VT100Backend::new_with_scrollback(width, height, /*scrollback_len*/ 256);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        term.set_viewport_area(Rect::new(
            /*x*/ 0,
            height - viewport_height,
            width,
            viewport_height,
        ));

        let markdown = "\
| Col A | Col B | Col C | Col D | Col E |\n\
| --- | --- | --- | --- | --- |\n\
| a1 | `cargo test` | 😀 | [docs](https://example.com/a) | ok |\n\
| a2 | `cargo fmt` | 🚧 | [guide](https://example.com/b) | maybe |\n\
| a3 | `assert!()` | 🧪 | [api](https://example.com/c) | test |\n\
| a4 | `grep` | 🔍 | [search](https://example.com/d) | search |\n\
| a5 | `zip` | 🎁 | [bundle](https://example.com/e) | bundle |\n";
        let mut lines = vec![Line::from("before table marker")];
        lines.extend(render_markdown_text(markdown).lines);

        insert_history_lines_with_mode(&mut term, lines, InsertHistoryMode::Newline)
            .expect("insert rendered table history");

        let viewport = term.viewport_area;
        {
            let writer = term.backend_mut();
            for y in viewport.top()..viewport.bottom() {
                queue!(writer, MoveTo(/*x*/ 0, y), Clear(ClearType::UntilNewLine))
                    .expect("clear viewport row");
            }
        }

        let visible_rows: Vec<String> = term
            .backend()
            .vt100()
            .screen()
            .rows(/*start*/ 0, width)
            .collect();
        let visible_tail = visible_rows[..viewport.top() as usize].join("\n");
        assert!(
            visible_tail.contains("zip") && visible_tail.contains("bundle"),
            "expected final table row above viewport after repaint: {visible_rows:?}",
        );

        let max_scrollback = {
            let screen = term.backend_mut().vt100_mut().screen_mut();
            screen.set_scrollback(usize::MAX);
            screen.scrollback()
        };
        let mut scrollback_windows = Vec::new();
        for offset in (0..=max_scrollback).rev() {
            term.backend_mut()
                .vt100_mut()
                .screen_mut()
                .set_scrollback(offset);
            let rows: Vec<String> = term
                .backend()
                .vt100()
                .screen()
                .rows(/*start*/ 0, width)
                .collect();
            scrollback_windows.push(rows);
        }
        let scrollback = scrollback_windows
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            scrollback.contains("before table marker"),
            "expected marker row in scrollback windows: {scrollback_windows:?}",
        );
        assert!(
            scrollback.contains('┌') && scrollback.contains('┐'),
            "expected table border row in scrollback windows: {scrollback_windows:?}",
        );
        assert!(
            scrollback.contains("zip") && scrollback.contains("bundle"),
            "expected final table row in scrollback windows: {scrollback_windows:?}",
        );
    }

    #[test]
    fn vt100_resize_replay_replaces_cleared_history_without_incremental_insert() {
        let width: u16 = 36;
        let height: u16 = 14;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        term.set_viewport_area(Rect::new(
            /*x*/ 0, /*y*/ 0, width, /*height*/ 3,
        ));

        insert_history_lines(&mut term, vec![Line::from("stale row that must disappear")])
            .expect("insert stale history");
        term.clear_scrollback_and_visible_screen_ansi()
            .expect("clear before replay");

        let lines = vec![
            Line::from("┌─────────┬────────────────────┐"),
            Line::from("│   Label │ Alpha              │"),
            Line::from("│ Content │ first value        │"),
            Line::from("├─────────┼────────────────────┤"),
            Line::from("│   Label │ Beta               │"),
            Line::from("│ Content │ second value       │"),
            Line::from("└─────────┴────────────────────┘"),
        ];
        replay_history_lines_after_clear(&mut term, lines).expect("replay history");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        assert_eq!(term.viewport_area, Rect::new(0, 7, width, 3));
        assert_eq!(term.visible_history_rows(), 7);
        assert!(rows[0].contains("┌─────────┬────────────────────┐"));
        assert!(rows[6].contains("└─────────┴────────────────────┘"));
        assert!(
            !rows.iter().any(|row| row.contains("stale row")),
            "stale history survived resize replay: {rows:?}"
        );
    }

    #[test]
    fn vt100_resize_replay_keeps_visible_tail_above_viewport_when_history_overflows() {
        let width: u16 = 20;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        term.set_viewport_area(Rect::new(
            /*x*/ 0, /*y*/ 0, width, /*height*/ 2,
        ));

        let lines = (1..=10)
            .map(|index| Line::from(format!("history row {index:02}")))
            .collect();
        replay_history_lines_after_clear(&mut term, lines).expect("replay overflowing history");

        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        assert_eq!(term.viewport_area, Rect::new(0, 6, width, 2));
        assert_eq!(term.visible_history_rows(), 6);
        assert!(rows[0].contains("history row 05"), "rows: {rows:?}");
        assert!(rows[5].contains("history row 10"), "rows: {rows:?}");
        assert!(rows[6].trim().is_empty(), "rows: {rows:?}");
        assert!(rows[7].trim().is_empty(), "rows: {rows:?}");
    }
}
