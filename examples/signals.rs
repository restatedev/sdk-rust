use restate_sdk::prelude::*;

struct SignalExample;

#[restate_sdk::service]
impl SignalExample {
    /// Await a signal named "approval" on the current invocation.
    ///
    /// Share this invocation's id (`ctx.invocation_id()`) with whoever should approve; they can
    /// then complete the signal via `approve`/`reject` below.
    #[handler]
    async fn await_approval(&self, ctx: Context<'_>) -> Result<String, HandlerError> {
        println!(
            "Waiting for approval. Approve with invocation id: {}",
            ctx.invocation_id()
        );

        // Suspends until another invocation resolves/rejects the "approval" signal on this
        // invocation. Works in `select!` too, since it is a durable future.
        let decision = ctx.signal::<String>("approval").await?;

        Ok(format!("Approved with: {decision}"))
    }

    /// Resolve the "approval" signal on the target invocation.
    #[handler]
    async fn approve(&self, ctx: Context<'_>, invocation_id: String) -> Result<(), HandlerError> {
        ctx.invocation_handle(invocation_id)
            .signal("approval")
            .resolve("looks good".to_string());
        Ok(())
    }

    /// Reject the "approval" signal on the target invocation. The awaiting handler observes a
    /// terminal error.
    #[handler]
    async fn reject(&self, ctx: Context<'_>, invocation_id: String) -> Result<(), HandlerError> {
        ctx.invocation_handle(invocation_id)
            .signal("approval")
            .reject(TerminalError::new("rejected"));
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(SignalExample).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
