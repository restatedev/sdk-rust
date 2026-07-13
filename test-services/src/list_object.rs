use restate_sdk::prelude::*;
use restate_sdk::service::ServiceDefinition;

const LIST: &str = "list";

#[restate_sdk::handler]
pub(crate) async fn append(ctx: ObjectContext<'_>, value: String) -> HandlerResult<()> {
    let mut list = ctx
        .get::<Json<Vec<String>>>(LIST)
        .await?
        .unwrap_or_default()
        .into_inner();
    list.push(value);
    ctx.set(LIST, Json(list));
    Ok(())
}

#[restate_sdk::handler]
pub(crate) async fn get(ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<String>>> {
    Ok(ctx
        .get::<Json<Vec<String>>>(LIST)
        .await?
        .unwrap_or_default())
}

#[restate_sdk::handler]
pub(crate) async fn clear(ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<String>>> {
    let current = ctx
        .get::<Json<Vec<String>>>(LIST)
        .await?
        .unwrap_or_default();
    ctx.clear(LIST);
    Ok(current)
}

pub(crate) fn definition() -> ServiceDefinition {
    define_object("ListObject")
        .handler(append)
        .handler(get)
        .handler(clear)
        .build()
}
