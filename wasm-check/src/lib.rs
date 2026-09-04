//! CI-only crate; see Cargo.toml.
//!
//! Exports one function that reaches every clock-touching SDK path (`ctx.run`,
//! `sleep`, `send_after`, `drain_input`), so a release build of this crate for
//! wasm32-unknown-unknown retains them and the binary can be searched for the
//! "time not implemented on this platform" / "there is no reactor running" panics
//! that `std::time` / `tokio::time` compile to on that target. Unlike the
//! `disallowed-methods` lint (`wasm-check/clippy.toml`, selected via
//! `CLIPPY_CONF_DIR` in `just check-wasm32`), this sees through dependencies.

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
