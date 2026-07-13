//! Ambient, endpoint-lifetime shared state (dependency injection).
//!
//! Values registered with `.state(...)` on the service or endpoint builder are stored here,
//! keyed by their type, and retrieved inside handlers via
//! [`ContextState::state`](crate::context::ContextState::state).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// A type-indexed set of shared, `Send + Sync` values.
///
/// Used for dependency injection: a value of type `T` is stored under `TypeId::of::<T>()` and
/// retrieved by type. Cloning is cheap (values are behind [`Arc`]).
#[derive(Default, Clone)]
pub(crate) struct StateMap {
    inner: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl StateMap {
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
    /// Used to layer service-level state over endpoint-level state, with the service winning.
    pub(crate) fn overlay(&mut self, higher: &StateMap) {
        self.inner
            .extend(higher.inner.iter().map(|(k, v)| (*k, Arc::clone(v))));
    }
}
