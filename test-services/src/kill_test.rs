use crate::awakeable_holder;
use restate_sdk::prelude::*;

#[restate_sdk::service]
#[name = "KillTestRunner"]
pub(crate) trait KillTestRunner {
    #[name = "startCallTree"]
    async fn start_call_tree() -> HandlerResult<()>;
}

pub(crate) struct KillTestRunnerImpl;

impl KillTestRunner for KillTestRunnerImpl {
    async fn start_call_tree(&self, context: Context<'_>) -> HandlerResult<()> {
        context
            .object_client::<KillTestSingletonClient>("")
            .recursive_call()
            .call()
            .await?;
        Ok(())
    }
}

#[restate_sdk::object]
#[name = "KillTestSingleton"]
pub(crate) trait KillTestSingleton {
    #[name = "recursiveCall"]
    async fn recursive_call() -> HandlerResult<()>;
    #[name = "isUnlocked"]
    async fn is_unlocked() -> HandlerResult<()>;
}

pub(crate) struct KillTestSingletonImpl;

impl KillTestSingleton for KillTestSingletonImpl {
    async fn recursive_call(&self, context: ObjectContext<'_>) -> HandlerResult<()> {
        let awakeable_holder_client =
            context.object_client::<awakeable_holder::AwakeableHolderClient>("kill");

        let (awk_id, awakeable) = context.awakeable::<()>();
        awakeable_holder_client.hold(awk_id).send();
        awakeable.await?;

        context
            .object_client::<KillTestSingletonClient>("")
            .recursive_call()
            .call()
            .await?;

        Ok(())
    }

    async fn is_unlocked(&self, _: ObjectContext<'_>) -> HandlerResult<()> {
        // no-op
        Ok(())
    }
}
