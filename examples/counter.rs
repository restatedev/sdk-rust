use restate_sdk::prelude::*;

const COUNT: &str = "count";

#[restate_sdk::handler]
async fn get(ctx: SharedObjectContext<'_>) -> Result<u64, TerminalError> {
    Ok(ctx.get::<u64>(COUNT).await?.unwrap_or(0))
}

#[restate_sdk::handler]
async fn add(ctx: ObjectContext<'_>, val: u64) -> Result<u64, TerminalError> {
    let current = ctx.get::<u64>(COUNT).await?.unwrap_or(0);
    let new = current + val;
    ctx.set(COUNT, new);
    Ok(new)
}

#[restate_sdk::handler]
async fn increment(ctx: ObjectContext<'_>) -> Result<u64, TerminalError> {
    // Handlers remain plain functions, so their bodies are directly reusable via `::call`.
    add::call(ctx, 1).await
}

#[restate_sdk::handler]
async fn reset(ctx: ObjectContext<'_>) -> Result<(), TerminalError> {
    ctx.clear(COUNT);
    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let counter = object!("Counter", get, add, increment, reset);
    HttpServer::new(Endpoint::builder().bind(counter).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
