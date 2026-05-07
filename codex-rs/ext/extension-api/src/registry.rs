use std::sync::Arc;

use crate::CodexExtension;
use crate::McpToolContributor;
use crate::PromptContributor;

/// Mutable registry used while extensions install their typed contributions.
pub struct ExtensionRegistryBuilder<C> {
    mcp_tool_contributors: Vec<Arc<dyn McpToolContributor<C>>>,
    prompt_contributors: Vec<Arc<dyn PromptContributor<C>>>,
}

impl<C> Default for ExtensionRegistryBuilder<C> {
    fn default() -> Self {
        Self {
            mcp_tool_contributors: Vec::new(),
            prompt_contributors: Vec::new(),
        }
    }
}

impl<C> ExtensionRegistryBuilder<C> {
    /// Creates an empty registry builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs one extension and returns the builder.
    #[must_use]
    pub fn with_extension<E>(mut self, extension: Arc<E>) -> Self
    where
        E: CodexExtension<C> + 'static,
    {
        self.install_extension(extension);
        self
    }

    /// Installs one extension into the registry under construction.
    pub fn install_extension<E>(&mut self, extension: Arc<E>)
    where
        E: CodexExtension<C> + 'static,
    {
        extension.install(self);
    }

    /// Registers one MCP tool contributor.
    pub fn mcp_tool_contributor(&mut self, contributor: Arc<dyn McpToolContributor<C>>) {
        self.mcp_tool_contributors.push(contributor);
    }

    /// Registers one prompt contributor.
    pub fn prompt_contributor(&mut self, contributor: Arc<dyn PromptContributor<C>>) {
        self.prompt_contributors.push(contributor);
    }

    /// Finishes construction and returns the immutable registry.
    pub fn build(self) -> ExtensionRegistry<C> {
        ExtensionRegistry {
            mcp_tool_contributors: self.mcp_tool_contributors,
            prompt_contributors: self.prompt_contributors,
        }
    }
}

/// Immutable typed registry produced after extensions are installed.
pub struct ExtensionRegistry<C> {
    mcp_tool_contributors: Vec<Arc<dyn McpToolContributor<C>>>,
    prompt_contributors: Vec<Arc<dyn PromptContributor<C>>>,
}

impl<C> ExtensionRegistry<C> {
    /// Returns the registered MCP tool contributors.
    pub fn mcp_tool_contributors(&self) -> &[Arc<dyn McpToolContributor<C>>] {
        &self.mcp_tool_contributors
    }

    /// Returns the registered prompt contributors.
    pub fn prompt_contributors(&self) -> &[Arc<dyn PromptContributor<C>>] {
        &self.prompt_contributors
    }
}
