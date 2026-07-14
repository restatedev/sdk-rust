use anyhow::anyhow;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FailureToPropagate {
    error_message: String,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

/// Process-wide counters, injected as an extension (registered on the endpoint in `main`).
#[derive(Clone, Default)]
pub(crate) struct FailingState {
    eventual_success_calls: Arc<AtomicI32>,
    eventual_success_side_effects: Arc<AtomicI32>,
    eventual_failure_side_effects: Arc<AtomicI32>,
}

#[restate_sdk::handler(name = "terminallyFailingCall")]
pub(crate) async fn terminally_failing_call(
    _ctx: ObjectContext<'_>,
    Json(failure): Json<FailureToPropagate>,
) -> HandlerResult<()> {
    Err(TerminalError::new(failure.error_message).into())
}

#[restate_sdk::handler(name = "callTerminallyFailingCall")]
pub(crate) async fn call_terminally_failing_call(
    mut ctx: ObjectContext<'_>,
    Json(failure): Json<FailureToPropagate>,
) -> HandlerResult<String> {
    let uuid = ctx.rand_uuid().to_string();
    ctx.object_client::<FailingClient>(uuid)
        .terminally_failing_call(Json(failure))
        .call()
        .await?;

    unreachable!("This should be unreachable")
}

#[restate_sdk::handler(name = "failingCallWithEventualSuccess")]
pub(crate) async fn failing_call_with_eventual_success(
    ctx: ObjectContext<'_>,
) -> HandlerResult<i32> {
    let calls = &ctx.extension::<FailingState>().eventual_success_calls;
    let current_attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;

    if current_attempt >= 4 {
        calls.store(0, Ordering::SeqCst);
        Ok(current_attempt)
    } else {
        Err(anyhow!("Failed at attempt ${current_attempt}").into())
    }
}

#[restate_sdk::handler(name = "terminallyFailingSideEffect")]
pub(crate) async fn terminally_failing_side_effect(
    ctx: ObjectContext<'_>,
    Json(failure): Json<FailureToPropagate>,
) -> HandlerResult<()> {
    ctx.run::<_, _, ()>(|| async move { Err(TerminalError::new(failure.error_message))? })
        .await?;

    unreachable!("This should be unreachable")
}

#[restate_sdk::handler(name = "sideEffectSucceedsAfterGivenAttempts")]
pub(crate) async fn side_effect_succeeds_after_given_attempts(
    ctx: ObjectContext<'_>,
    minimum_attempts: i32,
) -> HandlerResult<i32> {
    let cloned_counter = Arc::clone(
        &ctx.extension::<FailingState>()
            .eventual_success_side_effects,
    );
    let success_attempt = ctx
        .run(|| async move {
            let current_attempt = cloned_counter.fetch_add(1, Ordering::SeqCst) + 1;

            if current_attempt >= minimum_attempts {
                cloned_counter.store(0, Ordering::SeqCst);
                Ok(current_attempt)
            } else {
                Err(anyhow!("Failed at attempt {current_attempt}"))?
            }
        })
        .retry_policy(
            RunRetryPolicy::new()
                .initial_delay(Duration::from_millis(10))
                .exponentiation_factor(1.0),
        )
        .name("failing_side_effect")
        .await?;

    Ok(success_attempt)
}

#[restate_sdk::handler(name = "sideEffectFailsAfterGivenAttempts")]
pub(crate) async fn side_effect_fails_after_given_attempts(
    ctx: ObjectContext<'_>,
    retry_policy_max_retry_count: i32,
) -> HandlerResult<i32> {
    let counter = Arc::clone(
        &ctx.extension::<FailingState>()
            .eventual_failure_side_effects,
    );
    let cloned_counter = Arc::clone(&counter);
    if ctx
        .run(|| async move {
            let current_attempt = cloned_counter.fetch_add(1, Ordering::SeqCst) + 1;
            Err::<(), _>(anyhow!("Failed at attempt {current_attempt}").into())
        })
        .retry_policy(
            RunRetryPolicy::new()
                .initial_delay(Duration::from_millis(10))
                .exponentiation_factor(1.0)
                .max_attempts(retry_policy_max_retry_count as u32),
        )
        .await
        .is_err()
    {
        Ok(counter.load(Ordering::SeqCst))
    } else {
        Err(TerminalError::new("Expecting the side effect to fail!"))?
    }
}

object!(Failing: {
    terminally_failing_call,
    call_terminally_failing_call,
    failing_call_with_eventual_success,
    terminally_failing_side_effect,
    side_effect_succeeds_after_given_attempts,
    side_effect_fails_after_given_attempts,
});
