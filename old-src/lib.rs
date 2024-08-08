mod state;

pub use state::Store;

use crate::RestateOutput::{Completed, Suspended};
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use tokio::sync::mpsc;
use {
    futures::{
        future::{BoxFuture, FutureExt},
        task::{waker_ref, ArcWake},
    },
    std::{
        future::Future,
        sync::{Arc, Mutex},
        task::Context,
    },
};

#[derive(Clone)]
struct RestateJournal {
    is_suspended: Arc<AtomicBool>,
}

impl Default for RestateJournal {
    fn default() -> Self {
        Self {
            is_suspended: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl RestateJournal {
    fn is_suspended(&self) -> bool {
        self.is_suspended.load(Ordering::Acquire)
    }

    fn suspend(&self) {
        self.is_suspended.store(true, Ordering::Release)
    }
}

#[derive(Debug, PartialEq)]
enum RestateOutput<T: Debug + PartialEq> {
    Suspended,
    Completed(T),
}

/// Task executor that receives tasks off of a channel and runs them.
struct Executor<T: Debug> {
    journal: RestateJournal,

    task_rx: mpsc::UnboundedReceiver<Arc<Task<T>>>,
    task_tx: mpsc::UnboundedSender<Arc<Task<T>>>,
}

/// A future that can reschedule itself to be polled by an `Executor`.
struct Task<T: Debug> {
    /// In-progress future that should be pushed to completion.
    ///
    /// The `Mutex` is not necessary for correctness, since we only have
    /// one thread executing tasks at once. However, Rust isn't smart
    /// enough to know that `future` is only mutated from one thread,
    /// so we need to use the `Mutex` to prove thread-safety. A production
    /// executor would not need this, and could use `UnsafeCell` instead.
    future: Mutex<Option<BoxFuture<'static, T>>>,

    /// Handle to place the task itself back onto the task queue.
    task_tx: mpsc::UnboundedSender<Arc<Task<T>>>,
}

impl<T: Debug> Debug for Task<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl<T: Debug> ArcWake for Task<T> {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        // Implement `wake` by sending this task back onto the task channel
        // so that it will be polled again by the executor.
        let cloned = arc_self.clone();
        arc_self
            .task_tx
            .send(cloned)
            .expect("unexpected closed channel");
    }
}

impl<T: Debug + PartialEq> Executor<T> {
    fn new(journal: RestateJournal) -> Self {
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        Self {
            journal,
            task_rx,
            task_tx,
        }
    }

    async fn run(
        &mut self,
        future: impl Future<Output = T> + 'static + Send,
    ) -> Result<RestateOutput<T>, ()> {
        let future = future.boxed();
        let task = Arc::new(Task {
            future: Mutex::new(Some(future)),
            task_tx: self.task_tx.clone(),
        });
        self.task_tx.send(task).expect("unexpected closed channel");

        while let Some(task) = self.task_rx.recv().await {
            // Take the future, and if it has not yet completed (is still Some),
            // poll it in an attempt to complete it.
            let mut future_slot = task.future.lock().unwrap();
            if let Some(mut future) = future_slot.take() {
                // Create a `LocalWaker` from the task itself
                let waker = waker_ref(&task);
                let context = &mut Context::from_waker(&*waker);
                // `BoxFuture<T>` is a type alias for
                // `Pin<Box<dyn Future<Output = T> + Send + 'static>>`.
                // We can get a `Pin<&mut dyn Future + Send + 'static>`
                // from it by calling the `Pin::as_mut` method.
                match future.as_mut().poll(context) {
                    Poll::Pending => {
                        *future_slot = Some(future);
                    }
                    Poll::Ready(out) => return Ok(Completed(out)),
                }
                if self.journal.is_suspended() {
                    return Ok(Suspended);
                }
            }
        }
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::ready;
    use std::future::pending;

    #[tokio::test]
    async fn completed() {
        let journal = RestateJournal::default();
        let mut executor = Executor::new(journal.clone());

        let res = executor.run(async { ready(123).await }).await.unwrap();

        assert_eq!(Completed(123), res);
    }

    #[tokio::test]
    async fn suspended() {
        let journal = RestateJournal::default();
        let mut executor = Executor::new(journal.clone());

        let res = executor
            .run(async move {
                journal.suspend();
                pending::<i32>().await
            })
            .await
            .unwrap();

        assert_eq!(Suspended, res);
    }
}
