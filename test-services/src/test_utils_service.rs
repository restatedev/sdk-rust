use crate::awakeable_holder;
use crate::list_object::ListObjectClient;
use futures::future::BoxFuture;
use futures::FutureExt;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateAwakeableAndAwaitItRequest {
    awakeable_key: String,
    await_timeout: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all_fields = "camelCase")]
pub(crate) enum CreateAwakeableAndAwaitItResponse {
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "result")]
    Result { value: String },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InterpretRequest {
    list_name: String,
    commands: Vec<InterpretCommand>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all_fields = "camelCase")]
pub(crate) enum InterpretCommand {
    #[serde(rename = "createAwakeableAndAwaitIt")]
    CreateAwakeableAndAwaitIt { awakeable_key: String },
    #[serde(rename = "getEnvVariable")]
    GetEnvVariable { env_name: String },
}

#[restate_sdk::service]
#[name = "TestUtilsService"]
pub(crate) trait TestUtilsService {
    #[name = "echo"]
    async fn echo(input: String) -> HandlerResult<String>;
    #[name = "uppercaseEcho"]
    async fn uppercase_echo(input: String) -> HandlerResult<String>;
    #[name = "echoHeaders"]
    async fn echo_headers() -> HandlerResult<Json<HashMap<String, String>>>;
    #[name = "createAwakeableAndAwaitIt"]
    async fn create_awakeable_and_await_it(
        req: Json<CreateAwakeableAndAwaitItRequest>,
    ) -> HandlerResult<Json<CreateAwakeableAndAwaitItResponse>>;
    #[name = "sleepConcurrently"]
    async fn sleep_concurrently(millis_durations: Json<Vec<u64>>) -> HandlerResult<()>;
    #[name = "countExecutedSideEffects"]
    async fn count_executed_side_effects(increments: u32) -> HandlerResult<u32>;
    #[name = "getEnvVariable"]
    async fn get_env_variable(env: String) -> HandlerResult<String>;
    #[name = "cancelInvocation"]
    async fn cancel_invocation(invocation_id: String) -> Result<(), TerminalError>;
    #[name = "interpretCommands"]
    async fn interpret_commands(req: Json<InterpretRequest>) -> HandlerResult<()>;
}

pub(crate) struct TestUtilsServiceImpl;

impl TestUtilsService for TestUtilsServiceImpl {
    async fn echo(&self, _: Context<'_>, input: String) -> HandlerResult<String> {
        Ok(input)
    }

    async fn uppercase_echo(&self, _: Context<'_>, input: String) -> HandlerResult<String> {
        Ok(input.to_ascii_uppercase())
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

    async fn create_awakeable_and_await_it(
        &self,
        context: Context<'_>,
        Json(req): Json<CreateAwakeableAndAwaitItRequest>,
    ) -> HandlerResult<Json<CreateAwakeableAndAwaitItResponse>> {
        if req.await_timeout.is_some() {
            unimplemented!("await timeout is not yet implemented");
        }

        let (awk_id, awakeable) = context.awakeable::<String>();

        context
            .object_client::<awakeable_holder::AwakeableHolderClient>(req.awakeable_key)
            .hold(awk_id)
            .call()
            .await?;
        let value = awakeable.await?;

        Ok(CreateAwakeableAndAwaitItResponse::Result { value }.into())
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

    async fn get_env_variable(&self, _: Context<'_>, env: String) -> HandlerResult<String> {
        Ok(std::env::var(env).ok().unwrap_or_default())
    }

    async fn cancel_invocation(
        &self,
        ctx: Context<'_>,
        invocation_id: String,
    ) -> Result<(), TerminalError> {
        ctx.invocation_handle(invocation_id).cancel().await?;
        Ok(())
    }

    async fn interpret_commands(
        &self,
        context: Context<'_>,
        Json(req): Json<InterpretRequest>,
    ) -> HandlerResult<()> {
        let list_client = context.object_client::<ListObjectClient>(req.list_name);

        for cmd in req.commands {
            match cmd {
                InterpretCommand::CreateAwakeableAndAwaitIt { awakeable_key } => {
                    let (awk_id, awakeable) = context.awakeable::<String>();
                    context
                        .object_client::<awakeable_holder::AwakeableHolderClient>(awakeable_key)
                        .hold(awk_id)
                        .call()
                        .await?;
                    let value = awakeable.await?;
                    list_client.append(value).send();
                }
                InterpretCommand::GetEnvVariable { env_name } => {
                    list_client
                        .append(std::env::var(env_name).ok().unwrap_or_default())
                        .send();
                }
            }
        }

        Ok(())
    }
}
