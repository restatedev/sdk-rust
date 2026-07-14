use restate_sdk::prelude::*;

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

object!(ListObject: { append, get, clear });
