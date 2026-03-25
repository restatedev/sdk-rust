use restate_sdk::prelude::*;
use std::time::Duration;

/// This example shows how to fan out multiple calls and process results as they complete,
/// using [`select_any`] for dynamic concurrent operations.
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
        // Fan out: create a dynamic number of durable futures
        let mut futures = vec![
            ctx.sleep(Duration::from_secs(3)),
            ctx.sleep(Duration::from_secs(1)),
            ctx.sleep(Duration::from_secs(2)),
        ];

        // Process all results as they complete (like FuturesUnordered)
        let mut completion_order = Vec::new();
        while !futures.is_empty() {
            let (index, result) = select_any(&mut futures).await?;
            result?;
            completion_order.push(index);
        }

        Ok(format!("Completed in order: {:?}", completion_order))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(FanOutImpl.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
