use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CounterUpdateResponse {
    old_value: u64,
    new_value: u64,
}

pub(crate) struct Counter;

const COUNT: &str = "counter";

#[restate_sdk::object(vis = "pub(crate)", name = "Counter")]
impl Counter {
    #[handler(shared, name = "get")]
    async fn get(&self, ctx: SharedObjectContext<'_>) -> HandlerResult<u64> {
        Ok(ctx.get::<u64>(COUNT).await?.unwrap_or(0))
    }

    #[handler(name = "add")]
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

    #[handler(name = "reset")]
    async fn reset(&self, ctx: ObjectContext<'_>) -> HandlerResult<()> {
        ctx.clear(COUNT);
        Ok(())
    }

    #[handler(name = "addThenFail")]
    async fn add_then_fail(&self, ctx: ObjectContext<'_>, val: u64) -> HandlerResult<()> {
        let current = ctx.get::<u64>(COUNT).await?.unwrap_or(0);
        let new = current + val;
        ctx.set(COUNT, new);

        info!("Old count {}, new count {}", current, new);

        Err(TerminalError::new(ctx.key()).into())
    }
}
