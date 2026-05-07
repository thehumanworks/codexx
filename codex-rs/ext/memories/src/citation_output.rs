use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_extension_api::OutputContributionFuture;
use codex_extension_api::OutputContributor;
use codex_extension_api::Stores;
use codex_extension_api::scopes::Thread;
use codex_extension_api::scopes::Turn;
use codex_memories_read::citations::parse_memory_citation;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_utils_stream_parser::strip_citations;

use crate::MemoriesExtension;

#[derive(Debug, Default)]
struct ThreadMemoriesState {
    turns_with_citations: AtomicU64,
}

#[derive(Debug, Default)]
struct TurnMemoriesState {
    had_citation: AtomicBool,
}

impl<C> OutputContributor<C, TurnItem> for MemoriesExtension {
    fn contribute<'a>(
        &'a self,
        _context: &'a C,
        stores: &'a Stores<'a>,
        output: &'a mut TurnItem,
    ) -> OutputContributionFuture<'a> {
        if let TurnItem::AgentMessage(agent_message) = output {
            let combined = agent_message
                .content
                .iter()
                .map(|entry| match entry {
                    AgentMessageContent::Text { text } => text.as_str(),
                })
                .collect::<String>();
            let (visible_text, citations) = strip_citations(&combined);
            agent_message.content = vec![AgentMessageContent::Text { text: visible_text }];
            agent_message.memory_citation = parse_memory_citation(citations);

            if agent_message.memory_citation.is_some()
                && let Some(turns_with_citations) = record_citation_seen(stores)
            {
                tracing::info!(turns_with_citations, "memory citation seen in turn");
            }
        }

        Box::pin(std::future::ready(Ok(())))
    }
}

fn record_citation_seen(stores: &Stores<'_>) -> Option<u64> {
    let turn_state = stores.get_or_init::<Turn, TurnMemoriesState>(Default::default);
    if turn_state.had_citation.swap(true, Ordering::Relaxed) {
        return None;
    }

    let thread_stats = stores.get_or_init::<Thread, ThreadMemoriesState>(Default::default);
    Some(
        thread_stats
            .turns_with_citations
            .fetch_add(1, Ordering::Relaxed)
            + 1,
    )
}
