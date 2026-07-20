#![allow(unused_imports)]
use restate_sdk::prelude::*;

#[derive(Clone)]
struct Dep;

// Borrowed extensions are not supported: extensions are cloned out (like axum). Use `Extension<Dep>`.
#[restate_sdk::handler]
async fn borrow_ext(_ctx: Context<'_>, Extension(_dep): Extension<&Dep>) -> HandlerResult<()> {
    Ok(())
}

fn main() {}
