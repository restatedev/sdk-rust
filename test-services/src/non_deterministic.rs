use crate::counter::CounterClient;
use restate_sdk::prelude::*;
use restate_sdk::service::ServiceDefinition;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Per-service invocation counter, injected as ambient state.
#[derive(Clone, Default)]
pub(crate) struct NonDetState(Arc<Mutex<HashMap<String, i32>>>);

const STATE_A: &str = "a";
const STATE_B: &str = "b";

async fn do_left_action(state: &NonDetState, ctx: &ObjectContext<'_>) -> bool {
    let mut counts = state.0.lock().await;
    *(counts
        .entry(ctx.key().to_owned())
        .and_modify(|i| *i += 1)
        .or_default())
        % 2
        == 1
}

async fn sleep_then_increment_counter(ctx: &ObjectContext<'_>) -> HandlerResult<()> {
    ctx.sleep(Duration::from_millis(100)).await?;
    ctx.object_client::<CounterClient>(ctx.key()).add(1).send();
    Ok(())
}

#[restate_sdk::handler(name = "eitherSleepOrCall")]
pub(crate) async fn either_sleep_or_call(ctx: ObjectContext<'_>) -> HandlerResult<()> {
    if do_left_action(ctx.state::<NonDetState>(), &ctx).await {
        ctx.sleep(Duration::from_millis(100)).await?;
    } else {
        ctx.object_client::<CounterClient>("abc")
            .get()
            .call()
            .await?;
    }
    sleep_then_increment_counter(&ctx).await
}

#[restate_sdk::handler(name = "callDifferentMethod")]
pub(crate) async fn call_different_method(ctx: ObjectContext<'_>) -> HandlerResult<()> {
    if do_left_action(ctx.state::<NonDetState>(), &ctx).await {
        ctx.object_client::<CounterClient>("abc")
            .get()
            .call()
            .await?;
    } else {
        ctx.object_client::<CounterClient>("abc")
            .reset()
            .call()
            .await?;
    }
    sleep_then_increment_counter(&ctx).await
}

#[restate_sdk::handler(name = "backgroundInvokeWithDifferentTargets")]
pub(crate) async fn background_invoke_with_different_targets(
    ctx: ObjectContext<'_>,
) -> HandlerResult<()> {
    if do_left_action(ctx.state::<NonDetState>(), &ctx).await {
        ctx.object_client::<CounterClient>("abc").get().send();
    } else {
        ctx.object_client::<CounterClient>("abc").reset().send();
    }
    sleep_then_increment_counter(&ctx).await
}

#[restate_sdk::handler(name = "setDifferentKey")]
pub(crate) async fn set_different_key(ctx: ObjectContext<'_>) -> HandlerResult<()> {
    if do_left_action(ctx.state::<NonDetState>(), &ctx).await {
        ctx.set(STATE_A, "my-state".to_owned());
    } else {
        ctx.set(STATE_B, "my-state".to_owned());
    }
    sleep_then_increment_counter(&ctx).await
}

pub(crate) fn definition() -> ServiceDefinition {
    define_object("NonDeterministic")
        .state(NonDetState::default())
        .handler(either_sleep_or_call)
        .handler(call_different_method)
        .handler(background_invoke_with_different_targets)
        .handler(set_different_key)
        .build()
}
