use futures::future::BoxFuture;
use futures::FutureExt;
use restate_sdk::prelude::*;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[restate_sdk::service]
#[name = "TestUtilsService"]
pub(crate) trait TestUtilsService {
    #[name = "echo"]
    async fn echo(input: String) -> HandlerResult<String>;
    #[name = "uppercaseEcho"]
    async fn uppercase_echo(input: String) -> HandlerResult<String>;
    #[name = "rawEcho"]
    async fn raw_echo(input: bytes::Bytes) -> Result<Vec<u8>, Infallible>;
    #[name = "echoHeaders"]
    async fn echo_headers() -> HandlerResult<Json<HashMap<String, String>>>;
    #[name = "sleepConcurrently"]
    async fn sleep_concurrently(millis_durations: Json<Vec<u64>>) -> HandlerResult<()>;
    #[name = "countExecutedSideEffects"]
    async fn count_executed_side_effects(increments: u32) -> HandlerResult<u32>;
    #[name = "cancelInvocation"]
    async fn cancel_invocation(invocation_id: String) -> Result<(), TerminalError>;
}

pub(crate) struct TestUtilsServiceImpl;

impl TestUtilsService for TestUtilsServiceImpl {
    async fn echo(&self, _: Context<'_>, input: String) -> HandlerResult<String> {
        Ok(input)
    }

    async fn uppercase_echo(&self, _: Context<'_>, input: String) -> HandlerResult<String> {
        Ok(input.to_ascii_uppercase())
    }

    async fn raw_echo(&self, _: Context<'_>, input: bytes::Bytes) -> Result<Vec<u8>, Infallible> {
        Ok(input.to_vec())
    }

    async fn echo_headers(
        &self,
        context: Context<'_>,
    ) -> HandlerResult<Json<HashMap<String, String>>> {
        let mut headers = HashMap::new();
        for k in context.headers().keys() {
            headers.insert(
                k.as_str().to_owned(),
                context.headers().get(k).unwrap().clone(),
            );
        }

        Ok(headers.into())
    }

    async fn sleep_concurrently(
        &self,
        context: Context<'_>,
        millis_durations: Json<Vec<u64>>,
    ) -> HandlerResult<()> {
        let mut futures: Vec<BoxFuture<'_, Result<(), TerminalError>>> = vec![];

        for duration in millis_durations.into_inner() {
            futures.push(context.sleep(Duration::from_millis(duration)).boxed());
        }

        for fut in futures {
            fut.await?;
        }

        Ok(())
    }

    async fn count_executed_side_effects(
        &self,
        context: Context<'_>,
        increments: u32,
    ) -> HandlerResult<u32> {
        let counter: Arc<AtomicU8> = Default::default();

        for _ in 0..increments {
            let counter_clone = Arc::clone(&counter);
            context
                .run(|| async {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await?;
        }

        Ok(counter.load(Ordering::SeqCst) as u32)
    }

    async fn cancel_invocation(
        &self,
        ctx: Context<'_>,
        invocation_id: String,
    ) -> Result<(), TerminalError> {
        ctx.invocation_handle(invocation_id).cancel().await?;
        Ok(())
    }
}
