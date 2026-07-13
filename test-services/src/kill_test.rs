use crate::awakeable_holder;
use restate_sdk::prelude::*;
use restate_sdk::service::ServiceDefinition;

restate_sdk::interface! {
    object KillTestSingleton {
        #[name = "recursiveCall"]
        recursive_call() -> ();
        #[name = "isUnlocked"]
        is_unlocked() -> ();
    }
}

#[restate_sdk::handler]
pub(crate) async fn recursive_call(context: ObjectContext<'_>) -> HandlerResult<()> {
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

#[restate_sdk::handler]
pub(crate) async fn is_unlocked(_context: ObjectContext<'_>) -> HandlerResult<()> {
    // no-op
    Ok(())
}

#[restate_sdk::handler(name = "startCallTree")]
pub(crate) async fn start_call_tree(context: ObjectContext<'_>) -> HandlerResult<()> {
    context
        .object_client::<KillTestSingletonClient>(context.key())
        .recursive_call()
        .call()
        .await?;
    Ok(())
}

pub(crate) fn singleton_definition() -> ServiceDefinition {
    KillTestSingleton::from_handlers(KillTestSingletonHandlers {
        recursive_call,
        is_unlocked,
    })
}

pub(crate) fn runner_definition() -> ServiceDefinition {
    define_object("KillTestRunner")
        .handler(start_call_tree)
        .build()
}
