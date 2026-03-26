use restate_sdk::prelude::*;
use std::time::Duration;

/// This example shows how to fan out multiple calls and process results as they complete,
/// using [`DurableFuturesUnordered`] for dynamic concurrent operations.
///
/// The `fan_out` handler spawns multiple sleep timers and collects results as they complete,
/// similar to how you'd use `FuturesUnordered` in regular async Rust.
///
/// To try it:
///
/// ```shell
/// $ curl -v http://localhost:8080/FanOut/fan_out
/// ```
#[restate_sdk::service]
trait FanOut {
    async fn fan_out() -> Result<String, TerminalError>;
}

struct FanOutImpl;

impl FanOut for FanOutImpl {
    async fn fan_out(&self, ctx: Context<'_>) -> Result<String, TerminalError> {
        let labels = ["fast", "medium", "slow"];

        // Fan out: create a dynamic number of durable futures
        let mut futures = DurableFuturesUnordered::new();
        futures.push(ctx.sleep(Duration::from_secs(1)));
        futures.push(ctx.sleep(Duration::from_secs(2)));
        futures.push(ctx.sleep(Duration::from_secs(3)));

        // Process results as they complete — index tells you which future finished
        let mut completion_order = Vec::new();
        while let Some((index, result)) = futures.next().await? {
            result?;
            completion_order.push(labels[index]);
        }

        Ok(format!("Completed in order: {completion_order:?}"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(FanOutImpl.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
