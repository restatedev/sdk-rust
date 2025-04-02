use restate_sdk::prelude::*;

const COUNT: &str = "count";

struct Counter;

#[restate_sdk::object]
impl Counter {
    #[handler(shared)]
    async fn get(&self, ctx: SharedObjectContext<'_>) -> Result<u64, TerminalError> {
        Ok(ctx.get::<u64>(COUNT).await?.unwrap_or(0))
    }

    #[handler]
    async fn add(&self, ctx: ObjectContext<'_>, val: u64) -> Result<u64, TerminalError> {
        let current = ctx.get::<u64>(COUNT).await?.unwrap_or(0);
        let new = current + val;
        ctx.set(COUNT, new);
        Ok(new)
    }

    #[handler]
    async fn increment(&self, ctx: ObjectContext<'_>) -> Result<u64, TerminalError> {
        self.add(ctx, 1).await
    }

    #[handler]
    async fn reset(&self, ctx: ObjectContext<'_>) -> Result<(), TerminalError> {
        ctx.clear(COUNT);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(Counter.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
