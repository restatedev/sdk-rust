//! Endpoint-lifetime injected dependencies (extensions).
//!
//! Values registered with `.extension(...)` on the service or endpoint builder are stored here,
//! keyed by their type, and retrieved inside handlers via
//! [`ContextExtensions::extension`](crate::context::ContextExtensions::extension).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// A type-indexed set of shared, `Send + Sync` values.
///
/// Used for dependency injection: a value of type `T` is stored under `TypeId::of::<T>()` and
/// retrieved by type. Cloning is cheap (values are behind [`Arc`]).
#[derive(Default, Clone)]
pub(crate) struct ExtensionMap {
    inner: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl ExtensionMap {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert<T: Any + Send + Sync>(&mut self, value: T) {
        self.inner.insert(TypeId::of::<T>(), Arc::new(value));
    }

    pub(crate) fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.inner
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
    }

    /// Overlay `higher` on top of `self`: entries present in `higher` override those in `self`.
    ///
    /// Used to layer service-level extensions over endpoint-level ones, with the service winning.
    pub(crate) fn overlay(&mut self, higher: &ExtensionMap) {
        self.inner
            .extend(higher.inner.iter().map(|(k, v)| (*k, Arc::clone(v))));
    }
}
