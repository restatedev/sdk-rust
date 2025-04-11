use crate::awakeable_holder;
use anyhow::anyhow;
use restate_sdk::prelude::*;
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

#[restate_sdk::object]
#[name = "CancelTestRunner"]
pub(crate) trait CancelTestRunner {
    #[name = "startTest"]
    async fn start_test(op: Json<BlockingOperation>) -> HandlerResult<()>;
    #[name = "verifyTest"]
    async fn verify_test() -> HandlerResult<bool>;
}

pub(crate) struct CancelTestRunnerImpl;

const CANCELED: &str = "canceled";

impl CancelTestRunner for CancelTestRunnerImpl {
    async fn start_test(
        &self,
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

    async fn verify_test(&self, context: ObjectContext<'_>) -> HandlerResult<bool> {
        Ok(context.get::<bool>(CANCELED).await?.unwrap_or(false))
    }
}

#[restate_sdk::object]
#[name = "CancelTestBlockingService"]
pub(crate) trait CancelTestBlockingService {
    #[name = "block"]
    async fn block(op: Json<BlockingOperation>) -> HandlerResult<()>;
    #[name = "isUnlocked"]
    async fn is_unlocked() -> HandlerResult<()>;
}

pub(crate) struct CancelTestBlockingServiceImpl;

impl CancelTestBlockingService for CancelTestBlockingServiceImpl {
    async fn block(
        &self,
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

    async fn is_unlocked(&self, _: ObjectContext<'_>) -> HandlerResult<()> {
        // no-op
        Ok(())
    }
}
