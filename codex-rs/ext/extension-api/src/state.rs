use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

type ErasedData = Arc<dyn Any + Send + Sync>;

/// Built-in extension-store scope markers.
pub mod scopes {
    pub enum Thread {}
    pub enum Turn {}
}

/// Typed extension-owned data attached to one host object.
#[derive(Default, Debug)]
pub struct ExtensionData {
    entries: Mutex<HashMap<TypeId, ErasedData>>,
}

impl ExtensionData {
    /// Creates an empty attachment map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the attached value of type `T`, if one exists.
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        let value = self.entries().get(&TypeId::of::<T>())?.clone();
        Some(downcast_data(value))
    }

    /// Returns the attached value of type `T`, inserting one from `init` when absent.
    ///
    /// The initializer runs while this map is locked, so it should stay cheap;
    /// heavyweight lazy work belongs inside the attached value itself.
    pub fn get_or_init<T>(&self, init: impl FnOnce() -> T) -> Arc<T>
    where
        T: Any + Send + Sync,
    {
        let mut entries = self.entries();
        let value = entries
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(init()));
        downcast_data(Arc::clone(value))
    }

    /// Stores `value` as the attachment of type `T`, returning any previous value.
    pub fn insert<T>(&self, value: T) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.entries()
            .insert(TypeId::of::<T>(), Arc::new(value))
            .map(downcast_data)
    }

    /// Removes and returns the attached value of type `T`, if one exists.
    pub fn remove<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.entries().remove(&TypeId::of::<T>()).map(downcast_data)
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<TypeId, ErasedData>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Dynamic set of host-owned extension stores visible at one contribution site.
///
/// The host decides which lifetime scopes are available for each insertion
/// point. Contributors can then address one of those visible scopes by marker
/// type while storing extension-private values inside the selected owner.
#[derive(Default, Debug)]
pub struct Stores<'a> {
    stores: HashMap<TypeId, &'a ExtensionData>,
}

impl<'a> Stores<'a> {
    /// Creates an empty set of visible stores.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces the store identified by scope marker `Scope`.
    pub fn insert_scope<Scope: 'static>(
        &mut self,
        store: &'a ExtensionData,
    ) -> Option<&'a ExtensionData> {
        self.stores.insert(TypeId::of::<Scope>(), store)
    }

    /// Returns the value of type `T` from `Scope`, if one exists.
    ///
    /// # Panics
    ///
    /// Panics when `Scope` is unavailable at this contribution site. That is a
    /// host/contributor wiring bug: insertion points define which scopes exist,
    /// and contributors should only request scopes promised by their insertion point.
    pub fn get<Scope, T>(&self) -> Option<Arc<T>>
    where
        Scope: 'static,
        T: Any + Send + Sync,
    {
        self.store::<Scope>().get::<T>()
    }

    /// Returns the value of type `T`, inserting one into `Scope` when absent.
    ///
    /// # Panics
    ///
    /// Panics when `Scope` is unavailable at this contribution site. That is a
    /// host/contributor wiring bug: insertion points define which scopes exist,
    /// and contributors should only request scopes promised by their insertion point.
    pub fn get_or_init<Scope, T>(&self, init: impl FnOnce() -> T) -> Arc<T>
    where
        Scope: 'static,
        T: Any + Send + Sync,
    {
        self.store::<Scope>().get_or_init(init)
    }

    /// Returns the value of type `T`, inserting one into `Scope` when available.
    ///
    /// Use this when a contributor intentionally supports multiple insertion
    /// points and can proceed without state from a scope that is not visible.
    pub fn try_get_or_init<Scope, T>(&self, init: impl FnOnce() -> T) -> Option<Arc<T>>
    where
        Scope: 'static,
        T: Any + Send + Sync,
    {
        self.try_store::<Scope>()
            .map(|store| store.get_or_init(init))
    }

    /// Stores `value` in `Scope`, returning any previous value of type `T`.
    ///
    /// # Panics
    ///
    /// Panics when `Scope` is unavailable at this contribution site. That is a
    /// host/contributor wiring bug: insertion points define which scopes exist,
    /// and contributors should only request scopes promised by their insertion point.
    pub fn insert<Scope, T>(&self, value: T) -> Option<Arc<T>>
    where
        Scope: 'static,
        T: Any + Send + Sync,
    {
        self.store::<Scope>().insert(value)
    }

    /// Removes and returns the value of type `T` from `Scope`, if one exists.
    ///
    /// # Panics
    ///
    /// Panics when `Scope` is unavailable at this contribution site. That is a
    /// host/contributor wiring bug: insertion points define which scopes exist,
    /// and contributors should only request scopes promised by their insertion point.
    pub fn remove<Scope, T>(&self) -> Option<Arc<T>>
    where
        Scope: 'static,
        T: Any + Send + Sync,
    {
        self.store::<Scope>().remove::<T>()
    }

    fn try_store<Scope: 'static>(&self) -> Option<&ExtensionData> {
        self.stores.get(&TypeId::of::<Scope>()).copied()
    }

    fn store<Scope: 'static>(&self) -> &ExtensionData {
        self.try_store::<Scope>().unwrap_or_else(|| {
            // This panic means a mistake made by a developer!!!
            panic!(
                "extension store for scope `{}` is unavailable",
                std::any::type_name::<Scope>()
            )
        })
    }
}

/// Builds a dynamic [`Stores`] bag from the scopes available at one insertion point.
///
/// I know people don't like macros but this is super cool IMO
#[macro_export]
macro_rules! stores {
    ($($scope:ty => $store:expr),* $(,)?) => {{
        let mut stores = $crate::Stores::new();
        $(stores.insert_scope::<$scope>($store);)*
        stores
    }};
}

fn downcast_data<T>(value: ErasedData) -> Arc<T>
where
    T: Any + Send + Sync,
{
    let Ok(value) = value.downcast::<T>() else {
        unreachable!("typed extension data stored an incompatible value");
    };
    value
}
