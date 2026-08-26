//! Calls the Greeter example through Restate's typed ingress client.
//!
//! Start Restate 1.7 or newer, run and register the `greeter` example, then run:
//!
//! ```text
//! cargo run --example ingress_client --features reqwest-client
//! ```
//!
//! Pass an ingress URL and name as optional arguments after `--`.
#![allow(dead_code)]

use restate_sdk::{ingress::ReqwestClient, prelude::*};

// Define a service, this will generate the GreeterIngressClient
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
    let mut args = std::env::args().skip(1);
    let ingress_url = args
        .next()
        .unwrap_or_else(|| "http://localhost:8080".to_owned());
    let name = args.next().unwrap_or_else(|| "Ada".to_owned());

    let client = ReqwestClient::connect(ingress_url.parse()?)?;
    let greeter = GreeterIngressClient::from_client(client);

    // Wait for completion and decode the handler's typed output.
    let response = greeter.greet(name.clone()).call().await?;
    let invocation = response.invocation_handle();
    let greeting = response.into_body()?;
    println!("{greeting}");
    println!("completed invocation: {}", invocation.invocation_id());

    // Or enqueue an invocation without waiting for its output.
    let response = greeter.greet(format!("{name} (async)")).send().await?;
    println!(
        "queued invocation: {} ({:?})",
        response.invocation_handle().invocation_id(),
        response.send_status()
    );

    Ok(())
}
