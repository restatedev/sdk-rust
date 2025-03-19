use crate::awakeable_holder;
use anyhow::anyhow;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BlockingOperation {
    Call,
    Sleep,
    Awakeable,
}

pub(crate) struct CancelTestRunner;

const CANCELED: &str = "canceled";

#[restate_sdk::object(vis = "pub(crate)", name = "CancelTestRunner")]
impl CancelTestRunner {
    #[handler(name = "startTest")]
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

    #[handler(name = "verifyTest")]
    async fn verify_test(&self, context: ObjectContext<'_>) -> HandlerResult<bool> {
        Ok(context.get::<bool>(CANCELED).await?.unwrap_or(false))
    }
}

pub(crate) struct CancelTestBlockingService;

#[restate_sdk::object(vis = "pub(crate)", name = "CancelTestBlockingService")]
impl CancelTestBlockingService {
    #[handler(name = "block")]
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

    #[handler(name = "isUnlocked")]
    async fn is_unlocked(&self, _ctx: ObjectContext<'_>) -> HandlerResult<()> {
        // no-op
        Ok(())
    }
}
