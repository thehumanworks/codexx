use std::sync::Arc;

use crate::ApprovalInterceptorContributor;
use crate::CodexExtension;
use crate::ContextContributor;
use crate::ToolContributor;

/// Mutable registry used while extensions install their typed contributions.
pub struct ExtensionRegistryBuilder<C> {
    context_contributors: Vec<Arc<dyn ContextContributor<C>>>,
    tool_contributors: Vec<Arc<dyn ToolContributor<C>>>,
    approval_interceptor_contributors: Vec<Arc<dyn ApprovalInterceptorContributor<C>>>,
}

impl<C> Default for ExtensionRegistryBuilder<C> {
    fn default() -> Self {
        Self {
            approval_interceptor_contributors: Vec::new(),
            context_contributors: Vec::new(),
            tool_contributors: Vec::new(),
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

    /// Registers one approval interceptor contributor.
    pub fn approval_interceptor_contributor(
        &mut self,
        contributor: Arc<dyn ApprovalInterceptorContributor<C>>,
    ) {
        self.approval_interceptor_contributors.push(contributor);
    }

    /// Registers one prompt contributor.
    pub fn prompt_contributor(&mut self, contributor: Arc<dyn ContextContributor<C>>) {
        self.context_contributors.push(contributor);
    }

    /// Registers one native tool contributor.
    pub fn tool_contributor(&mut self, contributor: Arc<dyn ToolContributor<C>>) {
        self.tool_contributors.push(contributor);
    }

    /// Finishes construction and returns the immutable registry.
    pub fn build(self) -> ExtensionRegistry<C> {
        ExtensionRegistry {
            approval_interceptor_contributors: self.approval_interceptor_contributors,
            prompt_contributors: self.context_contributors,
            tool_contributors: self.tool_contributors,
        }
    }
}

/// Immutable typed registry produced after extensions are installed.
pub struct ExtensionRegistry<C> {
    approval_interceptor_contributors: Vec<Arc<dyn ApprovalInterceptorContributor<C>>>,
    prompt_contributors: Vec<Arc<dyn ContextContributor<C>>>,
    tool_contributors: Vec<Arc<dyn ToolContributor<C>>>,
}

impl<C> ExtensionRegistry<C> {
    /// Returns the registered approval interceptor contributors.
    pub fn approval_interceptor_contributors(
        &self,
    ) -> &[Arc<dyn ApprovalInterceptorContributor<C>>] {
        &self.approval_interceptor_contributors
    }

    /// Returns the registered prompt contributors.
    pub fn prompt_contributors(&self) -> &[Arc<dyn ContextContributor<C>>] {
        &self.prompt_contributors
    }

    /// Returns the registered native tool contributors.
    pub fn tool_contributors(&self) -> &[Arc<dyn ToolContributor<C>>] {
        &self.tool_contributors
    }
}
