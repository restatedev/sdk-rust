# Release Notes for Issue #127: In-process Restate Cloud tunnel

## New Feature

The SDK can now serve an endpoint over outbound HTTP/2 connections to Restate Cloud, so a
service no longer needs an inbound HTTP port. It is Unix-only and behind the non-default `tunnel`
feature (plus a crypto backend, `rust_crypto` or `aws_lc_rs`):

```toml
restate-sdk = { version = "0.11", features = ["tunnel"] }
```

The operator supplies the `RESTATE_INPROC_*` environment variables and a projected token Secret, so
the application just runs the tunnel:

```rust
use restate_sdk::prelude::*;

let endpoint = Endpoint::builder().bind(Greeter).build();
Tunnel::new(endpoint).run().await?;
```

`run()` handles SIGINT/SIGTERM: the first signal drains gracefully, a second forces close. For an
application-managed lifecycle, use `connect()` (returns after the first successful handshake):

```rust
let connection = Tunnel::new(endpoint).connect().await?;
println!("{}", connection.info().deployment_url());
connection.shutdown().await?; // graceful drain; or .close() for abrupt
```
