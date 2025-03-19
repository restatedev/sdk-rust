use crate::awakeable_holder;
use restate_sdk::prelude::*;

pub(crate) struct KillTestRunner;

#[restate_sdk::object(vis = "pub(crate)", name = "KillTestRunner")]
impl KillTestRunner {
    #[handler(name = "startCallTree")]
    async fn start_call_tree(&self, context: ObjectContext<'_>) -> HandlerResult<()> {
        context
            .object_client::<KillTestSingletonClient>(context.key())
            .recursive_call()
            .call()
            .await?;
        Ok(())
    }
}

pub(crate) struct KillTestSingleton;

#[restate_sdk::object(vis = "pub(crate)", name = "KillTestSingleton")]
impl KillTestSingleton {
    #[handler(name = "recursiveCall")]
    async fn recursive_call(&self, context: ObjectContext<'_>) -> HandlerResult<()> {
        let awakeable_holder_client =
            context.object_client::<awakeable_holder::AwakeableHolderClient>(context.key());

        let (awk_id, awakeable) = context.awakeable::<()>();
        awakeable_holder_client.hold(awk_id).send();
        awakeable.await?;

        context
            .object_client::<KillTestSingletonClient>(context.key())
            .recursive_call()
            .call()
            .await?;

        Ok(())
    }

    #[handler(name = "isUnlocked")]
    async fn is_unlocked(&self, _ctx: ObjectContext<'_>) -> HandlerResult<()> {
        // no-op
        Ok(())
    }
}
