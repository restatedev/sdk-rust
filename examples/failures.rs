use restate_sdk::prelude::*;
use rand::RngCore;

#[restate_sdk::service]
trait FailureExample {
    async fn do_run() -> HandlerResult<()>;
}

struct FailureExampleImpl;

#[derive(Debug, thiserror::Error)]
#[error("I'm very bad, retry me")]
struct MyError;

impl FailureExample for FailureExampleImpl {
    async fn do_run(&self, context: Context<'_>) -> HandlerResult<()> {
        context
            .run("get_ip", || async move {
                if rand::thread_rng().next_u32() % 4 == 0 {
                    return Err(TerminalError::new("Failed!!!").into())
                }

                Err(MyError.into())
            })
            .await?;

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HyperServer::new(
        Endpoint::builder()
            .with_service(FailureExampleImpl.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
