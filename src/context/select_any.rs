use crate::context::DurableFuture;
use crate::errors::TerminalError;

/// A collection of durable futures that yields results as they complete,
/// similar to [`futures::stream::FuturesUnordered`].
///
/// Each future is assigned a stable index when pushed, which is returned
/// alongside the result from [`next`](Self::next) so you can correlate
/// outputs with inputs.
///
/// # Example
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # use std::time::Duration;
/// #
/// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
/// let labels = vec!["fast", "medium", "slow"];
/// let durations = vec![1, 2, 3];
///
/// let mut futures = DurableFuturesUnordered::new();
/// for secs in &durations {
///     futures.push(ctx.sleep(Duration::from_secs(*secs)));
/// }
///
/// while let Some((index, result)) = futures.next().await? {
///     result?;
///     println!("{} timer done!", labels[index]);
/// }
/// #    Ok(())
/// # }
/// ```
///
/// You can also collect into it from an iterator:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # use std::time::Duration;
/// #
/// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
/// let durations = vec![1, 2, 3];
/// let mut futures: DurableFuturesUnordered<_> = durations
///     .iter()
///     .map(|secs| ctx.sleep(Duration::from_secs(*secs)))
///     .collect();
///
/// while let Some((index, result)) = futures.next().await? {
///     result?;
/// }
/// #    Ok(())
/// # }
/// ```
pub struct DurableFuturesUnordered<F> {
    futures: Vec<(usize, F)>,
    next_index: usize,
}

impl<F> DurableFuturesUnordered<F> {
    /// Create an empty collection.
    pub fn new() -> Self {
        Self {
            futures: Vec::new(),
            next_index: 0,
        }
    }

    /// Add a durable future to the collection.
    /// Returns the stable index assigned to this future.
    pub fn push(&mut self, future: F) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        self.futures.push((index, future));
        index
    }

    /// Returns `true` if there are no remaining futures.
    pub fn is_empty(&self) -> bool {
        self.futures.is_empty()
    }

    /// Returns the number of remaining futures.
    pub fn len(&self) -> usize {
        self.futures.len()
    }
}

impl<F> Default for DurableFuturesUnordered<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F> FromIterator<F> for DurableFuturesUnordered<F> {
    fn from_iter<I: IntoIterator<Item = F>>(iter: I) -> Self {
        let futures: Vec<_> = iter.into_iter().enumerate().collect();
        let next_index = futures.len();
        Self {
            futures,
            next_index,
        }
    }
}

impl<F: DurableFuture> DurableFuturesUnordered<F> {
    /// Await the next completed future.
    ///
    /// Returns `Ok(None)` if there are no remaining futures.
    /// Returns `Err(TerminalError)` if the invocation was cancelled.
    /// Returns `Ok(Some((index, output)))` with the stable index and output
    /// of the next completed future.
    pub async fn next(&mut self) -> Result<Option<(usize, F::Output)>, TerminalError> {
        if self.futures.is_empty() {
            return Ok(None);
        }

        let handles: Vec<_> = self.futures.iter().map(|(_, f)| f.handle()).collect();
        let ctx = self.futures[0].1.inner_context();
        let pos = ctx.select(handles).await?;
        let (index, future) = self.futures.swap_remove(pos);
        let output = future.await;
        Ok(Some((index, output)))
    }
}
