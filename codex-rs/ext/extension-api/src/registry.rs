use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ApprovalInterceptorContributor;
use crate::CodexExtension;
use crate::ContextContributor;
use crate::OutputContributor;
use crate::ToolContributor;

/// Mutable registry used while extensions install their typed contributions.
pub struct ExtensionRegistryBuilder<C> {
    context_contributors: Vec<Arc<dyn ContextContributor<C>>>,
    tool_contributors: Vec<Arc<dyn ToolContributor<C>>>,
    output_contributors: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    approval_interceptor_contributors: Vec<Arc<dyn ApprovalInterceptorContributor<C>>>,
}

impl<C> Default for ExtensionRegistryBuilder<C> {
    fn default() -> Self {
        Self {
            approval_interceptor_contributors: Vec::new(),
            context_contributors: Vec::new(),
            tool_contributors: Vec::new(),
            output_contributors: HashMap::new(),
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

    /// Registers one ordered output contributor for output type `O`.
    pub fn output_contributor<O>(&mut self, contributor: Arc<dyn OutputContributor<C, O>>)
    where
        C: 'static,
        O: 'static,
    {
        let Some(contributors) = self
            .output_contributors
            .entry(TypeId::of::<O>())
            .or_insert_with(|| Box::new(Vec::<Arc<dyn OutputContributor<C, O>>>::new()))
            .downcast_mut::<Vec<Arc<dyn OutputContributor<C, O>>>>()
        else {
            unreachable!("output contributor bucket type must match its registered output type");
        };
        contributors.push(contributor);
    }

    /// Finishes construction and returns the immutable registry.
    pub fn build(self) -> ExtensionRegistry<C> {
        ExtensionRegistry {
            approval_interceptor_contributors: self.approval_interceptor_contributors,
            context_contributors: self.context_contributors,
            tool_contributors: self.tool_contributors,
            output_contributors: self.output_contributors,
        }
    }
}

/// Immutable typed registry produced after extensions are installed.
pub struct ExtensionRegistry<C> {
    context_contributors: Vec<Arc<dyn ContextContributor<C>>>,
    tool_contributors: Vec<Arc<dyn ToolContributor<C>>>,
    output_contributors: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    approval_interceptor_contributors: Vec<Arc<dyn ApprovalInterceptorContributor<C>>>,
}

impl<C> ExtensionRegistry<C> {
    /// Returns the registered approval interceptor contributors.
    pub fn approval_interceptor_contributors(
        &self,
    ) -> &[Arc<dyn ApprovalInterceptorContributor<C>>] {
        &self.approval_interceptor_contributors
    }

    /// Returns the registered prompt contributors.
    pub fn context_contributors(&self) -> &[Arc<dyn ContextContributor<C>>] {
        &self.context_contributors
    }

    /// Returns the registered native tool contributors.
    pub fn tool_contributors(&self) -> &[Arc<dyn ToolContributor<C>>] {
        &self.tool_contributors
    }

    /// Returns the registered ordered output contributors for output type `O`.
    pub fn output_contributors<O>(&self) -> &[Arc<dyn OutputContributor<C, O>>]
    where
        C: 'static,
        O: 'static,
    {
        self.output_contributors
            .get(&TypeId::of::<O>())
            .and_then(|contributors| {
                contributors.downcast_ref::<Vec<Arc<dyn OutputContributor<C, O>>>>()
            })
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}
