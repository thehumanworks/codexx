mod contributors;
pub mod ctx;
mod tools;

use std::path::PathBuf;
use std::sync::Arc;

use crate::ctx::MemoriesReadConfig;
use codex_core::config::Config;
use codex_extension_api::CodexExtension;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadStartContributor;
use codex_features::Feature;
use codex_memories_read::build_memory_tool_developer_instructions;
use codex_memories_read::memory_root;
use codex_utils_absolute_path::AbsolutePathBuf;
use contributors::CitationContributor;
use contributors::PromptContributor;
use contributors::ToolsContributor;
use tools::MemoriesTools;

/// Extension that contributes memories read surfaces.
#[derive(Clone, Debug)]
pub struct MemoriesExtension {
    prompt: Arc<PromptContributor>,
    tools: Arc<ToolsContributor>,
    citations: Arc<CitationContributor>,
}

impl CodexExtension<Config> for MemoriesExtension {
    fn install(self: Arc<Self>, registry: &mut ExtensionRegistryBuilder<Config>) {
        registry.thread_start_contributor(self.clone());
        registry.prompt_contributor(self.prompt.clone());
        registry.tool_contributor(self.tools.clone());
        registry.turn_item_contributor(self.citations.clone());
    }
}

impl ThreadStartContributor<Config> for MemoriesExtension {
    fn contribute(
        &self,
        config: &Config,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) {
        thread_store.insert(MemoriesReadConfig {
            enabled: config.features.enabled(Feature::MemoryTool) && config.memories.use_memories,
        });
    }
}

impl MemoriesExtension {
    /// Creates a memories extension from the prompt text and backing directory
    /// it should expose.
    pub fn new(read_prompt: Option<String>, memories_root: impl Into<PathBuf>) -> Self {
        Self {
            prompt: Arc::new(PromptContributor::new(read_prompt)),
            tools: Arc::new(ToolsContributor::new(MemoriesTools::new(memories_root))),
            citations: Arc::new(CitationContributor),
        }
    }
}

pub async fn extension(codex_home: &AbsolutePathBuf) -> Arc<MemoriesExtension> {
    Arc::new(MemoriesExtension::new(
        build_memory_tool_developer_instructions(codex_home).await,
        memory_root(codex_home).to_path_buf(),
    ))
}
