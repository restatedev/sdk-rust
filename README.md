[![Documentation](https://img.shields.io/docsrs/restate-sdk)](https://docs.rs/restate-sdk)
[![crates.io](https://img.shields.io/crates/v/restate_sdk.svg)](https://crates.io/crates/restate-sdk/)
[![Examples](https://img.shields.io/badge/view-examples-blue)](https://github.com/restatedev/examples)
[![Discord](https://img.shields.io/discord/1128210118216007792?logo=discord)](https://discord.gg/skW3AZ6uGd)
[![Twitter](https://img.shields.io/twitter/follow/restatedev.svg?style=social&label=Follow)](https://twitter.com/intent/follow?screen_name=restatedev)

# Restate Rust SDK

[Restate](https://restate.dev/) is a system for easily building resilient applications using _distributed durable async/await_. This repository contains the Restate SDK for writing services using Rust.

## Community

- 🤗️ [Join our online community](https://discord.gg/skW3AZ6uGd) for help, sharing feedback and talking to the community.
- 📖 [Check out our documentation](https://docs.restate.dev) to get quickly started!
- 📣 [Follow us on Twitter](https://twitter.com/restatedev) for staying up to date.
- 🙋 [Create a GitHub issue](https://github.com/restatedev/sdk-java/issues) for requesting a new feature or reporting a problem.
- 🏠 [Visit our GitHub org](https://github.com/restatedev) for exploring other repositories.

## Using the SDK

Add Restate and Tokio as dependencies:

```toml
[dependencies]
restate-sdk = "0.8"
tokio = { version = "1", features = ["full"] }
```

Then you're ready to develop your Restate service using Rust:

```rust
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
async fn main() {
    // To enable logging/tracing
    // tracing_subscriber::fmt::init();
    HttpServer::new(
        Endpoint::builder()
            .bind(Greeter)
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
```

## Calling services through ingress

The impl-block service macros also generate a typed ingress client for each service. Enable the
`reqwest-client` feature to use the SDK's built-in HTTP transport:

```toml
[dependencies]
restate-sdk = { version = "0.11", features = ["reqwest-client"] }
```

`ReqwestClient` is an alias for the transport-neutral `Client<reqwest::Client>`. Connect it to a
Restate ingress base URI, then wrap it in the generated `<Service>IngressClient`:

```rust
use restate_sdk::ingress::ReqwestClient;

let client = ReqwestClient::connect("http://localhost:8080".parse().unwrap()).unwrap();
let greeter = GreeterIngressClient::from_client(client);

// Wait for the invocation to complete and decode its typed result.
let response = greeter
    .greet("Ada".to_owned())
    .idempotency_key("greet-ada")
    .call()
    .await
    .unwrap();
let invocation = response.invocation_handle();
assert_eq!(response.into_body().unwrap(), "Greetings Ada");
println!("invocation ID: {}", invocation.invocation_id());

// Enqueue an invocation without waiting for its result.
let response = greeter
    .greet("Grace".to_owned())
    .send()
    .await
    .unwrap();
println!("send status: {:?}", response.send_status());
```

Generated object and workflow ingress clients additionally take their key in `from_client`. The
generic `Client<E>` and generated clients are available without `reqwest-client`; implement
`RequestExecutor` to use another buffered HTTP transport.

## Running on Lambda

The Restate Rust SDK supports running services on AWS Lambda using Lambda Function URLs. This allows you to deploy your Restate services as serverless functions.

### Setup

First, enable the `lambda` feature in your `Cargo.toml`:

```toml
[dependencies]
restate-sdk = { version = "0.8", features = ["lambda"] }
tokio = { version = "1", features = ["full"] }
```

### Basic Lambda Service

Here's how to create a simple Lambda service:

```rust
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
async fn main() {
    // To enable logging/tracing
    // check https://docs.aws.amazon.com/lambda/latest/dg/rust-logging.html#rust-logging-tracing

    // Build and run the Lambda endpoint
    LambdaEndpoint::run(
        Endpoint::builder()
            .bind(Greeter)
            .build(),
    )
    .await
    .unwrap();
}
```

### Deployment

1. Install `cargo-lambda`
   ```
   cargo install cargo-lambda
   ```
2. Build your Lambda function:

   ```bash
   cargo lambda build --release --arm64 --output-format zip
   ```

3. Create a Lambda function with the following configuration:

   - **Runtime**: Amazon Linux 2023
   - **Architecture**: arm64

4. Upload your `zip` file to the Lambda function.

### Logging

The SDK uses tokio's [`tracing`](https://docs.rs/tracing/latest/tracing/) crate to generate logs.
Just configure it as usual through [`tracing_subscriber`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/) to get your logs.

### Testing

The SDK uses [Testcontainers](https://rust.testcontainers.org/) to support integration testing using a Docker-deployed restate server.
The `restate-sdk-testcontainers` crate provides a framework for initializing the test environment, and an integration test example in `testcontainers/tests/test_container.rs`.

```rust
use restate_sdk::ingress::ReqwestClient;

#[tokio::test]
async fn test_container() {
    tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::INFO) // Set the maximum log level
        .init();

    let endpoint = Endpoint::builder().bind(MyService).build();

    // simple test environment initialization with default configuration
    // let test_environment = TestEnvironment::default().start(endpoint).await.unwrap();

    // custom test environment initialization
    let test_environment = TestEnvironment::new()
        // optional passthrough logging from the Restate server testcontainer
        // prints container logs to tracing::info level
        .with_container_logging()
        .with_container(
            "docker.restate.dev/restatedev/restate".to_string(),
            "1.7.2".to_string(),
        )
        .start(endpoint)
        .await
        .unwrap();

    let ingress_url = test_environment.ingress_url();

    let client = ReqwestClient::connect(ingress_url.parse().unwrap()).unwrap();
    let client = MyServiceIngressClient::from_client(client);

    let response = client
        .my_handler()
        .idempotency_key("abc")
        .call()
        .await
        .unwrap();
    let output = response.into_body().unwrap();

    assert_eq!(output, "hello!");
    info!("MyService/my_handler response: {output:?}");
}
```

## Versions

The Rust SDK is currently in active development, and might break across releases.

The compatibility with Restate is described in the following table:

| Restate Server\sdk-rust | 0.7 - 0.10 | 0.11 |
|-------------------------|------------|------|
| 1.6                     | ✅         | ✅   |
| 1.7                     | ✅         | ✅   |

Some features require a minimum version of both Restate and the SDK:

- **Typed ingress client and the new `/restate/` invocation routes**: requires Restate >= 1.7 with
  sdk-rust >= 0.11
- **Scope and limit key**: requires Restate >= 1.7 with sdk-rust >= 0.11

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

To release you must be part of the [owners team](https://github.com/orgs/restatedev/teams/owners).

To release we use [cargo-release](https://github.com/crate-ci/cargo-release).

```
cargo install cargo-release
```

Before releasing you need to log into crates.io for which you have to create an API token on https://crates.io/me

```
cargo login
```

You might have to use the `+nightly` toolchain because of releasing multiple crates at once.
First try the dry-run:

```
cargo +nightly release <VERSION> --exclude test-services --workspace
```

If everything looks good run with `--execute`

```
cargo +nightly release <VERSION> --exclude test-services --workspace --execute
```
