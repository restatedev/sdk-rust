use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CounterUpdateResponse {
    old_value: u64,
    new_value: u64,
}

const COUNT: &str = "counter";

#[restate_sdk::handler]
pub(crate) async fn get(ctx: SharedObjectContext<'_>) -> HandlerResult<u64> {
    Ok(ctx.get::<u64>(COUNT).await?.unwrap_or(0))
}

#[restate_sdk::handler]
pub(crate) async fn add(
    ctx: ObjectContext<'_>,
    val: u64,
) -> HandlerResult<Json<CounterUpdateResponse>> {
    let current = ctx.get::<u64>(COUNT).await?.unwrap_or(0);
    let new = current + val;
    ctx.set(COUNT, new);

    info!("Old count {}, new count {}", current, new);

    Ok(CounterUpdateResponse {
        old_value: current,
        new_value: new,
    }
    .into())
}

#[restate_sdk::handler]
pub(crate) async fn reset(ctx: ObjectContext<'_>) -> HandlerResult<()> {
    ctx.clear(COUNT);
    Ok(())
}

#[restate_sdk::handler(name = "addThenFail")]
pub(crate) async fn add_then_fail(ctx: ObjectContext<'_>, val: u64) -> HandlerResult<()> {
    let current = ctx.get::<u64>(COUNT).await?.unwrap_or(0);
    let new = current + val;
    ctx.set(COUNT, new);

    info!("Old count {}, new count {}", current, new);

    Err(TerminalError::new(ctx.key()).into())
}

// Defines the `Counter` service type + `CounterClient` (used by the NonDeterministic service).
object!(Counter: { add, add_then_fail, get, reset });
