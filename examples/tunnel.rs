use restate_sdk::prelude::*;

struct Greeter;

#[service]
impl Greeter {
    #[handler]
    async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
        Ok(format!("Greetings {name}"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Install a subscriber so the tunnel's startup line (including the
    // deployment URL to register) is printed.
    tracing_subscriber::fmt::init();

    let endpoint = Endpoint::builder().bind(Greeter).build();

    // The Restate operator supplies the tunnel configuration through the
    // RESTATE_INPROC_* environment variables and its projected token Secret.
    Tunnel::new(endpoint).run().await?;

    Ok(())
}
