mod citation_output;
pub mod ctx;
mod list_tool;
mod prompt_contributor;
mod tool_contributor;

use std::path::PathBuf;
use std::sync::Arc;

use crate::ctx::MemoriesContext;
use codex_extension_api::CodexExtension;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_memories_read::build_memory_tool_developer_instructions;
use codex_memories_read::memory_root;
use codex_protocol::items::TurnItem;
use codex_utils_absolute_path::AbsolutePathBuf;
use list_tool::ListMemoriesTool;

/// Extension that contributes memories read surfaces.
#[derive(Clone, Debug)]
pub struct MemoriesExtension {
    read_prompt: Option<String>,
    pub(crate) list_tool: Arc<ListMemoriesTool>, // This is just to have useful examples, it will disappear
}

impl<C> CodexExtension<C> for MemoriesExtension
where
    C: MemoriesContext + Send + Sync + 'static,
{
    fn install(self: Arc<Self>, registry: &mut ExtensionRegistryBuilder<C>) {
        registry.tool_contributor(self.clone());
        registry.output_contributor::<TurnItem>(self.clone());
        registry.prompt_contributor(self);
    }
}

impl MemoriesExtension {
    /// Creates a memories extension from the prompt text and backing directory
    /// it should expose.
    pub fn new(read_prompt: Option<String>, memories_root: impl Into<PathBuf>) -> Self {
        Self {
            read_prompt,
            list_tool: Arc::new(ListMemoriesTool::new(memories_root)),
        }
    }

    /// Returns the rendered developer instruction for read access, if available.
    pub fn read_prompt(&self) -> Option<&str> {
        self.read_prompt.as_deref()
    }

    // Just for example
    pub(crate) fn is_read_surface_enabled<C: MemoriesContext>(&self, context: &C) -> bool {
        context.memory_tool_enabled() && context.use_memories()
    }
}

pub async fn extension(codex_home: &AbsolutePathBuf) -> Arc<MemoriesExtension> {
    Arc::new(MemoriesExtension::new(
        build_memory_tool_developer_instructions(codex_home).await,
        memory_root(codex_home).to_path_buf(),
    ))
}
