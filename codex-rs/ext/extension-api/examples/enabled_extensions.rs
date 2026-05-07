use std::sync::Arc;

use codex_extension_api::ExtensionRegistryBuilder;
use codex_git_attribution as git_attribution;
use codex_git_attribution::GitAttributionContext;
use codex_memories::MemoriesContext;
use codex_memories::MemoriesExtension;
use codex_multi_agent_v2 as multi_agent_v2;
use codex_multi_agent_v2::MultiAgentV2Context;
use codex_multi_agent_v2::UsageHintAudience;

fn main() {
    let registry = ExtensionRegistryBuilder::<ctx::RuntimeContext>::new()
        .with_extension(git_attribution::extension())
        .with_extension(Arc::new(MemoriesExtension::new(Some(
            "Please use FS access bla bla bla.".to_string(),
        ))))
        .with_extension(multi_agent_v2::extension())
        .build();

    let root_context = ctx::RuntimeContext {
        commit_attribution: None,
        memory_tool_enabled: true,
        use_memories: true,
        multi_agent_v2_enabled: true,
        multi_agent_v2_usage_hint_audience: UsageHintAudience::RootAgent,
        root_agent_usage_hint_text: Some("Root-agent usage hint.".to_string()),
        subagent_usage_hint_text: Some("Subagent usage hint.".to_string()),
    };
    let memories_disabled_context = ctx::RuntimeContext {
        use_memories: false,
        ..root_context.clone()
    };

    // Build the prompt
    let prompt_fragments = registry
        .prompt_contributors()
        .iter()
        .flat_map(|contributor| contributor.contribute(&root_context))
        .collect::<Vec<_>>();

    // Get tools (MCP here but this should shift to handlers)
    let tools = registry
        .mcp_tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&root_context))
        .collect::<Vec<_>>();
    let tools_without_memories = registry
        .mcp_tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&memories_disabled_context))
        .collect::<Vec<_>>();

    println!("prompt fragments: {}", prompt_fragments.len());
    println!("mcp tools: {}", tools.len());
    println!(
        "mcp tools when use_memories=false: {}",
        tools_without_memories.len()
    );
}


mod ctx {
    use codex_git_attribution::GitAttributionContext;
    use codex_memories::MemoriesContext;
    use codex_multi_agent_v2::{MultiAgentV2Context, UsageHintAudience};

    #[derive(Clone)]
    pub struct RuntimeContext {
        // Ideally this should be at the config layer instead
        pub commit_attribution: Option<String>,
        pub memory_tool_enabled: bool,
        pub use_memories: bool,
        pub multi_agent_v2_enabled: bool,
        pub multi_agent_v2_usage_hint_audience: UsageHintAudience,
        pub root_agent_usage_hint_text: Option<String>,
        pub subagent_usage_hint_text: Option<String>,
    }

    impl GitAttributionContext for RuntimeContext {
        fn commit_attribution(&self) -> Option<&str> {
            self.commit_attribution.as_deref()
        }
    }

    impl MemoriesContext for RuntimeContext {
        fn memory_tool_enabled(&self) -> bool {
            self.memory_tool_enabled
        }

        fn use_memories(&self) -> bool {
            self.use_memories
        }
    }

    impl MultiAgentV2Context for RuntimeContext {
        fn multi_agent_v2_enabled(&self) -> bool {
            self.multi_agent_v2_enabled
        }

        fn usage_hint_audience(&self) -> UsageHintAudience {
            self.multi_agent_v2_usage_hint_audience
        }

        fn root_agent_usage_hint_text(&self) -> Option<&str> {
            self.root_agent_usage_hint_text.as_deref()
        }

        fn subagent_usage_hint_text(&self) -> Option<&str> {
            self.subagent_usage_hint_text.as_deref()
        }
    }
}