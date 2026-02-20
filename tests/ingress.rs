#[cfg(feature = "ingress-client")]
mod ingress_client_tests {
    use assert_matches::assert_matches;
    use restate_sdk::prelude::*;
    use restate_sdk::serde::Serialize as RestateSerialize;
    use serde::{Deserialize, Serialize};
    use std::error::Error;
    use std::time::Duration;
    use wiremock::matchers::{body_json, body_string, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    type TestResult = Result<(), Box<dyn Error>>;

    #[restate_sdk::service]
    trait GreeterService {
        async fn greet(name: String) -> HandlerResult<String>;
    }

    #[restate_sdk::service]
    trait UnitService {
        async fn ping() -> HandlerResult<()>;
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct TestUser {
        name: String,
        age: u32,
    }

    #[restate_sdk::service]
    trait JsonService {
        async fn echo_user(user: Json<TestUser>) -> HandlerResult<String>;
    }

    #[restate_sdk::service]
    #[name = "renamed-http-service"]
    trait RenamedService {
        #[name = "renamed-http-handler"]
        async fn greet(name: String) -> HandlerResult<String>;
    }

    #[restate_sdk::object]
    trait GreeterObject {
        async fn greet(name: String) -> HandlerResult<String>;
    }

    #[restate_sdk::workflow]
    trait GreeterWorkflow {
        async fn greet(name: String) -> HandlerResult<String>;
    }

    #[restate_sdk::service]
    trait FailingService {
        async fn fail(req: FailingPayload) -> HandlerResult<String>;
    }

    #[test]
    fn reqwest_builder_exposes_typed_service_client() -> TestResult {
        let client =
            ingress::Client::new(ingress::ServerUrl::try_from("http://localhost:8080")?, None)?;

        let _request = client
            .service_client::<GreeterServiceClient>()
            .greet("hi".to_string());
        Ok(())
    }

    #[test]
    fn auth_token_rejects_invalid_auth_header() -> TestResult {
        let err = ingress::AuthToken::new("token\nbad".to_string().into());
        assert!(err.is_err(), "invalid auth header should fail");
        Ok(())
    }

    #[test]
    fn reqwest_builder_accepts_auth_token() -> TestResult {
        let token = ingress::AuthToken::new("token".to_string().into())?;
        let server_url = "https://localhost:8080/".try_into()?;
        let _client = ingress::Client::new(server_url, Some(token))?;
        Ok(())
    }

    #[tokio::test]
    async fn reqwest_service_client_call_executes_http_request() -> TestResult {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/GreeterService/greet"))
            .and(body_string("\"hi\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("\"hello hi\""),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = ingress::Client::new(server.uri().try_into()?, None)?;

        let response = client
            .service_client::<GreeterServiceClient>()
            .greet("hi".to_string())
            .call()
            .await?;

        assert_eq!(response, "hello hi");
        Ok(())
    }

    #[tokio::test]
    async fn reqwest_service_client_call_accepts_empty_body_for_unit_response() -> TestResult {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/UnitService/ping"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(""),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = ingress::Client::new(server.uri().try_into()?, None)?;

        client
            .service_client::<UnitServiceClient>()
            .ping()
            .call()
            .await?;

        Ok(())
    }

    #[tokio::test]
    async fn reqwest_service_client_call_supports_json_wrapper_types() -> TestResult {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/JsonService/echo_user"))
            .and(body_json(serde_json::json!({"name":"alice","age":42})))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("\"ok\""),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = ingress::Client::new(server.uri().try_into()?, None)?;

        let response = client
            .service_client::<JsonServiceClient>()
            .echo_user(Json(TestUser {
                name: "alice".to_string(),
                age: 42,
            }))
            .call()
            .await?;

        assert_eq!(response, "ok");
        Ok(())
    }

    struct FailingPayload;

    impl RestateSerialize for FailingPayload {
        type Error = serde_json::Error;

        fn serialize(&self) -> Result<bytes::Bytes, Self::Error> {
            Err(serde_json::Error::io(std::io::Error::other(
                "intentional serialize failure",
            )))
        }
    }

    impl restate_sdk::serde::Deserialize for FailingPayload {
        type Error = std::convert::Infallible;

        fn deserialize(_: &mut bytes::Bytes) -> Result<Self, Self::Error> {
            Ok(Self)
        }
    }

    impl restate_sdk::serde::PayloadMetadata for FailingPayload {}

    #[tokio::test]
    async fn reqwest_builder_post_propagates_serialize_failure() -> TestResult {
        let client =
            ingress::Client::new(ingress::ServerUrl::try_from("http://localhost:8080")?, None)?;

        let result = client
            .service_client::<FailingServiceClient>()
            .fail(FailingPayload)
            .call()
            .await;

        assert_matches!(result, Err(ingress::RequestError::Serde(_)));
        Ok(())
    }

    #[tokio::test]
    async fn reqwest_service_client_call_propagates_non_success_status() -> TestResult {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/GreeterService/greet"))
            .respond_with(ResponseTemplate::new(503).set_body_string("downstream unavailable"))
            .expect(1)
            .mount(&server)
            .await;

        let client = ingress::Client::new(server.uri().try_into()?, None)?;

        let result = client
            .service_client::<GreeterServiceClient>()
            .greet("hi".to_string())
            .call()
            .await;

        assert_matches!(
            result,
            Err(ingress::RequestError::Status { status: 503, ref body })
            if body.contains("downstream unavailable")
        );
        Ok(())
    }

    #[tokio::test]
    async fn reqwest_service_client_call_serializes_delay_query_param() -> TestResult {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/GreeterService/greet"))
            .and(query_param("delay", "5000ms"))
            .and(body_string("\"hi\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("\"hello hi\""),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = ingress::Client::new(server.uri().try_into()?, None)?;

        let response = client
            .service_client::<GreeterServiceClient>()
            .greet("hi".to_string())
            .delay(Duration::from_secs(5))
            .call()
            .await?;

        assert_eq!(response, "hello hi");
        Ok(())
    }

    #[tokio::test]
    async fn reqwest_service_client_uses_renamed_service_and_handler_paths() -> TestResult {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/renamed-http-service/renamed-http-handler"))
            .and(body_string("\"hi\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("\"hello hi\""),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = ingress::Client::new(server.uri().try_into()?, None)?;

        let response = client
            .service_client::<RenamedServiceClient>()
            .greet("hi".to_string())
            .call()
            .await?;

        assert_eq!(response, "hello hi");
        Ok(())
    }

    #[tokio::test]
    async fn reqwest_service_client_call_propagates_idempotency_key_header() -> TestResult {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/GreeterService/greet"))
            .and(header("idempotency-key", "test-key-1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("\"hello hi\""),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = ingress::Client::new(server.uri().try_into()?, None)?;

        let response = client
            .service_client::<GreeterServiceClient>()
            .greet("hi".to_string())
            .idempotency_key("test-key-1")
            .call()
            .await?;

        assert_eq!(response, "hello hi");
        Ok(())
    }

    #[tokio::test]
    async fn reqwest_keyed_clients_call_execute_http_request() -> TestResult {
        let object_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/GreeterObject/my-object-key/greet"))
            .and(body_string("\"hi\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("\"hello hi\""),
            )
            .expect(1)
            .mount(&object_server)
            .await;

        let object_client = ingress::Client::new(object_server.uri().try_into()?, None)?;

        let object_response = object_client
            .object_client::<GreeterObjectClient>("my-object-key")
            .greet("hi".to_string())
            .call()
            .await?;

        assert_eq!(object_response, "hello hi");

        let workflow_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/GreeterWorkflow/my-workflow-key/greet"))
            .and(body_string("\"hi\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("\"hello hi\""),
            )
            .expect(1)
            .mount(&workflow_server)
            .await;

        let workflow_client = ingress::Client::new(workflow_server.uri().try_into()?, None)?;

        let workflow_response = workflow_client
            .workflow_client::<GreeterWorkflowClient>("my-workflow-key")
            .greet("hi".to_string())
            .call()
            .await?;

        assert_eq!(workflow_response, "hello hi");
        Ok(())
    }
}
