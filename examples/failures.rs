use rand::RngCore;
use restate_sdk::prelude::*;

#[derive(Debug, thiserror::Error)]
#[error("I'm very bad, retry me")]
struct MyError;

struct FailureExample;

#[restate_sdk::service]
impl FailureExample {
    #[handler(name = "doRun")]
    async fn do_run(&self, context: Context<'_>) -> Result<(), TerminalError> {
        context
            .run::<_, _, ()>(|| async move {
                if rand::thread_rng().next_u32() % 4 == 0 {
                    Err(TerminalError::new("Failed!!!"))?
                }

                Err(MyError)?
            })
            .await?;

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(FailureExample.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
