//! Memories tools and prompt contribution packaged as a Codex extension.

#![forbid(unsafe_code)]

mod citation_output;
pub mod ctx;
mod list_tool;

use std::path::PathBuf;
use std::sync::Arc;

use crate::ctx::MemoriesContext;
use codex_extension_api::CodexExtension;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::PromptFragment;
use codex_extension_api::ToolContribution;
use codex_extension_api::ToolContributor;
use codex_memories_read::build_memory_tool_developer_instructions;
use codex_memories_read::memory_root;
use codex_protocol::items::TurnItem;
use codex_utils_absolute_path::AbsolutePathBuf;
use list_tool::ListMemoriesTool;

/// Extension that contributes memories read surfaces.
#[derive(Clone, Debug)]
pub struct MemoriesExtension {
    read_prompt: Option<String>,
    list_tool: Arc<ListMemoriesTool>,
}

impl<C: MemoriesContext + Send + Sync + 'static> ToolContributor<C> for MemoriesExtension {
    fn tools(&self, context: &C) -> Vec<ToolContribution<C>> {
        if !self.is_read_surface_enabled(context) {
            return Vec::new();
        }

        vec![self.list_tool.contribution()]
    }
}

impl<C: MemoriesContext> ContextContributor<C> for MemoriesExtension {
    fn contribute(&self, context: &C) -> Vec<PromptFragment> {
        if !self.is_read_surface_enabled(context) {
            return Vec::new();
        }

        self.read_prompt()
            .map(PromptFragment::developer_policy)
            .into_iter()
            .collect()
    }
}

impl<C: MemoriesContext + Send + Sync + 'static> CodexExtension<C> for MemoriesExtension {
    fn install(self: Arc<Self>, registry: &mut ExtensionRegistryBuilder<C>) {
        registry.tool_contributor(self.clone());
        registry.output_contributor::<TurnItem>(self.clone());
        registry.prompt_contributor(self);
    }
}

impl MemoriesExtension {
    fn new(read_prompt: Option<String>, memories_root: impl Into<PathBuf>) -> Self {
        Self {
            read_prompt,
            list_tool: Arc::new(ListMemoriesTool::new(memories_root)),
        }
    }

    /// Creates an extension that contributes native tools but no prompt fragment.
    pub fn tools_only(memories_root: impl Into<PathBuf>) -> Self {
        Self::new(None, memories_root)
    }

    /// Creates an extension with one pre-rendered read prompt and native tools.
    pub fn with_read_prompt(read_prompt: String, memories_root: impl Into<PathBuf>) -> Self {
        Self::new(Some(read_prompt), memories_root)
    }

    /// Creates an extension using the live memories read prompt for this Codex home.
    pub async fn from_codex_home(codex_home: &AbsolutePathBuf) -> Self {
        Self::new(
            build_memory_tool_developer_instructions(codex_home).await,
            memory_root(codex_home).to_path_buf(),
        )
    }

    /// Returns the rendered developer instruction for read access, if available.
    pub fn read_prompt(&self) -> Option<&str> {
        self.read_prompt.as_deref()
    }

    fn is_read_surface_enabled<C: MemoriesContext>(&self, context: &C) -> bool {
        context.memory_tool_enabled() && context.use_memories()
    }
}

/// todo Other builder pattern...
pub async fn extension(codex_home: &AbsolutePathBuf) -> Arc<MemoriesExtension> {
    Arc::new(MemoriesExtension::from_codex_home(codex_home).await)
}
