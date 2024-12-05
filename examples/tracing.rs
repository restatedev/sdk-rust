use restate_sdk::prelude::*;
use std::convert::Infallible;
use std::time::Duration;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

#[restate_sdk::service]
trait Greeter {
    async fn greet(name: String) -> Result<String, Infallible>;
}

struct GreeterImpl;

impl Greeter for GreeterImpl {
    async fn greet(&self, ctx: Context<'_>, name: String) -> Result<String, Infallible> {
        let timeout = 60; // More than suspension timeout to trigger replay
        info!("This will be logged on replay");
        _ = ctx.service_client::<DelayerClient>().delay(1).call().await;
        info!("This will not be logged on replay");
        _ = ctx
            .service_client::<DelayerClient>()
            .delay(timeout)
            .call()
            .await;
        info!("This will be logged on processing after suspension");
        Ok(format!("Greetings {name} after {timeout} seconds"))
    }
}

#[restate_sdk::service]
trait Delayer {
    async fn delay(seconds: u64) -> Result<String, Infallible>;
}

struct DelayerImpl;

impl Delayer for DelayerImpl {
    async fn delay(&self, ctx: Context<'_>, seconds: u64) -> Result<String, Infallible> {
        _ = ctx.sleep(Duration::from_secs(seconds)).await;
        info!("Delayed for {seconds} seconds");
        Ok(format!("Delayed {seconds}"))
    }
}

#[tokio::main]
async fn main() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "restate_sdk=info".into());
    let replay_filter = restate_sdk::filter::ReplayAwareFilter;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(env_filter)
                .with_filter(replay_filter),
        )
        .init();
    HttpServer::new(
        Endpoint::builder()
            .bind(GreeterImpl.serve())
            .bind(DelayerImpl.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
