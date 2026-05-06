//! Extension points for codex-core.

use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::tools::registry::ToolHandler as CoreToolHandler;
use crate::tools::registry::ToolKind;
use codex_tools::ToolName;

pub use crate::function_tool::FunctionCallError;
pub use crate::tools::context::FunctionToolOutput;
pub use crate::tools::context::ToolInvocation;

type FunctionHandler = dyn Fn(
        ToolInvocation,
    ) -> Pin<Box<dyn Future<Output = Result<FunctionToolOutput, FunctionCallError>> + Send>>
    + Send
    + Sync;

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

/// Provides tools through codex-core extensibility.
///
/// Implementations are expected to return handlers owned by the provider. Tool
/// specs may still be provided by the existing built-in plan while handlers
/// migrate behind this extension point.
pub trait ToolProvider: Send + Sync + 'static {
    /// Return tool handlers owned by this provider.
    fn handlers(&self) -> Vec<ToolHandler>;
}

/// A tool handler supplied by an extension provider.
pub struct ToolHandler {
    tool_name: ToolName,
    function: Arc<FunctionHandler>,
}

impl ToolHandler {
    /// Wrap a function tool handler for registration with codex-core.
    pub fn function<F, Fut>(tool_name: ToolName, handler: F) -> Self
    where
        F: Fn(ToolInvocation) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<FunctionToolOutput, FunctionCallError>> + Send + 'static,
    {
        Self {
            tool_name,
            function: Arc::new(move |invocation| Box::pin(handler(invocation))),
        }
    }
}

impl CoreToolHandler for ToolHandler {
    type Output = FunctionToolOutput;

    fn tool_name(&self) -> ToolName {
        self.tool_name.clone()
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> impl Future<Output = Result<Self::Output, FunctionCallError>> + Send {
        (self.function)(invocation)
    }
}
