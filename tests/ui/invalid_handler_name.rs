#[allow(unused_imports)]
use restate_sdk::prelude::*;

// The `name = "..."` override is validated at compile time against the Restate handler-name
// pattern; a space is not allowed, so this must fail to compile (rather than panic at discovery).
#[restate_sdk::handler(name = "bad name")]
async fn greet(_ctx: Context<'_>) -> HandlerResult<()> {
    Ok(())
}

fn main() {}
