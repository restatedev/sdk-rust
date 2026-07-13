use rand::Rng;
use restate_sdk::prelude::*;

#[derive(Debug, thiserror::Error)]
#[error("I'm very bad, retry me")]
struct MyError;

#[restate_sdk::handler(name = "doRun")]
async fn do_run(ctx: Context<'_>) -> Result<(), TerminalError> {
    ctx.run::<_, _, ()>(|| async move {
        if rand::rng().next_u32().is_multiple_of(4) {
            Err(TerminalError::new("Failed!!!"))?
        }

        Err(MyError)?
    })
    .await?;

    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let failure_example = define_service("FailureExample").handler(do_run).build();
    HttpServer::new(Endpoint::builder().bind(failure_example).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
