use codex_extension_api::OutputContributionFuture;
use codex_extension_api::OutputContributor;
use codex_memories_read::citations::parse_memory_citation;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_utils_stream_parser::strip_citations;

use crate::MemoriesExtension;

impl<C> OutputContributor<C, TurnItem> for MemoriesExtension {
    fn contribute<'a>(
        &'a self,
        _context: &'a C,
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
        }

        Box::pin(std::future::ready(Ok(())))
    }
}
