use restate_sdk::prelude::*;

#[restate_sdk::handler]
pub async fn my_handler(ctx: ObjectContext<'_>, greeting: String) -> Result<String, HandlerError> {
    Ok(format!("Greetings {} {}", greeting, ctx.key()))
}

#[restate_sdk::handler]
pub async fn my_concurrent_handler(
    ctx: SharedObjectContext<'_>,
    greeting: String,
) -> Result<String, HandlerError> {
    Ok(format!("Greetings {} {}", greeting, ctx.key()))
}

// Defines MyVirtualObject
object!(MyVirtualObject: { my_handler, my_concurrent_handler });
