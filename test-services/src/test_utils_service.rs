use futures::FutureExt;
use futures::future::BoxFuture;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolveSignalRequest {
    invocation_id: String,
    signal_name: String,
    value: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RejectSignalRequest {
    invocation_id: String,
    signal_name: String,
    reason: String,
}

pub(crate) struct TestUtilsService;

#[service(name = "TestUtilsService")]
impl TestUtilsService {
    #[handler(name = "echo")]
    async fn echo(&self, _: Context<'_>, input: String) -> HandlerResult<String> {
        Ok(input)
    }

    #[handler(name = "uppercaseEcho")]
    async fn uppercase_echo(&self, _: Context<'_>, input: String) -> HandlerResult<String> {
        Ok(input.to_ascii_uppercase())
    }

    #[handler(name = "rawEcho")]
    async fn raw_echo(&self, _: Context<'_>, input: bytes::Bytes) -> Result<Vec<u8>, Infallible> {
        Ok(input.to_vec())
    }

    #[handler(name = "echoHeaders")]
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

    #[handler(name = "sleepConcurrently")]
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

    #[handler(name = "countExecutedSideEffects")]
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

    #[handler(name = "cancelInvocation")]
    async fn cancel_invocation(
        &self,
        ctx: Context<'_>,
        invocation_id: String,
    ) -> Result<(), TerminalError> {
        ctx.invocation_handle(invocation_id).cancel();
        Ok(())
    }

    #[handler(name = "resolveSignal")]
    async fn resolve_signal(
        &self,
        ctx: Context<'_>,
        req: Json<ResolveSignalRequest>,
    ) -> Result<(), HandlerError> {
        let ResolveSignalRequest {
            invocation_id,
            signal_name,
            value,
        } = req.into_inner();
        ctx.invocation_handle(invocation_id)
            .signal(signal_name)
            .resolve(value);
        Ok(())
    }

    #[handler(name = "rejectSignal")]
    async fn reject_signal(
        &self,
        ctx: Context<'_>,
        req: Json<RejectSignalRequest>,
    ) -> Result<(), HandlerError> {
        let RejectSignalRequest {
            invocation_id,
            signal_name,
            reason,
        } = req.into_inner();
        ctx.invocation_handle(invocation_id)
            .signal(signal_name)
            .reject(TerminalError::new(reason));
        Ok(())
    }
}
