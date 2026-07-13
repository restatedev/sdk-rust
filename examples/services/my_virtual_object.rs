use restate_sdk::prelude::*;

// Generates `MyVirtualObjectClient` and the `MyVirtualObject::server` builder.
restate_sdk::interface! {
    object MyVirtualObject {
        my_handler(String) -> String;
        my_concurrent_handler(String) -> String;
    }
}

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
