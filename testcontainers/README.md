[![Documentation](https://img.shields.io/docsrs/restate-sdk-testcontainers)](https://docs.rs/restate-sdk-testcontainers)
[![crates.io](https://img.shields.io/crates/v/restate_sdk_testcontainers.svg)](https://crates.io/crates/restate-sdk-testcontainers/)
[![Examples](https://img.shields.io/badge/view-examples-blue)](https://github.com/restatedev/examples)
[![Discord](https://img.shields.io/discord/1128210118216007792?logo=discord)](https://discord.gg/skW3AZ6uGd)
[![Twitter](https://img.shields.io/twitter/follow/restatedev.svg?style=social&label=Follow)](https://twitter.com/intent/follow?screen_name=restatedev)

# Restate Rust SDK Testcontainers

The SDK uses [Testcontainers](https://rust.testcontainers.org/) to support integration testing using a Docker-deployed restate server.
The `restate-sdk-testcontainers` crate provides a framework for initializing the test environment, and an integration test example in `testcontainers/tests/test_container.rs`.
The typed HTTP client shown below requires the `reqwest-client` feature on `restate-sdk`.

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
            "docker.io/restatedev/restate".to_string(),
            "latest".to_string(),
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
