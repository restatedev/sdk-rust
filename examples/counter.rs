use restate_sdk::prelude::*;

#[restate_sdk::object]
trait Counter {
    #[shared]
    async fn get() -> HandlerResult<u64>;
    async fn add(val: u64) -> HandlerResult<u64>;
    async fn increment() -> HandlerResult<u64>;
    async fn reset() -> HandlerResult<()>;
}

struct CounterImpl;

const COUNT: &str = "count";

impl Counter for CounterImpl {
    async fn get(&self, ctx: SharedObjectContext<'_>) -> HandlerResult<u64> {
        Ok(ctx.get::<u64>(COUNT).await?.unwrap_or(0))
    }

    async fn add(&self, ctx: ObjectContext<'_>, val: u64) -> HandlerResult<u64> {
        let current = ctx.get::<u64>(COUNT).await?.unwrap_or(0);
        let new = current + val;
        ctx.set(COUNT, new);
        Ok(new)
    }

    async fn increment(&self, ctx: ObjectContext<'_>) -> HandlerResult<u64> {
        self.add(ctx, 1).await
    }

    async fn reset(&self, ctx: ObjectContext<'_>) -> HandlerResult<()> {
        ctx.clear(COUNT);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HyperServer::new(
        Endpoint::builder()
            .with_service(CounterImpl.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
