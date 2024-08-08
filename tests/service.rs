use restate_sdk::HandlerResult;

struct Greeter;

#[restate_sdk::service]
impl Greeter {
    #[handler]
    async fn greet(ctx: Context, input: String) -> HandlerResult<String> {}
}
