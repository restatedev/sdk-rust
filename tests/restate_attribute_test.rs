use restate_sdk::prelude::*;

// Test the new #[restate(...)] attribute with various configurations
#[restate_sdk::object]
trait TestObject {
    // Test using just the shared flag
    #[restate(shared)]
    async fn test_shared_flag() -> HandlerResult<String>;

    // Test using named boolean syntax
    #[restate(shared = true)]
    async fn test_shared_named() -> HandlerResult<String>;

    // Test using lazy_state flag
    #[restate(lazy_state)]
    async fn test_lazy_state_flag() -> HandlerResult<String>;

    // Test combining multiple attributes
    #[restate(shared, lazy_state = false)]
    async fn test_combined() -> HandlerResult<String>;

    // Test with timeout configuration
    #[restate(inactivity_timeout = "30s")]
    async fn test_inactivity_timeout() -> HandlerResult<()>;

    // Test with multiple timeout configurations
    #[restate(shared, inactivity_timeout = "30s", abort_timeout = "5m")]
    async fn test_multiple_timeouts() -> HandlerResult<String>;

    // Test with various duration formats
    #[restate(inactivity_timeout = "100ms")]
    async fn test_milliseconds() -> HandlerResult<()>;

    #[restate(abort_timeout = "2h")]
    async fn test_hours() -> HandlerResult<()>;

    // Regular handler without any special config
    async fn regular_handler() -> HandlerResult<()>;
}

struct TestObjectImpl;

impl TestObject for TestObjectImpl {
    async fn test_shared_flag(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<String> {
        Ok("shared flag".to_string())
    }

    async fn test_shared_named(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<String> {
        Ok("shared named".to_string())
    }

    async fn test_lazy_state_flag(&self, _ctx: ObjectContext<'_>) -> HandlerResult<String> {
        Ok("lazy state flag".to_string())
    }

    async fn test_combined(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<String> {
        Ok("combined".to_string())
    }

    async fn test_inactivity_timeout(&self, _ctx: ObjectContext<'_>) -> HandlerResult<()> {
        Ok(())
    }

    async fn test_multiple_timeouts(
        &self,
        _ctx: SharedObjectContext<'_>,
    ) -> HandlerResult<String> {
        Ok("multiple timeouts".to_string())
    }

    async fn test_milliseconds(&self, _ctx: ObjectContext<'_>) -> HandlerResult<()> {
        Ok(())
    }

    async fn test_hours(&self, _ctx: ObjectContext<'_>) -> HandlerResult<()> {
        Ok(())
    }

    async fn regular_handler(&self, _ctx: ObjectContext<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn test_restate_attribute_compiles() {
    // This test just verifies that the code compiles correctly
    // The actual functionality is tested through the discovery mechanism
    let _service = TestObjectImpl.serve();
}
