use restate_sdk::prelude::*;
use restate_sdk::serde::{Json, PayloadMetadata};
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
    async fn empty_output_handler(input: String) -> HandlerResult<()>;
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
    async fn empty_output_handler(&self, _ctx: Context<'_>, _input: String) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn schema_discovery_and_validation() {
    let discovery = ServeSchemaTestService::<SchemaTestServiceImpl>::discover();
    assert_eq!(discovery.name.to_string(), "SchemaTestService");
    assert_eq!(discovery.handlers.len(), 5);

    for handler in &discovery.handlers {
        let input = handler
            .input
            .as_ref()
            .expect("Handler should have input schema");
        let output = handler
            .output
            .as_ref()
            .expect("Handler should have output schema");

        match handler.name.to_string().as_str() {
            "string_handler" | "json_handler" | "complex_handler" | "empty_output_handler" => {
                let input_schema = input
                    .json_schema
                    .as_ref()
                    .expect("Input schema should exist for handlers with input");
                let output_schema = output.json_schema.as_ref();

                match handler.name.to_string().as_str() {
                    "string_handler" => {
                        assert_eq!(
                            input_schema.get("type").and_then(|v| v.as_str()),
                            Some("string")
                        );
                        assert!(output_schema.is_some());
                        assert_eq!(
                            output_schema.unwrap().get("type").and_then(|v| v.as_str()),
                            Some("integer")
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
                    "empty_output_handler" => {
                        assert_eq!(
                            input_schema.get("type").and_then(|v| v.as_str()),
                            Some("string")
                        );
                        // For empty output handler, we don't expect json_schema to be set in output
                        assert!(
                            output_schema.is_none(),
                            "Empty output handler should have json_schema set to None"
                        );
                        // Verify that set_content_type_if_empty is set
                        assert_eq!(output.set_content_type_if_empty, Some(false));
                    }
                    _ => unreachable!("Unexpected handler"),
                }
            }
            "no_input_handler" => {
                // For no_input_handler, we don't expect json_schema to be set
                assert!(
                    input.json_schema.is_none(),
                    "No input handler should have json_schema set to None"
                );

                let output_schema = output
                    .json_schema
                    .as_ref()
                    .expect("Output schema should exist");

                assert_eq!(
                    output_schema.get("type").and_then(|v| v.as_str()),
                    Some("string")
                );
            }
            _ => unreachable!("Unexpected handler"),
        }
    }
}

#[test]
fn schema_generation() {
    let string_schema = <String as PayloadMetadata>::json_schema().unwrap();
    assert_eq!(string_schema["type"], "string");

    let json_schema = <Json<TestUser> as PayloadMetadata>::json_schema().unwrap();
    #[cfg(feature = "schemars")]
    assert!(json_schema["properties"]["name"]["type"] == "string");
    #[cfg(not(feature = "schemars"))]
    assert_eq!(json_schema, serde_json::json!({}));
}
