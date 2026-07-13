use crate::awakeable_holder;
use anyhow::anyhow;
use restate_sdk::prelude::*;
use restate_sdk::service::ServiceDefinition;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BlockingOperation {
    Call,
    Sleep,
    Awakeable,
}

const CANCELED: &str = "canceled";

// ---- CancelTestRunner ----

#[restate_sdk::handler(name = "startTest")]
pub(crate) async fn start_test(
    context: ObjectContext<'_>,
    op: Json<BlockingOperation>,
) -> HandlerResult<()> {
    let this = context.object_client::<CancelTestBlockingServiceClient>(context.key());

    match this.block(op).call().await {
        Ok(_) => Err(anyhow!("Block succeeded, this is unexpected").into()),
        Err(e) if e.code() == 409 => {
            context.set(CANCELED, true);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[restate_sdk::handler(name = "verifyTest")]
pub(crate) async fn verify_test(context: ObjectContext<'_>) -> HandlerResult<bool> {
    Ok(context.get::<bool>(CANCELED).await?.unwrap_or(false))
}

// ---- CancelTestBlockingService ----

restate_sdk::interface! {
    object CancelTestBlockingService {
        block(Json<BlockingOperation>) -> ();
        #[name = "isUnlocked"]
        is_unlocked() -> ();
    }
}

#[restate_sdk::handler]
pub(crate) async fn block(
    context: ObjectContext<'_>,
    op: Json<BlockingOperation>,
) -> HandlerResult<()> {
    let this = context.object_client::<CancelTestBlockingServiceClient>(context.key());
    let awakeable_holder_client =
        context.object_client::<awakeable_holder::AwakeableHolderClient>(context.key());

    let (awk_id, awakeable) = context.awakeable::<String>();
    awakeable_holder_client.hold(awk_id).call().await?;
    awakeable.await?;

    match &op.0 {
        BlockingOperation::Call => {
            this.block(op).call().await?;
        }
        BlockingOperation::Sleep => {
            context
                .sleep(Duration::from_secs(60 * 60 * 24 * 1024))
                .await?;
        }
        BlockingOperation::Awakeable => {
            let (_, uncompletable) = context.awakeable::<String>();
            uncompletable.await?;
        }
    }

    Ok(())
}

#[restate_sdk::handler]
pub(crate) async fn is_unlocked(_context: ObjectContext<'_>) -> HandlerResult<()> {
    // no-op
    Ok(())
}

pub(crate) fn runner_definition() -> ServiceDefinition {
    object!("CancelTestRunner", start_test, verify_test)
}

pub(crate) fn blocking_definition() -> ServiceDefinition {
    CancelTestBlockingService::from_handlers(CancelTestBlockingServiceHandlers { block, is_unlocked })
}
