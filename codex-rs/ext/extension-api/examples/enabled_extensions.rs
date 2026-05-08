use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_git_attribution as git_attribution;
use codex_memories::MemoriesExtension;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use codex_memories::citation_output::MemoryState;
use codex_protocol::items::{TurnItem, UserMessageItem};

#[tokio::main]
async fn main() {
    // 1. Build the concrete extension values owned by the host.
    let memories = Arc::new(MemoriesExtension::new(
        Some("Use memories when they help answer the user.".to_string()),
        std::env::temp_dir().join("codex-memories-example"),
    ));
    let git_attribution = git_attribution::extension();

    // 2. Install those extensions into one registry for the runtime context type
    //    this host exposes to contributors.
    let registry = ExtensionRegistryBuilder::<ctx::RuntimeContext>::new()
        .with_extension(git_attribution)
        .with_extension(memories)
        .build();

    // 3. Build the runtime facts contributors can inspect when they decide
    //    whether they are active for this request.
    //
    // Ideally, this is instead the TurnContext or the Config or so
    let runtime = ctx::RuntimeContext {
        commit_attribution: None,
        memory_tool_enabled: true,
        use_memories: true,
    };

    // 4. Build the host-owned stores used by the active contribution families.
    let session_data = ExtensionData::new();
    let thread_data = ExtensionData::new();
    let turn_data = ExtensionData::new();

    // 5. Invoke whichever contribution families this insertion point needs.
    let prompt_fragments = registry
        .context_contributors()
        .iter()
        .flat_map(|contributor| contributor.contribute(&runtime, &session_data, &thread_data))
        .collect::<Vec<_>>();

    let tools = registry
        .tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&runtime, &thread_data))
        .collect::<Vec<_>>();

    let tools = registry
        .tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&runtime, &thread_data))
        .collect::<Vec<_>>();

    let mut item =TurnItem::UserMessage(UserMessageItem {
        content: vec!(),
        id: String::new()
    });
    for contributor in registry.turn_item_contributors() {
        let _ = contributor.contribute(&runtime, &thread_data, &turn_data, &mut item).await;
    }
    for contributor in registry.turn_item_contributors() {
        let _ = contributor.contribute(&runtime, &thread_data, &turn_data, &mut item).await;
    }

    let session_state = session_data.get_or_init::<MemoryState>(Default::default);
    let thread_state = thread_data.get_or_init::<MemoryState>(Default::default);
    let turn_state = turn_data.get_or_init::<MemoryState>(Default::default);

    println!("Session: {} (expected 1)", session_state.counter.load(Ordering::Relaxed));
    println!("Thread: {} (expected 3)", thread_state.counter.load(Ordering::Relaxed));
    println!("Turn: {} (expected 2)", turn_state.counter.load(Ordering::Relaxed));

    println!("prompt fragments: {}", prompt_fragments.len());
    println!("native tools: {}", tools.len());
}

mod ctx {
    use codex_git_attribution::GitAttributionContext;
    use codex_memories::ctx::MemoriesContext;

    /// Host-owned projection exposed to extension contributors.
    #[derive(Clone)]
    pub struct RuntimeContext {
        pub commit_attribution: Option<String>,
        pub memory_tool_enabled: bool,
        pub use_memories: bool,
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
}
