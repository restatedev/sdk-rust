use restate_sdk::prelude::*;

pub(crate) struct ListObject;

const LIST: &str = "list";

#[restate_sdk::object(vis = "pub(crate)", name = "ListObject")]
impl ListObject {
    #[handler(name = "append")]
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

    #[handler(name = "get")]
    async fn get(&self, ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<String>>> {
        Ok(ctx
            .get::<Json<Vec<String>>>(LIST)
            .await?
            .unwrap_or_default())
    }

    #[handler(name = "clear")]
    async fn clear(&self, ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<String>>> {
        let get = ctx
            .get::<Json<Vec<String>>>(LIST)
            .await?
            .unwrap_or_default();
        ctx.clear(LIST);
        Ok(get)
    }
}
