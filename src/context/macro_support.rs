use crate::endpoint::ContextInternal;
use restate_sdk_shared_core::NotificationHandle;

// Sealed future trait, used by select statement
pub trait SealedDurableFuture {
    fn inner_context(&self) -> ContextInternal;
    fn handle(&self) -> NotificationHandle;
}
