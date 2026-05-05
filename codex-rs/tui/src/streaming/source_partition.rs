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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Table,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceBlock {
    range: Range<usize>,
    kind: BlockKind,
}

pub(crate) fn partition_source(source: &str) -> SourcePartition {
    let blocks = top_level_blocks(source);
    let stable_end = if blocks.last().is_some_and(|block| {
        block.kind == BlockKind::Table || looks_like_table_prefix(&source[block.range.clone()])
    }) {
        if blocks.len() >= 2 {
            blocks[blocks.len() - 2].range.end
        } else {
            0
        }
    } else {
        source.len()
    };
    let stable_blocks = blocks
        .iter()
        .filter(|block| block.range.end <= stable_end)
        .map(|block| block.range.clone())
        .collect();

    SourcePartition {
        stable_end,
        stable_blocks,
        tail: stable_end..source.len(),
    }
}

pub(crate) fn source_has_table_block(source: &str) -> bool {
    top_level_blocks(source)
        .iter()
        .any(|block| block.kind == BlockKind::Table)
}

fn looks_like_table_prefix(source: &str) -> bool {
    let mut lines = source.lines().filter(|line| !line.trim().is_empty());
    let Some(first) = lines.next() else {
        return false;
    };
    let trimmed = first.trim();
    let pipe_count = trimmed.chars().filter(|ch| *ch == '|').count();
    pipe_count >= 2 || (pipe_count >= 1 && trimmed.starts_with('|'))
}

fn top_level_blocks(source: &str) -> Vec<SourceBlock> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let mut blocks: Vec<SourceBlock> = Vec::new();
    let mut depth = 0usize;
    let mut block_start: Option<usize> = None;
    let mut block_kind = BlockKind::Other;

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(tag) if is_block_start(&tag) => {
                if depth == 0 {
                    block_start = Some(range.start);
                    block_kind = block_kind_for_start(&tag);
                }
                depth += 1;
            }
            Event::End(tag) if is_block_end(tag) => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0
                        && let Some(start) = block_start.take()
                    {
                        blocks.push(SourceBlock {
                            range: start..range.end,
                            kind: block_kind,
                        });
                        block_kind = BlockKind::Other;
                    }
                }
            }
            Event::Rule if depth == 0 => {
                blocks.push(SourceBlock {
                    range: range.clone(),
                    kind: BlockKind::Other,
                });
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

fn block_kind_for_start(tag: &Tag<'_>) -> BlockKind {
    if matches!(tag, Tag::Table(_)) {
        BlockKind::Table
    } else {
        BlockKind::Other
    }
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

        assert_eq!(partition.stable_end, source.len());
        assert_eq!(&source[partition.tail], "");
        assert_eq!(partition.stable_blocks.len(), 2);
    }

    #[test]
    fn fenced_code_with_pipes_is_stable() {
        let source = "```\n| A | B |\n| --- | --- |\n```\n";

        let partition = partition_source(source);

        assert_eq!(partition.stable_end, source.len());
        assert_eq!(&source[partition.tail], "");
    }

    #[test]
    fn normal_paragraph_is_stable() {
        let source = "hello\n";

        let partition = partition_source(source);

        assert_eq!(partition.stable_end, source.len());
        assert_eq!(&source[partition.tail], "");
    }

    #[test]
    fn table_header_candidate_remains_tail() {
        let source = "| A | B |\n";

        let partition = partition_source(source);

        assert_eq!(partition.stable_end, 0);
        assert_eq!(&source[partition.tail], source);
    }

    #[test]
    fn detects_table_blocks() {
        assert!(source_has_table_block(
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n"
        ));
        assert!(!source_has_table_block(
            "```\n| A | B |\n| --- | --- |\n```\n"
        ));
    }
}
