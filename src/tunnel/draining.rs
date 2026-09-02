use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::sync::watch;

/// Shared client-drain state.
///
/// The watch channel retains the latest count.  A waiter subscribes before its
/// final zero check, which prevents the classic `Notify` lost-wakeup race when
/// the last response finishes while shutdown is registering its waiter.
pub(crate) struct DrainState {
    draining: AtomicBool,
    in_flight: AtomicUsize,
    changed: watch::Sender<usize>,
}

impl DrainState {
    pub(crate) fn new() -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            draining: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            changed,
        })
    }

    pub(crate) fn begin(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Register work unless shutdown has started.
    ///
    /// The second drain check closes the race where shutdown begins between
    /// the first check and incrementing the counter.
    pub(crate) fn try_start(self: &Arc<Self>) -> Option<InFlightPermit> {
        if self.is_draining() {
            return None;
        }

        let count = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.changed.send_replace(count);

        let permit = InFlightPermit {
            state: Some(Arc::clone(self)),
        };
        if self.is_draining() {
            drop(permit);
            None
        } else {
            Some(permit)
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub(crate) async fn wait_empty(&self) {
        // Subscribe first, then inspect retained state.  If the last permit is
        // dropped between these operations, `changed()` observes the retained
        // update instead of sleeping forever.
        let mut changed = self.changed.subscribe();
        loop {
            if self.count() == 0 {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    fn finish(&self) {
        let previous = self.in_flight.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0, "in-flight tunnel counter underflow");
        self.changed.send_replace(previous.saturating_sub(1));
    }
}

/// A response owns this permit until its body reaches EOS or is dropped.
pub(crate) struct InFlightPermit {
    state: Option<Arc<DrainState>>,
}

impl Drop for InFlightPermit {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            state.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn final_permit_cannot_be_lost_while_waiter_registers() {
        for _ in 0..1_000 {
            let state = DrainState::new();
            let permit = state.try_start().unwrap();
            state.begin();

            let wait = state.wait_empty();
            tokio::pin!(wait);
            tokio::task::yield_now().await;
            drop(permit);

            tokio::time::timeout(std::time::Duration::from_secs(1), wait)
                .await
                .expect("waiter must observe the final permit")
        }
    }

    #[tokio::test]
    async fn begin_rejects_new_work() {
        let state = DrainState::new();
        state.begin();
        assert!(state.try_start().is_none());
        state.wait_empty().await;
    }
}
