use crate::endpoint::ContextInternal;
use restate_sdk_shared_core::NotificationHandle;

// Sealed future trait, used by select statement
#[doc(hidden)]
pub trait SealedDurableFuture {
    fn inner_context(&self) -> ContextInternal;
    /// Registration failures trap without a notification handle.
    fn handle(&self) -> Option<NotificationHandle>;
}
