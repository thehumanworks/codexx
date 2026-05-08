use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributionFuture;
use codex_extension_api::TurnItemContributor;
use codex_memories_read::citations::parse_memory_citation;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_utils_stream_parser::strip_citations;

#[derive(Debug)]
pub(crate) struct CitationContributor;

impl TurnItemContributor for CitationContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> TurnItemContributionFuture<'a> {
        if let TurnItem::AgentMessage(agent_message) = item {
            let text = agent_message
                .content
                .iter()
                .map(|entry| match entry {
                    AgentMessageContent::Text { text } => text.as_str(),
                })
                .collect::<String>();
            let (visible_text, citations) = strip_citations(&text);
            agent_message.content = vec![AgentMessageContent::Text { text: visible_text }];
            agent_message.memory_citation = parse_memory_citation(citations);
        }

        Box::pin(std::future::ready(Ok(())))
    }
}
