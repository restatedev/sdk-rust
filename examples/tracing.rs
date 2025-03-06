use restate_sdk::prelude::*;
use std::time::Duration;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

#[restate_sdk::service]
trait Greeter {
    async fn greet(name: String) -> Result<String, HandlerError>;
}

struct GreeterImpl;

impl Greeter for GreeterImpl {
    async fn greet(&self, ctx: Context<'_>, name: String) -> Result<String, HandlerError> {
        info!("Before sleep");
        ctx.sleep(Duration::from_secs(61)).await?; // More than suspension timeout to trigger replay
        info!("After sleep");
        Ok(format!("Greetings {name}"))
    }
}

#[tokio::main]
async fn main() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,restate_sdk=debug".into());
    let replay_filter = restate_sdk::filter::ReplayAwareFilter;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(env_filter)
                .with_filter(replay_filter),
        )
        .init();
    HttpServer::new(Endpoint::builder().bind(GreeterImpl.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
