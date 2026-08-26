# Release Notes for Issue #33: Add a typed ingress client

## New Feature

The SDK now generates a typed `<Type>IngressClient` for every impl-block service, virtual object,
and workflow. Ingress clients invoke handlers from outside a Restate service and require Restate
1.7 or newer.

Enable the `reqwest-client` feature to use the built-in reqwest client:

```rust
use restate_sdk::ingress::ReqwestClient;

let client = ReqwestClient::connect("http://localhost:8080".parse()?)?;
let greeter = GreeterIngressClient::from_client(client);
let greeting = greeter
    .greet("Ada".to_owned())
    .call()
    .await?
    .into_body()?;

println!("{greeting}");
```

To use another HTTP client, implement `RequestExecutor` and pass it to `Client::new`.
