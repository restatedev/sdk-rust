use restate_sdk::prelude::*;

// Test backward compatibility with the old #[shared] attribute
#[restate_sdk::object]
trait BackwardCompatObject {
    // Old syntax should still work
    #[shared]
    async fn legacy_shared() -> HandlerResult<String>;

    // Mix old and new syntax (as long as they don't conflict)
    #[shared]
    #[restate(inactivity_timeout = "30s")]
    async fn mixed_syntax() -> HandlerResult<String>;

    // New syntax
    #[restate(shared)]
    async fn new_shared() -> HandlerResult<String>;
}

struct BackwardCompatObjectImpl;

impl BackwardCompatObject for BackwardCompatObjectImpl {
    async fn legacy_shared(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<String> {
        Ok("legacy".to_string())
    }

    async fn mixed_syntax(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<String> {
        Ok("mixed".to_string())
    }

    async fn new_shared(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<String> {
        Ok("new".to_string())
    }
}

#[test]
fn test_backward_compatibility() {
    // This test verifies backward compatibility with old attributes
    let _service = BackwardCompatObjectImpl.serve();
}
