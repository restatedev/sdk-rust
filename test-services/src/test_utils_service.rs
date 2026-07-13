use futures::FutureExt;
use futures::future::BoxFuture;
use restate_sdk::prelude::*;
use restate_sdk::service::ServiceDefinition;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

#[restate_sdk::handler]
pub(crate) async fn echo(_ctx: Context<'_>, input: String) -> HandlerResult<String> {
    Ok(input)
}

#[restate_sdk::handler(name = "uppercaseEcho")]
pub(crate) async fn uppercase_echo(_ctx: Context<'_>, input: String) -> HandlerResult<String> {
    Ok(input.to_ascii_uppercase())
}

#[restate_sdk::handler(name = "rawEcho")]
pub(crate) async fn raw_echo(_ctx: Context<'_>, input: bytes::Bytes) -> Result<Vec<u8>, Infallible> {
    Ok(input.to_vec())
}

#[restate_sdk::handler(name = "echoHeaders")]
pub(crate) async fn echo_headers(ctx: Context<'_>) -> HandlerResult<Json<HashMap<String, String>>> {
    let mut headers = HashMap::new();
    for k in ctx.headers().keys() {
        headers.insert(k.as_str().to_owned(), ctx.headers().get(k).unwrap().clone());
    }

    Ok(headers.into())
}

#[restate_sdk::handler(name = "sleepConcurrently")]
pub(crate) async fn sleep_concurrently(
    ctx: Context<'_>,
    millis_durations: Json<Vec<u64>>,
) -> HandlerResult<()> {
    let mut futures: Vec<BoxFuture<'_, Result<(), TerminalError>>> = vec![];

    for duration in millis_durations.into_inner() {
        futures.push(ctx.sleep(Duration::from_millis(duration)).boxed());
    }

    for fut in futures {
        fut.await?;
    }

    Ok(())
}

#[restate_sdk::handler(name = "countExecutedSideEffects")]
pub(crate) async fn count_executed_side_effects(
    ctx: Context<'_>,
    increments: u32,
) -> HandlerResult<u32> {
    let counter: Arc<AtomicU8> = Default::default();

    for _ in 0..increments {
        let counter_clone = Arc::clone(&counter);
        ctx.run(|| async {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await?;
    }

    Ok(counter.load(Ordering::SeqCst) as u32)
}

#[restate_sdk::handler(name = "cancelInvocation")]
pub(crate) async fn cancel_invocation(
    ctx: Context<'_>,
    invocation_id: String,
) -> Result<(), TerminalError> {
    ctx.invocation_handle(invocation_id).cancel().await?;
    Ok(())
}

pub(crate) fn definition() -> ServiceDefinition {
    service!(
        "TestUtilsService",
        echo,
        uppercase_echo,
        raw_echo,
        echo_headers,
        sleep_concurrently,
        count_executed_side_effects,
        cancel_invocation
    )
}
