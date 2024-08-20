use restate_sdk::prelude::*;

#[restate_sdk::object]
#[name = "ListObject"]
pub(crate) trait ListObject {
    #[name = "append"]
    async fn append(value: String) -> HandlerResult<()>;
    #[name = "get"]
    async fn get() -> HandlerResult<Json<Vec<String>>>;
    #[name = "clear"]
    async fn clear() -> HandlerResult<Json<Vec<String>>>;
}

pub(crate) struct ListObjectImpl;

const LIST: &str = "list";

impl ListObject for ListObjectImpl {
    async fn append(&self, ctx: ObjectContext<'_>, value: String) -> HandlerResult<()> {
        let mut list = ctx
            .get::<Json<Vec<String>>>(LIST)
            .await?
            .unwrap_or_default()
            .into_inner();
        list.push(value);
        ctx.set(LIST, Json(list));
        Ok(())
    }

    async fn get(&self, ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<String>>> {
        Ok(ctx
            .get::<Json<Vec<String>>>(LIST)
            .await?
            .unwrap_or_default())
    }

    async fn clear(&self, ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<String>>> {
        let get = ctx
            .get::<Json<Vec<String>>>(LIST)
            .await?
            .unwrap_or_default();
        ctx.clear(LIST);
        Ok(get)
    }
}
