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

#[restate_sdk::object]
#[name = "Counter"]
pub(crate) trait Counter {
    #[name = "add"]
    async fn add(val: u64) -> HandlerResult<Json<CounterUpdateResponse>>;
    #[name = "addThenFail"]
    async fn add_then_fail(val: u64) -> HandlerResult<()>;
    #[shared]
    #[name = "get"]
    async fn get() -> HandlerResult<u64>;
    #[name = "reset"]
    async fn reset() -> HandlerResult<()>;
}

pub(crate) struct CounterImpl;

const COUNT: &str = "counter";

impl Counter for CounterImpl {
    async fn get(&self, ctx: SharedObjectContext<'_>) -> HandlerResult<u64> {
        Ok(ctx.get::<u64>(COUNT).await?.unwrap_or(0))
    }

    async fn add(
        &self,
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

    async fn reset(&self, ctx: ObjectContext<'_>) -> HandlerResult<()> {
        ctx.clear(COUNT);
        Ok(())
    }

    async fn add_then_fail(&self, ctx: ObjectContext<'_>, val: u64) -> HandlerResult<()> {
        let current = ctx.get::<u64>(COUNT).await?.unwrap_or(0);
        let new = current + val;
        ctx.set(COUNT, new);

        info!("Old count {}, new count {}", current, new);

        Err(TerminalError::new(ctx.key()).into())
    }
}
