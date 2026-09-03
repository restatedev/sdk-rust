//! CI-only crate; see Cargo.toml.
//!
//! Two jobs:
//!
//! 1. Supply the `getrandom` backends so `cargo clippy -p restate-sdk -p
//!    restate-sdk-wasm-check --target wasm32-unknown-unknown` resolves, which
//!    lets the `disallowed-methods` lint in `wasm-check/clippy.toml` (selected
//!    via `CLIPPY_CONF_DIR` in `just check-wasm32`) run against the SDK.
//! 2. Export one function that reaches every clock-touching SDK path (`ctx.run`,
//!    `sleep`, `send_after`, `drain_input`), so a release build of this crate for
//!    wasm32 retains them and the binary can be searched for the
//!    "time not implemented on this platform" panic that `std::time` compiles to
//!    on that target. Unlike the lint, this sees through dependencies.

use std::time::Duration;

use restate_sdk::prelude::*;

struct Probe;

#[restate_sdk::object]
impl Probe {
    #[handler]
    async fn run(&self, ctx: ObjectContext<'_>) -> Result<u32, HandlerError> {
        let n: u32 = ctx.run(|| async { Ok(42) }).await?;
        ctx.sleep(Duration::from_millis(1)).await?;
        ctx.object_client::<ProbeClient>("other")
            .ping()
            .send_after(Duration::from_secs(1));
        Ok(n)
    }

    #[handler]
    async fn ping(&self, _ctx: ObjectContext<'_>) -> Result<(), HandlerError> {
        Ok(())
    }
}

/// Reachable from the wasm export table so the linker keeps the whole
/// invocation path. Never meant to be called.
#[unsafe(no_mangle)]
pub extern "C" fn restate_sdk_wasm_check_probe() -> u16 {
    let endpoint = Endpoint::builder().bind(Probe).build();
    let req = http::Request::builder()
        .uri("/invoke/Probe/run")
        .body(http_body_util::Full::new(bytes::Bytes::new()))
        .unwrap();
    endpoint
        .handle_with_options(
            req,
            HandleOptions {
                protocol_mode: ProtocolMode::RequestResponse,
            },
        )
        .status()
        .as_u16()
}
