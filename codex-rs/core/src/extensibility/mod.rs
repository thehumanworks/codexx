//! Extension points for codex-core.

use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

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
/// Implementations are markers for now; tool-surface methods will be added as
/// the extensibility API takes shape.
pub trait ToolProvider: Send + Sync + 'static {}
