use crate::context::DurableFuture;
use crate::errors::TerminalError;

/// Awaits the first completed future from a dynamic collection of durable futures,
/// removing the completed future from the Vec via `swap_remove`.
///
/// Returns `Ok((index, output))` where `index` is the position of the completed
/// future in the Vec *before* removal.
/// Returns `Err(TerminalError)` if the invocation is cancelled.
///
/// # Panics
///
/// Panics if `futures` is empty.
///
/// # Example
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # use std::time::Duration;
/// #
/// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
/// let mut futures = vec![
///     ctx.sleep(Duration::from_secs(1)),
///     ctx.sleep(Duration::from_secs(2)),
///     ctx.sleep(Duration::from_secs(3)),
/// ];
///
/// // Process all results as they complete
/// while !futures.is_empty() {
///     let (_index, result) = restate_sdk::select_any(&mut futures).await?;
///     // process result
/// }
/// #    Ok(())
/// # }
/// ```
pub async fn select_any<F>(futures: &mut Vec<F>) -> Result<(usize, F::Output), TerminalError>
where
    F: DurableFuture,
{
    assert!(
        !futures.is_empty(),
        "select_any requires at least one future"
    );

    let handles: Vec<_> = futures.iter().map(|f| f.handle()).collect();
    let ctx = futures[0].inner_context();
    let index = ctx.select(handles).await?;
    let future = futures.swap_remove(index);
    let output = future.await;
    Ok((index, output))
}
