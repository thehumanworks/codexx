use std::ops::Range;

use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourcePartition {
    pub(crate) stable_end: usize,
    pub(crate) stable_blocks: Vec<Range<usize>>,
    pub(crate) tail: Range<usize>,
}

pub(crate) fn partition_source(source: &str) -> SourcePartition {
    let blocks = top_level_blocks(source);
    let stable_end = if blocks.len() >= 2 {
        blocks[blocks.len() - 2].end
    } else {
        0
    };
    let stable_blocks = blocks
        .iter()
        .filter(|range| range.end <= stable_end)
        .cloned()
        .collect();

    SourcePartition {
        stable_end,
        stable_blocks,
        tail: stable_end..source.len(),
    }
}

fn top_level_blocks(source: &str) -> Vec<Range<usize>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let mut blocks: Vec<Range<usize>> = Vec::new();
    let mut depth = 0usize;
    let mut block_start: Option<usize> = None;

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(tag) if is_block_start(&tag) => {
                if depth == 0 {
                    block_start = Some(range.start);
                }
                depth += 1;
            }
            Event::End(tag) if is_block_end(tag) => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0
                        && let Some(start) = block_start.take()
                    {
                        blocks.push(start..range.end);
                    }
                }
            }
            Event::Rule if depth == 0 => {
                blocks.push(range.clone());
            }
            _ => {}
        }
    }

    blocks
}

fn is_block_start(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote
            | Tag::CodeBlock(_)
            | Tag::List(_)
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::Table(_)
            | Tag::MetadataBlock(_)
    )
}

fn is_block_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::Table
            | TagEnd::MetadataBlock(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn single_table_remains_tail() {
        let source = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";

        let partition = partition_source(source);

        assert_eq!(partition.stable_end, 0);
        assert_eq!(&source[partition.tail], source);
    }

    #[test]
    fn table_becomes_stable_after_later_block() {
        let source = "| A | B |\n| --- | --- |\n| 1 | 2 |\n\nDone.\n";

        let partition = partition_source(source);

        assert_eq!(
            &source[..partition.stable_end],
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n"
        );
        assert_eq!(&source[partition.tail], "\nDone.\n");
        assert_eq!(partition.stable_blocks.len(), 1);
    }

    #[test]
    fn fenced_code_with_pipes_is_one_tail_block() {
        let source = "```\n| A | B |\n| --- | --- |\n```\n";

        let partition = partition_source(source);

        assert_eq!(partition.stable_end, 0);
        assert_eq!(&source[partition.tail], source);
    }
}
