//! Memories tools and prompt contribution packaged as a Codex extension.

#![forbid(unsafe_code)]

use std::sync::Arc;

use codex_extension_api::CodexExtension;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::McpToolContributor;
use codex_extension_api::PromptContributor;
use codex_extension_api::PromptFragment;
use codex_memories_read::build_memory_tool_developer_instructions;
use codex_utils_absolute_path::AbsolutePathBuf;
use rmcp::model::Tool;
use rmcp::model::ToolAnnotations;
use serde_json::Map;
use serde_json::Value;

/// Runtime facts needed to decide whether read-memory surfaces are visible.
///
/// Hosts should expose the current effective values for the thread being
/// assembled. The extension owns the policy that combines those values.
pub trait MemoriesContext {
    fn memory_tool_enabled(&self) -> bool;
    fn use_memories(&self) -> bool;
}

/// Extension that contributes memories MCP tools plus their read-path guidance.
#[derive(Clone, Debug)]
pub struct MemoriesExtension {
    read_prompt: Option<String>,
}

impl MemoriesExtension {
    /// Creates an extension from a pre-rendered read prompt.
    pub fn new(read_prompt: Option<String>) -> Self {
        Self { read_prompt }
    }

    /// Creates an extension using the live memories read prompt for this Codex home.
    pub async fn from_codex_home(codex_home: &AbsolutePathBuf) -> Self {
        Self {
            read_prompt: build_memory_tool_developer_instructions(codex_home).await,
        }
    }

    /// Returns the rendered developer instruction for read access, if available.
    pub fn read_prompt(&self) -> Option<&str> {
        self.read_prompt.as_deref()
    }

    fn is_read_surface_enabled<C: MemoriesContext>(&self, context: &C) -> bool {
        context.memory_tool_enabled() && context.use_memories()
    }
}

impl<C: MemoriesContext> McpToolContributor<C> for MemoriesExtension {
    fn tools(&self, context: &C) -> Vec<Tool> {
        if !self.is_read_surface_enabled(context) {
            return Vec::new();
        }

        vec![
            simple_tool("list", "List memory entries."),
            simple_tool("read", "Read one memory entry."),
            simple_tool("search", "Search memory entries."),
        ]
    }
}

impl<C: MemoriesContext> PromptContributor<C> for MemoriesExtension {
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

impl<C: MemoriesContext> CodexExtension<C> for MemoriesExtension {
    fn install(self: Arc<Self>, registry: &mut ExtensionRegistryBuilder<C>) {
        registry.mcp_tool_contributor(self.clone());
        registry.prompt_contributor(self);
    }
}

/// Creates a shared memories extension using the live memories read prompt.
pub async fn extension(codex_home: &AbsolutePathBuf) -> Arc<MemoriesExtension> {
    Arc::new(MemoriesExtension::from_codex_home(codex_home).await)
}

fn simple_tool(name: &'static str, description: &'static str) -> Tool {
    Tool::new(name, description, simple_input_schema())
        .annotate(ToolAnnotations::new().read_only(true))
}

fn simple_input_schema() -> Map<String, Value> {
    Map::from_iter([("type".to_string(), Value::String("object".to_string()))])
}
