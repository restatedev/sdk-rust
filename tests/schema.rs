use restate_sdk::prelude::*;
use restate_sdk::serde::{Json, WithSchema};
use restate_sdk::service::Discoverable;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
struct TestUser {
    name: String,
    age: u32,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
struct Person {
    name: String,
    age: u32,
    address: Address,
}

#[derive(Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
struct Address {
    street: String,
    city: String,
}

#[restate_sdk::service]
trait SchemaTestService {
    async fn string_handler(input: String) -> HandlerResult<i32>;
    async fn no_input_handler() -> HandlerResult<String>;
    async fn json_handler(input: Json<TestUser>) -> HandlerResult<Json<TestUser>>;
    async fn complex_handler(input: Json<Person>) -> HandlerResult<Json<HashMap<String, Person>>>;
}

struct SchemaTestServiceImpl;

impl SchemaTestService for SchemaTestServiceImpl {
    async fn string_handler(&self, _ctx: Context<'_>, _input: String) -> HandlerResult<i32> {
        Ok(42)
    }
    async fn no_input_handler(&self, _ctx: Context<'_>) -> HandlerResult<String> {
        Ok("No input".to_string())
    }
    async fn json_handler(
        &self,
        _ctx: Context<'_>,
        input: Json<TestUser>,
    ) -> HandlerResult<Json<TestUser>> {
        Ok(input)
    }
    async fn complex_handler(
        &self,
        _ctx: Context<'_>,
        input: Json<Person>,
    ) -> HandlerResult<Json<HashMap<String, Person>>> {
        Ok(Json(HashMap::from([("original".to_string(), input.0)])))
    }
}

#[test]
fn schema_discovery_and_validation() {
    let discovery = ServeSchemaTestService::<SchemaTestServiceImpl>::discover();
    assert_eq!(discovery.name.to_string(), "SchemaTestService");
    assert_eq!(discovery.handlers.len(), 4);

    for handler in &discovery.handlers {
        let input = handler
            .input
            .as_ref()
            .expect("Handler should have input schema");
        let output = handler
            .output
            .as_ref()
            .expect("Handler should have output schema");
        let input_schema = input
            .json_schema
            .as_ref()
            .expect("Input schema should exist");
        let output_schema = output
            .json_schema
            .as_ref()
            .expect("Output schema should exist");

        match handler.name.to_string().as_str() {
            "string_handler" => {
                assert_eq!(
                    input_schema.get("type").and_then(|v| v.as_str()),
                    Some("string")
                );
                assert_eq!(
                    output_schema.get("type").and_then(|v| v.as_str()),
                    Some("integer")
                );
            }
            "no_input_handler" => {
                assert_eq!(
                    input_schema.get("type").and_then(|v| v.as_str()),
                    Some("null")
                );
                assert_eq!(
                    output_schema.get("type").and_then(|v| v.as_str()),
                    Some("string")
                );
            }
            "json_handler" => {
                #[cfg(feature = "schemars")]
                {
                    let obj = input_schema
                        .as_object()
                        .expect("Schema should be an object");
                    assert!(
                        obj.contains_key("properties"),
                        "Json schema should have properties"
                    );
                    assert!(obj["properties"]["name"]["type"] == "string");
                    assert!(obj["properties"]["age"]["type"] == "integer");
                }
                #[cfg(not(feature = "schemars"))]
                assert_eq!(input_schema, &serde_json::json!({}));
            }
            "complex_handler" => {
                #[cfg(feature = "schemars")]
                {
                    let obj = input_schema
                        .as_object()
                        .expect("Schema should be an object");
                    assert!(obj.contains_key("properties") || obj.contains_key("$ref"));
                    let props = obj.get("properties").or_else(|| obj.get("$ref")).unwrap();
                    assert!(props.is_object(), "Complex schema should define structure");
                }
                #[cfg(not(feature = "schemars"))]
                assert_eq!(input_schema, &serde_json::json!({}));
            }
            _ => unreachable!("Unexpected handler"),
        }
    }
}

#[test]
fn schema_generation() {
    let string_schema = <String as WithSchema>::generate_schema();
    assert_eq!(string_schema["type"], "string");

    let json_schema = <Json<TestUser> as WithSchema>::generate_schema();
    #[cfg(feature = "schemars")]
    assert!(json_schema["properties"]["name"]["type"] == "string");
    #[cfg(not(feature = "schemars"))]
    assert_eq!(json_schema, serde_json::json!({}));
}
