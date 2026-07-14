use crate::awakeable_holder;
use restate_sdk::prelude::*;

#[restate_sdk::handler(name = "recursiveCall")]
pub(crate) async fn recursive_call(context: ObjectContext<'_>) -> HandlerResult<()> {
    let awakeable_holder_client =
        context.object_client::<awakeable_holder::AwakeableHolderClient>(context.key());

    let (awk_id, awakeable) = context.awakeable::<()>();
    awakeable_holder_client.hold(awk_id).send();
    awakeable.await?;

    context
        .object_client::<KillTestSingletonClient>(context.key())
        .recursive_call(())
        .call()
        .await?;

    Ok(())
}

#[restate_sdk::handler(name = "isUnlocked")]
pub(crate) async fn is_unlocked(_context: ObjectContext<'_>) -> HandlerResult<()> {
    // no-op
    Ok(())
}

#[restate_sdk::handler(name = "startCallTree")]
pub(crate) async fn start_call_tree(context: ObjectContext<'_>) -> HandlerResult<()> {
    context
        .object_client::<KillTestSingletonClient>(context.key())
        .recursive_call(())
        .call()
        .await?;
    Ok(())
}

object!(KillTestSingleton: { recursive_call, is_unlocked });
object!(KillTestRunner: { start_call_tree });
