//! Extension points for codex-core.

use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::Config;
use codex_core_plugins::PluginsManager;

pub use crate::function_tool::FunctionCallError;
pub use crate::tools::context::FunctionToolOutput;
pub use crate::tools::context::ToolInvocation;
pub use crate::tools::registry::AnyToolHandler;
pub use crate::tools::registry::ToolHandler;
pub use crate::tools::registry::ToolKind;

/// Stores registered implementations of codex-core extension traits.
///
/// Registrations are keyed by the trait object type used at insertion time. To
/// register an implementation for an extension trait, coerce it to that trait
/// object first, such as `Arc<dyn ToolProvider>`.
#[derive(Clone, Default)]
pub struct ExtensionRegistry {
    extensions: HashMap<TypeId, Vec<Arc<dyn Any + Send + Sync>>>,
}

impl ExtensionRegistry {
    /// Register one implementation for the extension trait `T`.
    pub fn register<T>(&mut self, extension: Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let extension = Arc::new(extension);
        self.extensions
            .entry(TypeId::of::<T>())
            .or_default()
            .push(extension);
    }

    /// Return all registered implementations for the extension trait `T`.
    pub fn get<T>(&self) -> Vec<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.extensions
            .get(&TypeId::of::<T>())
            .into_iter()
            .flat_map(|extensions| extensions.iter())
            .filter_map(|extension| extension.downcast_ref::<Arc<T>>().cloned())
            .collect()
    }

    /// Returns true when no extension traits have been registered.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }
}

/// Runtime context available when extension providers create tool handlers.
#[derive(Clone)]
pub struct ToolProviderContext {
    config: Arc<Config>,
    plugins_manager: Arc<PluginsManager>,
    conversation_id: String,
    turn_id: String,
}

impl ToolProviderContext {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: Arc<Config>,
        plugins_manager: Arc<PluginsManager>,
        conversation_id: String,
        turn_id: String,
    ) -> Self {
        Self {
            config,
            plugins_manager,
            conversation_id,
            turn_id,
        }
    }

    pub fn config(&self) -> Arc<Config> {
        Arc::clone(&self.config)
    }

    pub fn plugins_manager(&self) -> Arc<PluginsManager> {
        Arc::clone(&self.plugins_manager)
    }

    pub fn conversation_id(&self) -> String {
        self.conversation_id.clone()
    }

    pub fn turn_id(&self) -> String {
        self.turn_id.clone()
    }
}

/// Provides tools through codex-core extensibility.
///
/// Implementations are expected to return handlers owned by the provider. Tool
/// specs may still be provided by the existing built-in plan while handlers
/// migrate behind this extension point.
pub trait ToolProvider: Send + Sync + 'static {
    /// Return tool handlers owned by this provider for the current config.
    fn handlers(&self, context: ToolProviderContext) -> Vec<Arc<dyn AnyToolHandler>>;
}
