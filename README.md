[![Documentation](https://img.shields.io/docsrs/restate-sdk)](https://docs.rs/restate-sdk)
[![crates.io](https://img.shields.io/crates/v/restate_sdk.svg)](https://crates.io/crates/restate-sdk/)
[![Examples](https://img.shields.io/badge/view-examples-blue)](https://github.com/restatedev/examples)
[![Discord](https://img.shields.io/discord/1128210118216007792?logo=discord)](https://discord.gg/skW3AZ6uGd)
[![Twitter](https://img.shields.io/twitter/follow/restatedev.svg?style=social&label=Follow)](https://twitter.com/intent/follow?screen_name=restatedev)

# Restate Rust SDK

[Restate](https://restate.dev/) is a system for easily building resilient applications using _distributed durable async/await_. This repository contains the Restate SDK for writing services using Rust.

## Community

* 🤗️ [Join our online community](https://discord.gg/skW3AZ6uGd) for help, sharing feedback and talking to the community.
* 📖 [Check out our documentation](https://docs.restate.dev) to get quickly started!
* 📣 [Follow us on Twitter](https://twitter.com/restatedev) for staying up to date.
* 🙋 [Create a GitHub issue](https://github.com/restatedev/sdk-java/issues) for requesting a new feature or reporting a problem.
* 🏠 [Visit our GitHub org](https://github.com/restatedev) for exploring other repositories.

## Using the SDK

Add Restate and Tokio as dependencies:

```toml
[dependencies]
restate-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

Then you're ready to develop your Restate service using Rust:

```rust
use restate_sdk::prelude::*;

#[restate_sdk::service]
trait Greeter {
    async fn greet(name: String) -> HandlerResult<String>;
}

struct GreeterImpl;

impl Greeter for GreeterImpl {
    async fn greet(&self, _: Context<'_>, name: String) -> HandlerResult<String> {
        Ok(format!("Greetings {name}"))
    }
}

#[tokio::main]
async fn main() {
    // To enable logging/tracing
    // tracing_subscriber::fmt::init();
    HttpServer::new(
        Endpoint::builder()
            .with_service(GreeterImpl.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
```

### Logging

The SDK uses tokio's [`tracing`](https://docs.rs/tracing/latest/tracing/) crate to generate logs.
Just configure it as usual through [`tracing_subscriber`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/) to get your logs.

### Testing

The SDK uses [Testcontainers](https://rust.testcontainers.org/) to support integration testing using a Docker-deployed restate server.
The `restate-sdk-test-env` crate provides a framework for initializing the test environment, and an integration test example in `test-env/tests/test_container.rs`.

```rust
#[tokio::test]
async fn test_container() {
    tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::INFO) // Set the maximum log level
        .init();

    let endpoint = Endpoint::builder().bind(MyServiceImpl.serve()).build();

    // simple test container intialization with default configuration
    //let test_container = TestContainer::default().start(endpoint).await.unwrap();

    // custom test container initialization with builder
    let test_container = TestContainer::builder()
        // optional passthrough logging from the resstate server testcontainer
        // prints container logs to tracing::info level
        .with_container_logging()
        .with_container(
            "docker.io/restatedev/restate".to_string(),
            "latest".to_string(),
        )
        .build()
        .start(endpoint)
        .await
        .unwrap();

    let ingress_url = test_container.ingress_url();

    // call container ingress url for /MyService/my_handler
    let response = reqwest::Client::new()
        .post(format!("{}/MyService/my_handler", ingress_url))
        .header("Accept", "application/json")
        .header("Content-Type", "*/*")
        .header("idempotency-key", "abc")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    info!(
        "/MyService/my_handler response: {:?}",
        response.text().await.unwrap()
    );
}
```

## Versions

The Rust SDK is currently in active development, and might break across releases.

The compatibility with Restate is described in the following table:

| Restate Server\sdk-rust | 0.0/0.1/0.2 | 0.3 | 0.4 |
|-------------------------|-------------|-----|-----|
| 1.0                     | ✅           | ❌   | ❌   |
| 1.1                     | ✅           | ✅   | ❌   |
| 1.2                     | ✅           | ✅   | ❌   |
| 1.3                     | ✅           | ✅   | ✅   |

## Contributing

We’re excited if you join the Restate community and start contributing!
Whether it is feature requests, bug reports, ideas & feedback or PRs, we appreciate any and all contributions.
We know that your time is precious and, therefore, deeply value any effort to contribute!

### Building the SDK locally

Prerequisites:

- [Rust](https://rustup.rs/)
- [Just](https://github.com/casey/just)

To build and test the SDK:

```shell
just verify
```

### Releasing

You need the [Rust toolchain](https://rustup.rs/). To verify:

```
just verify
```

To release we use [cargo-release](https://github.com/crate-ci/cargo-release):

```
cargo release <VERSION> --exclude test-services --workspace
```
