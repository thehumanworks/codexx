use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use codex_extension_api::CodexExtension;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::scopes::Thread;
use codex_extension_api::scopes::Turn;
use codex_git_attribution as git_attribution;
use codex_memories::MemoriesExtension;

fn main() {
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
    let context = ctx::RuntimeContext {
        commit_attribution: None,
        memory_tool_enabled: true,
        use_memories: true,
    };

    // 4. Assemble the stores that exist at this insertion point.
    //
    //    Another insertion point could expose only `Thread`, or could add more
    //    scopes later. Contributors receive the same dynamic bag and address the
    //    scopes they need by marker type. The idea would be to have some re-usable contributors
    //    such as the Output one, but I'm happy to negociate this one
    let thread_data = ExtensionData::new();
    let turn_data = ExtensionData::new();
    let stores = codex_extension_api::stores! {
        Thread => &thread_data,
        Turn => &turn_data,
    };

    // 5. Invoke whichever contribution families this insertion point needs.
    let prompt_fragments = registry
        .context_contributors()
        .iter()
        .flat_map(|contributor| contributor.contribute(&context, &stores))
        .collect::<Vec<_>>();

    let tools = registry
        .tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&context, &stores))
        .collect::<Vec<_>>();

    println!("prompt fragments: {}", prompt_fragments.len());
    println!("native tools: {}", tools.len());
}

// Just for the machinerie such that it compiles
#[derive(Debug, Default)]
struct ThreadPromptStats {
    prompt_builds: AtomicU64,
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
