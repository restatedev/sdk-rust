use std::collections::VecDeque;
use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{Future, ready};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderName, HeaderValue, Method, Request, Response, StatusCode, Version};
use restate_sdk::ingress::{
    Client, ClientBuildError, ClientError, InvocationId, Output, RequestExecutor, RequestTarget,
    SendStatus,
};
use restate_sdk::prelude::{
    Context, HandlerResult, ObjectContext, SharedObjectContext, SharedWorkflowContext,
    WorkflowContext,
};
use restate_sdk::serde::{Deserialize, InputMetadata, OutputMetadata, PayloadMetadata, Serialize};

const X_RESTATE_ID: HeaderName = HeaderName::from_static("x-restate-id");

#[derive(Debug)]
struct FakeTransportError(&'static str);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResponseExtension(&'static str);

impl fmt::Display for FakeTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for FakeTransportError {}

#[derive(Default)]
struct FakeState {
    requests: Vec<Request<Bytes>>,
    results: VecDeque<Result<Response<Bytes>, FakeTransportError>>,
}

#[derive(Clone, Default)]
struct Capture {
    state: Arc<Mutex<FakeState>>,
}

impl Capture {
    fn executor(&self) -> FakeExecutor {
        FakeExecutor {
            state: Arc::clone(&self.state),
        }
    }

    fn respond(&self, response: Response<Bytes>) {
        self.state.lock().unwrap().results.push_back(Ok(response));
    }

    fn fail(&self, message: &'static str) {
        self.state
            .lock()
            .unwrap()
            .results
            .push_back(Err(FakeTransportError(message)));
    }

    fn request_count(&self) -> usize {
        self.state.lock().unwrap().requests.len()
    }

    fn take_requests(&self) -> Vec<Request<Bytes>> {
        std::mem::take(&mut self.state.lock().unwrap().requests)
    }
}

// Deliberately not Clone. The client and its handles must still be cheaply cloneable.
struct FakeExecutor {
    state: Arc<Mutex<FakeState>>,
}

impl RequestExecutor for FakeExecutor {
    type Error = FakeTransportError;

    fn execute(
        &self,
        request: Request<Bytes>,
    ) -> impl Future<Output = Result<Response<Bytes>, Self::Error>> + Send {
        let result = {
            let mut state = self.state.lock().unwrap();
            state.requests.push(request);
            state
                .results
                .pop_front()
                .expect("the test must queue a fake response")
        };
        ready(result)
    }
}

fn client(base_uri: &str) -> (Client<FakeExecutor>, Capture) {
    let capture = Capture::default();
    let client = Client::new(base_uri.parse().unwrap(), capture.executor()).unwrap();
    (client, capture)
}

fn raw_response(status: StatusCode, body: impl Into<Bytes>) -> Response<Bytes> {
    Response::builder()
        .status(status)
        .body(body.into())
        .unwrap()
}

fn invocation_response(
    status: StatusCode,
    invocation_id: &str,
    body: impl Into<Bytes>,
) -> Response<Bytes> {
    Response::builder()
        .status(status)
        .header(X_RESTATE_ID, invocation_id)
        .header("x-test-response", "retained")
        .body(body.into())
        .unwrap()
}

fn acknowledgement(invocation_id: &str, status: &str) -> Response<Bytes> {
    invocation_response(
        StatusCode::ACCEPTED,
        invocation_id,
        serde_json::json!({
            "invocationId": invocation_id,
            "status": status,
            "executionTime": "2030-01-02T03:04:05Z",
            "futureWireField": { "is": "ignored" }
        })
        .to_string(),
    )
}

fn expect_error<T>(result: Result<T, ClientError>) -> ClientError {
    match result {
        Ok(_) => panic!("expected the ingress operation to fail"),
        Err(error) => error,
    }
}

fn assert_preserved_response(
    error: ClientError,
    status: StatusCode,
    body: &[u8],
) -> Response<Bytes> {
    let response = error
        .response()
        .expect("HTTP and protocol errors must retain their response");
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["x-test-response"], "retained");
    assert_eq!(response.body().as_ref(), body);

    let response = error
        .into_response()
        .expect("the retained response must also be recoverable by value");
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["x-test-response"], "retained");
    assert_eq!(response.body().as_ref(), body);
    response
}

async fn issue_call(
    client: &Client<FakeExecutor>,
    capture: &Capture,
    target: RequestTarget,
    scope: Option<&str>,
    invocation_id: &str,
) {
    capture.respond(invocation_response(
        StatusCode::OK,
        invocation_id,
        Bytes::new(),
    ));
    let request = client.request::<(), ()>(target, ());
    let response = match scope {
        Some(scope) => request.scope(scope).call().await.unwrap(),
        None => request.call().await.unwrap(),
    };
    assert_eq!(
        response.invocation_handle().invocation_id().as_str(),
        invocation_id
    );
    response.into_body().unwrap();
}

async fn issue_send(
    client: &Client<FakeExecutor>,
    capture: &Capture,
    target: RequestTarget,
    scope: Option<&str>,
    invocation_id: &str,
) {
    capture.respond(acknowledgement(invocation_id, "Accepted"));
    let request = client.request::<(), ()>(target, ());
    let response = match scope {
        Some(scope) => request.scope(scope).send().await.unwrap(),
        None => request.send().await.unwrap(),
    };
    assert_eq!(
        response.invocation_handle().invocation_id().as_str(),
        invocation_id
    );
    assert_eq!(response.send_status(), SendStatus::Accepted);
}

#[test]
fn validates_base_uris_and_clones_without_executor_bounds() {
    let relative = match Client::new(
        "/relative/path".parse().unwrap(),
        Capture::default().executor(),
    ) {
        Ok(_) => panic!("relative base URI must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(relative, ClientBuildError::RelativeBaseUri));

    let no_authority = match Client::new(
        "mailto:someone@example.com".parse().unwrap(),
        Capture::default().executor(),
    ) {
        Ok(_) => panic!("base URI without authority must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(no_authority, ClientBuildError::RelativeBaseUri));

    let query = match Client::new(
        "http://example.test/proxy?token=nope".parse().unwrap(),
        Capture::default().executor(),
    ) {
        Ok(_) => panic!("base URI query must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(query, ClientBuildError::BaseUriHasQuery));

    let (client, _capture) = client("http://example.test/");
    let cloned = client.clone();

    struct NonCloneOutput;
    impl Deserialize for NonCloneOutput {
        type Error = Infallible;

        fn deserialize(_: &mut Bytes) -> Result<Self, Self::Error> {
            Ok(Self)
        }
    }

    let persisted_id: InvocationId = String::from("external-id");
    let handle = cloned.invocation_handle::<NonCloneOutput>(persisted_id);
    let cloned_handle = handle.clone();
    let _: &String = cloned_handle.invocation_id();
    assert_eq!(cloned_handle.invocation_id().as_str(), "external-id");
}

#[test]
fn ingress_reexports_the_context_request_target() {
    let target: restate_sdk::context::RequestTarget = RequestTarget::service("Service", "handler");
    let _: RequestTarget = target;
}

#[tokio::test]
async fn emits_only_new_call_and_send_route_shapes() {
    let (client, capture) = client("http://example.test/proxy///");

    issue_call(
        &client,
        &capture,
        RequestTarget::service("Service", "handler"),
        None,
        "call-service",
    )
    .await;
    issue_call(
        &client,
        &capture,
        RequestTarget::service("Service", "handler"),
        Some("tenant"),
        "call-scoped-service",
    )
    .await;
    issue_call(
        &client,
        &capture,
        RequestTarget::object("Object", "object-key", "handler"),
        None,
        "call-object",
    )
    .await;
    issue_call(
        &client,
        &capture,
        RequestTarget::workflow("Workflow", "workflow-key", "handler"),
        Some("tenant"),
        "call-scoped-workflow",
    )
    .await;

    issue_send(
        &client,
        &capture,
        RequestTarget::service("Service", "handler"),
        None,
        "send-service",
    )
    .await;
    issue_send(
        &client,
        &capture,
        RequestTarget::service("Service", "handler"),
        Some("tenant"),
        "send-scoped-service",
    )
    .await;
    issue_send(
        &client,
        &capture,
        RequestTarget::object("Object", "object-key", "handler"),
        None,
        "send-object",
    )
    .await;
    issue_send(
        &client,
        &capture,
        RequestTarget::workflow("Workflow", "workflow-key", "handler"),
        Some("tenant"),
        "send-scoped-workflow",
    )
    .await;

    let requests = capture.take_requests();
    let paths: Vec<_> = requests
        .iter()
        .map(|request| request.uri().path_and_query().unwrap().as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "/proxy/restate/call/Service/handler",
            "/proxy/restate/scope/tenant/call/Service/handler",
            "/proxy/restate/call/Object/object-key/handler",
            "/proxy/restate/scope/tenant/call/Workflow/workflow-key/handler",
            "/proxy/restate/send/Service/handler",
            "/proxy/restate/scope/tenant/send/Service/handler",
            "/proxy/restate/send/Object/object-key/handler",
            "/proxy/restate/scope/tenant/send/Workflow/workflow-key/handler",
        ]
    );
    assert!(
        requests
            .iter()
            .all(|request| request.method() == Method::POST)
    );
    assert!(requests.iter().all(|request| request.body().is_empty()));
    for path in paths {
        assert!(!path.contains("/invoke/"));
        assert!(!path.contains("/lookup/"));
        assert!(!path.contains("/services/"));
        assert!(!path.contains("/workflows/"));
    }
}

#[tokio::test]
async fn encodes_every_dynamic_segment_and_formats_send_delays() {
    let (client, capture) = client("http://example.test/gateway/v1/");

    capture.respond(invocation_response(StatusCode::OK, "encoded", Bytes::new()));
    client
        .request::<(), ()>(
            RequestTarget::object("svc /\u{e9}?", "key/#?", "handle %/\u{6771}"),
            (),
        )
        .scope("scope /\u{e9}?")
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    capture.respond(acknowledgement("delayed-fraction", "Accepted"));
    client
        .request::<(), ()>(RequestTarget::service("Timer", "fire"), ())
        .send_after(Duration::new(62, 120_000_000))
        .await
        .unwrap();

    capture.respond(acknowledgement("delayed-zero", "Accepted"));
    client
        .request::<(), ()>(RequestTarget::service("Timer", "fire"), ())
        .send_after(Duration::ZERO)
        .await
        .unwrap();

    let requests = capture.take_requests();
    assert_eq!(
        requests[0].uri().path(),
        concat!(
            "/gateway/v1/restate/scope/scope%20%2F%C3%A9%3F/call/",
            "svc%20%2F%C3%A9%3F/key%2F%23%3F/handle%20%25%2F%E6%9D%B1"
        )
    );
    assert_eq!(
        requests[1].uri().path_and_query().unwrap().as_str(),
        "/gateway/v1/restate/send/Timer/fire?delay=PT62.12S"
    );
    assert_eq!(
        requests[2].uri().path_and_query().unwrap().as_str(),
        "/gateway/v1/restate/send/Timer/fire?delay=PT0S"
    );
}

struct EmptyMarked;

impl Serialize for EmptyMarked {
    type Error = Infallible;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        Ok(Bytes::new())
    }
}

impl PayloadMetadata for EmptyMarked {
    fn input_metadata() -> InputMetadata {
        InputMetadata {
            accept_content_type: "*/*",
            is_required: false,
        }
    }

    fn output_metadata() -> OutputMetadata {
        OutputMetadata {
            content_type: "application/x-empty-marked",
            set_content_type_if_empty: true,
        }
    }
}

struct EmptyRequired;

impl Serialize for EmptyRequired {
    type Error = Infallible;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        Ok(Bytes::new())
    }
}

impl PayloadMetadata for EmptyRequired {
    fn input_metadata() -> InputMetadata {
        InputMetadata {
            // This wildcard must not be used as the outgoing content type.
            accept_content_type: "application/input-wildcard+*",
            is_required: true,
        }
    }

    fn output_metadata() -> OutputMetadata {
        OutputMetadata {
            content_type: "application/x-required-empty",
            set_content_type_if_empty: false,
        }
    }
}

#[tokio::test]
async fn merges_headers_and_uses_concrete_output_metadata() {
    let capture = Capture::default();
    let client = Client::builder("http://example.test".parse().unwrap(), capture.executor())
        .default_header(AUTHORIZATION, HeaderValue::from_static("Bearer test-token"))
        .default_header(
            HeaderName::from_static("x-overridden"),
            HeaderValue::from_static("default"),
        )
        .build()
        .unwrap();

    for id in [
        "string",
        "explicit",
        "unit",
        "bytes",
        "option-none",
        "marked",
        "required",
    ] {
        capture.respond(invocation_response(StatusCode::OK, id, Bytes::new()));
    }

    client
        .request::<String, ()>(
            RequestTarget::service("Metadata", "string"),
            "hello".to_owned(),
        )
        .header(
            HeaderName::from_static("x-overridden"),
            HeaderValue::from_static("request"),
        )
        .header(
            HeaderName::from_static("x-request"),
            HeaderValue::from_static("present"),
        )
        .idempotency_key("dedupe-123")
        .scope("tenant")
        .limit_key("api-key/user_42")
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    client
        .request::<Vec<u8>, ()>(
            RequestTarget::service("Metadata", "explicit"),
            b"binary".to_vec(),
        )
        .content_type(HeaderValue::from_static("application/x-explicit"))
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    client
        .request::<(), ()>(RequestTarget::service("Metadata", "unit"), ())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    client
        .request::<Vec<u8>, ()>(RequestTarget::service("Metadata", "bytes"), Vec::new())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    client
        .request::<Option<Vec<u8>>, ()>(RequestTarget::service("Metadata", "option-none"), None)
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    client
        .request::<EmptyMarked, ()>(RequestTarget::service("Metadata", "marked"), EmptyMarked)
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    client
        .request::<EmptyRequired, ()>(
            RequestTarget::service("Metadata", "required"),
            EmptyRequired,
        )
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    let requests = capture.take_requests();
    let string = &requests[0];
    assert_eq!(
        string.uri().path(),
        "/restate/scope/tenant/call/Metadata/string"
    );
    assert_eq!(string.headers()[AUTHORIZATION], "Bearer test-token");
    assert_eq!(string.headers()["x-overridden"], "request");
    assert_eq!(string.headers()["x-request"], "present");
    assert_eq!(string.headers()["idempotency-key"], "dedupe-123");
    assert_eq!(string.headers()["x-restate-limit-key"], "api-key/user_42");
    assert_eq!(string.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(string.body().as_ref(), br#""hello""#);

    assert_eq!(
        requests[1].headers()[CONTENT_TYPE],
        "application/x-explicit"
    );
    assert_eq!(requests[1].body().as_ref(), b"binary");
    assert!(!requests[2].headers().contains_key(CONTENT_TYPE));
    assert!(requests[2].body().is_empty());
    assert_eq!(
        requests[3].headers()[CONTENT_TYPE],
        "application/octet-stream"
    );
    assert!(requests[3].body().is_empty());
    assert!(!requests[4].headers().contains_key(CONTENT_TYPE));
    assert!(requests[4].body().is_empty());
    assert_eq!(
        requests[5].headers()[CONTENT_TYPE],
        "application/x-empty-marked"
    );
    assert_eq!(
        requests[6].headers()[CONTENT_TYPE],
        "application/x-required-empty"
    );
    assert_ne!(
        requests[6].headers()[CONTENT_TYPE],
        "application/input-wildcard+*"
    );
}

#[derive(Debug)]
struct SerializationFailure;

impl fmt::Display for SerializationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("intentional serialization failure")
    }
}

impl Error for SerializationFailure {}

struct CannotSerialize;

impl Serialize for CannotSerialize {
    type Error = SerializationFailure;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        Err(SerializationFailure)
    }
}

impl PayloadMetadata for CannotSerialize {}

#[tokio::test]
async fn rejects_invalid_request_options_before_transport() {
    let (base_client, capture) = client("http://example.test");

    let error = expect_error(
        base_client
            .request::<(), ()>(RequestTarget::service("Service", "handler"), ())
            .limit_key("tenant")
            .send()
            .await,
    );
    assert!(matches!(error, ClientError::InvalidRequest { .. }));

    for invalid in [
        "a//b",
        "a/b/c",
        "not@allowed",
        "1234567890123456789012345678901234567",
    ] {
        let (isolated_client, isolated_capture) = client("http://example.test");
        isolated_capture.respond(acknowledgement("must-not-execute", "Accepted"));
        match isolated_client
            .request::<(), ()>(RequestTarget::service("Service", "handler"), ())
            .scope("scope")
            .limit_key(invalid)
            .send()
            .await
        {
            Err(ClientError::InvalidRequest { .. }) => {}
            Err(error) => panic!("limit key {invalid:?} returned the wrong error: {error:?}"),
            Ok(_) => panic!("limit key {invalid:?} was unexpectedly accepted"),
        }
        assert_eq!(isolated_capture.request_count(), 0);
    }

    // These mirror the server's split-terminator rules. An empty key means no limiting and does
    // not need a scope; one trailing delimiter is accepted for either hierarchy depth.
    for (valid, scope) in [("", None), ("a/", Some("scope")), ("a/b/", Some("scope"))] {
        let (isolated_client, isolated_capture) = client("http://example.test");
        isolated_capture.respond(acknowledgement("valid-limit-key", "Accepted"));
        let request = isolated_client
            .request::<(), ()>(RequestTarget::service("Service", "handler"), ())
            .limit_key(valid);
        let response = match scope {
            Some(scope) => request.scope(scope).send().await.unwrap(),
            None => request.send().await.unwrap(),
        };
        assert_eq!(response.send_status(), SendStatus::Accepted);
        let requests = isolated_capture.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].headers()["x-restate-limit-key"], valid);
    }

    let error = expect_error(
        base_client
            .request::<CannotSerialize, ()>(
                RequestTarget::service("Service", "handler"),
                CannotSerialize,
            )
            .call()
            .await,
    );
    assert!(matches!(error, ClientError::Serialization { .. }));

    let error = expect_error(
        base_client
            .request::<(), ()>(RequestTarget::service("Service", "handler"), ())
            .idempotency_key("a header cannot contain\na newline")
            .call()
            .await,
    );
    assert!(matches!(error, ClientError::Request { .. }));
    assert_eq!(capture.request_count(), 0);
}

#[tokio::test]
async fn reports_transport_failures_without_an_http_response() {
    let (client, capture) = client("http://example.test");
    capture.fail("connection reset by fake peer");

    let error = expect_error(
        client
            .request::<(), ()>(RequestTarget::service("Service", "handler"), ())
            .call()
            .await,
    );
    assert!(matches!(error, ClientError::Transport { .. }));
    assert!(error.response().is_none());
    assert_eq!(capture.request_count(), 1);
}

#[tokio::test]
async fn call_requires_a_valid_id_then_an_exact_200_and_retains_errors() {
    let (client, capture) = client("http://example.test");

    capture.respond(invocation_response(
        StatusCode::OK,
        "call-ok",
        b"42".as_slice(),
    ));
    let response = client
        .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
        .call()
        .await
        .unwrap();
    assert_eq!(
        response.invocation_handle().invocation_id().as_str(),
        "call-ok"
    );
    assert_eq!(response.into_body().unwrap(), 42);

    let failed_body = br#"{"code":500,"message":"handler failed"}"#;
    capture.respond(invocation_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed-invocation",
        failed_body.as_slice(),
    ));
    let response = client
        .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
        .call()
        .await
        .unwrap();
    let handle = response.invocation_handle();
    assert_eq!(handle.invocation_id().as_str(), "failed-invocation");
    let error = expect_error(response.into_body());
    assert!(matches!(
        error,
        ClientError::Status {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            ..
        }
    ));
    assert_preserved_response(error, StatusCode::INTERNAL_SERVER_ERROR, failed_body);

    let created_body = b"43";
    capture.respond(invocation_response(
        StatusCode::CREATED,
        "not-exact-200",
        created_body.as_slice(),
    ));
    let response = client
        .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
        .call()
        .await
        .unwrap();
    let error = expect_error(response.into_body());
    assert_preserved_response(error, StatusCode::CREATED, created_body);

    let missing_id_body = b"44";
    let mut missing_id = raw_response(StatusCode::OK, missing_id_body.as_slice());
    missing_id
        .headers_mut()
        .insert("x-test-response", HeaderValue::from_static("retained"));
    capture.respond(missing_id);
    let error = expect_error(
        client
            .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
            .call()
            .await,
    );
    assert!(matches!(error, ClientError::Protocol { .. }));
    assert_preserved_response(error, StatusCode::OK, missing_id_body);

    let ingress_error_body = b"dispatcher rejected request";
    let mut ingress_error = raw_response(StatusCode::BAD_REQUEST, ingress_error_body.as_slice());
    ingress_error
        .headers_mut()
        .insert("x-test-response", HeaderValue::from_static("retained"));
    capture.respond(ingress_error);
    let error = expect_error(
        client
            .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
            .call()
            .await,
    );
    assert!(matches!(error, ClientError::Protocol { .. }));
    assert_preserved_response(error, StatusCode::BAD_REQUEST, ingress_error_body);

    let malformed_id_body = b"45";
    let mut malformed_id = raw_response(StatusCode::OK, malformed_id_body.as_slice());
    malformed_id
        .headers_mut()
        .insert("x-test-response", HeaderValue::from_static("retained"));
    malformed_id.headers_mut().insert(
        X_RESTATE_ID,
        HeaderValue::from_bytes(&[0xff]).expect("opaque header bytes are valid HTTP values"),
    );
    capture.respond(malformed_id);
    let error = expect_error(
        client
            .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
            .call()
            .await,
    );
    assert!(matches!(error, ClientError::Protocol { .. }));
    assert_preserved_response(error, StatusCode::OK, malformed_id_body);

    let invalid_payload = b"not-json";
    capture.respond(invocation_response(
        StatusCode::OK,
        "decode-failure",
        invalid_payload.as_slice(),
    ));
    let response = client
        .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
        .call()
        .await
        .unwrap();
    let error = expect_error(response.into_body());
    assert!(matches!(error, ClientError::PayloadDecode { .. }));
    assert_preserved_response(error, StatusCode::OK, invalid_payload);
}

#[tokio::test]
async fn call_response_conversions_preserve_the_http_response() {
    let (client, capture) = client("http://example.test");

    let mut typed = invocation_response(StatusCode::CREATED, "typed", b"43".as_slice());
    *typed.version_mut() = Version::HTTP_2;
    typed
        .extensions_mut()
        .insert(ResponseExtension("typed-extension"));
    capture.respond(typed);

    let typed = client
        .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
        .call()
        .await
        .unwrap()
        .into_http_response()
        .unwrap();
    assert_eq!(typed.status(), StatusCode::CREATED);
    assert_eq!(typed.version(), Version::HTTP_2);
    assert_eq!(typed.headers()["x-test-response"], "retained");
    assert_eq!(
        typed.extensions().get::<ResponseExtension>(),
        Some(&ResponseExtension("typed-extension"))
    );
    assert_eq!(*typed.body(), 43);

    let raw_body = b"raw-response";
    let mut raw = invocation_response(StatusCode::PARTIAL_CONTENT, "raw", raw_body.as_slice());
    *raw.version_mut() = Version::HTTP_11;
    raw.extensions_mut()
        .insert(ResponseExtension("raw-extension"));
    capture.respond(raw);

    let raw = client
        .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
        .call()
        .await
        .unwrap()
        .into_raw_http_response();
    assert_eq!(raw.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(raw.version(), Version::HTTP_11);
    assert_eq!(raw.headers()["x-test-response"], "retained");
    assert_eq!(
        raw.extensions().get::<ResponseExtension>(),
        Some(&ResponseExtension("raw-extension"))
    );
    assert_eq!(raw.body().as_ref(), raw_body);
}

#[tokio::test]
async fn into_http_response_decode_errors_retain_the_untouched_raw_response() {
    let (client, capture) = client("http://example.test");
    let invalid_payload = b"not-json";
    let mut response = invocation_response(
        StatusCode::IM_A_TEAPOT,
        "decode-failure",
        invalid_payload.as_slice(),
    );
    *response.version_mut() = Version::HTTP_2;
    response
        .extensions_mut()
        .insert(ResponseExtension("decode-extension"));
    capture.respond(response);

    let error = expect_error(
        client
            .request::<(), u64>(RequestTarget::service("Service", "answer"), ())
            .call()
            .await
            .unwrap()
            .into_http_response(),
    );
    assert!(matches!(error, ClientError::PayloadDecode { .. }));

    let response = error
        .response()
        .expect("payload decode errors must retain the raw response");
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.headers()["x-test-response"], "retained");
    assert_eq!(
        response.extensions().get::<ResponseExtension>(),
        Some(&ResponseExtension("decode-extension"))
    );
    assert_eq!(response.body().as_ref(), invalid_payload);

    let response = error
        .into_response()
        .expect("the untouched raw response must be recoverable by value");
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.headers()["x-test-response"], "retained");
    assert_eq!(
        response.extensions().get::<ResponseExtension>(),
        Some(&ResponseExtension("decode-extension"))
    );
    assert_eq!(response.body().as_ref(), invalid_payload);
}

#[tokio::test]
async fn send_accepts_only_202_and_exposes_both_wire_statuses() {
    let (client, capture) = client("http://example.test");

    capture.respond(acknowledgement("send-accepted", "Accepted"));
    let response = client
        .request::<(), String>(RequestTarget::service("Service", "handler"), ())
        .send()
        .await
        .unwrap();
    assert_eq!(response.send_status(), SendStatus::Accepted);
    let handle = response.invocation_handle();
    assert_eq!(handle.invocation_id().as_str(), "send-accepted");

    capture.respond(acknowledgement(
        "send-previously-accepted",
        "PreviouslyAccepted",
    ));
    let response = client
        .request::<(), String>(RequestTarget::service("Service", "handler"), ())
        .send()
        .await
        .unwrap();
    assert_eq!(response.send_status(), SendStatus::PreviouslyAccepted);
    assert_eq!(
        response.invocation_handle().invocation_id().as_str(),
        "send-previously-accepted"
    );

    let wrong_status_body = br#"{"invocationId":"wrong-status","status":"Accepted"}"#;
    capture.respond(invocation_response(
        StatusCode::OK,
        "wrong-status",
        wrong_status_body.as_slice(),
    ));
    let error = expect_error(
        client
            .request::<(), ()>(RequestTarget::service("Service", "handler"), ())
            .send()
            .await,
    );
    assert!(matches!(
        error,
        ClientError::Status {
            status: StatusCode::OK,
            ..
        }
    ));
    assert_preserved_response(error, StatusCode::OK, wrong_status_body);
}

#[tokio::test]
async fn handles_use_only_direct_id_get_routes_and_decode_attach_and_output() {
    let capture = Capture::default();
    let client = Client::builder(
        "http://example.test/proxy/".parse().unwrap(),
        capture.executor(),
    )
    .default_header(
        HeaderName::from_static("x-default"),
        HeaderValue::from_static("on-every-request"),
    )
    .build()
    .unwrap();
    let handle = client.invocation_handle::<String>("invocation /\u{e9}?");

    let mut attach_response =
        invocation_response(StatusCode::OK, "ignored", br#""attached""#.as_slice());
    *attach_response.status_mut() = StatusCode::OK;
    capture.respond(attach_response);
    let attached = handle.attach().await.unwrap();
    assert_eq!(attached.status(), StatusCode::OK);
    assert_eq!(attached.headers()["x-test-response"], "retained");
    assert_eq!(attached.into_body(), "attached");

    capture.respond(invocation_response(
        StatusCode::OK,
        "ignored",
        br#""ready""#.as_slice(),
    ));
    let ready = handle.output().await.unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(ready.headers()["x-test-response"], "retained");
    assert_eq!(ready.into_body(), Output::Ready("ready".to_owned()));

    capture.respond(invocation_response(
        StatusCode::from_u16(470).unwrap(),
        "ignored",
        b"not ready yet".as_slice(),
    ));
    let not_ready = handle.output().await.unwrap();
    assert_eq!(not_ready.status().as_u16(), 470);
    assert_eq!(not_ready.headers()["x-test-response"], "retained");
    assert_eq!(not_ready.into_body(), Output::NotReady);

    let requests = capture.take_requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method().clone(), request.uri().path()))
            .collect::<Vec<_>>(),
        [
            (
                Method::GET,
                "/proxy/restate/attach/invocation%20%2F%C3%A9%3F"
            ),
            (
                Method::GET,
                "/proxy/restate/output/invocation%20%2F%C3%A9%3F"
            ),
            (
                Method::GET,
                "/proxy/restate/output/invocation%20%2F%C3%A9%3F"
            ),
        ]
    );
    for request in requests {
        assert!(request.body().is_empty());
        assert_eq!(request.headers()["x-default"], "on-every-request");
        assert!(!request.uri().path().contains("lookup"));
        assert!(!request.uri().path().contains("/invoke/"));
    }
}

#[tokio::test]
async fn attach_and_output_errors_retain_raw_responses() {
    let (client, capture) = client("http://example.test");

    let attach_status_body = b"attach failed";
    capture.respond(invocation_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "id",
        attach_status_body.as_slice(),
    ));
    let error = expect_error(client.invocation_handle::<u64>("id").attach().await);
    assert!(matches!(error, ClientError::Status { .. }));
    assert_preserved_response(error, StatusCode::INTERNAL_SERVER_ERROR, attach_status_body);

    let attach_decode_body = b"not-a-number";
    capture.respond(invocation_response(
        StatusCode::OK,
        "id",
        attach_decode_body.as_slice(),
    ));
    let error = expect_error(client.invocation_handle::<u64>("id").attach().await);
    assert!(matches!(error, ClientError::PayloadDecode { .. }));
    assert_preserved_response(error, StatusCode::OK, attach_decode_body);

    let output_status_body = b"output failed";
    capture.respond(invocation_response(
        StatusCode::NOT_FOUND,
        "id",
        output_status_body.as_slice(),
    ));
    let error = expect_error(client.invocation_handle::<u64>("id").output().await);
    assert!(matches!(error, ClientError::Status { .. }));
    assert_preserved_response(error, StatusCode::NOT_FOUND, output_status_body);

    let output_decode_body = b"also-not-a-number";
    capture.respond(invocation_response(
        StatusCode::OK,
        "id",
        output_decode_body.as_slice(),
    ));
    let error = expect_error(client.invocation_handle::<u64>("id").output().await);
    assert!(matches!(error, ClientError::PayloadDecode { .. }));
    assert_preserved_response(error, StatusCode::OK, output_decode_body);

    assert!(
        capture
            .take_requests()
            .iter()
            .all(|request| request.method() == Method::GET)
    );
}

#[tokio::test]
async fn workflow_handle_resolves_invocation_id_through_lookup() {
    let capture = Capture::default();
    let client = Client::builder(
        "http://example.test/proxy/".parse().unwrap(),
        capture.executor(),
    )
    .default_header(
        HeaderName::from_static("x-default"),
        HeaderValue::from_static("on-every-request"),
    )
    .build()
    .unwrap();

    capture.respond(raw_response(
        StatusCode::OK,
        serde_json::json!({ "invocationId": "inv_workflow_123" }).to_string(),
    ));

    let handle = client
        .lookup_workflow::<String>("MyWorkflow", "wf-key /\u{e9}", None)
        .await
        .unwrap();
    assert_eq!(handle.invocation_id().as_str(), "inv_workflow_123");

    // A scoped lookup adds the `scope` field to the body.
    capture.respond(raw_response(
        StatusCode::OK,
        serde_json::json!({ "invocationId": "inv_workflow_456" }).to_string(),
    ));
    let scoped = client
        .lookup_workflow::<String>("MyWorkflow", "wf-key", Some("my-scope".to_owned()))
        .await
        .unwrap();
    assert_eq!(scoped.invocation_id().as_str(), "inv_workflow_456");

    let requests = capture.take_requests();
    assert_eq!(requests.len(), 2);

    let request = &requests[0];
    assert_eq!(request.method(), Method::POST);
    assert_eq!(request.uri().path(), "/proxy/restate/lookup");
    assert_eq!(request.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(request.headers()["x-default"], "on-every-request");
    let body: serde_json::Value = serde_json::from_slice(request.body()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "target": "workflow",
            "workflowName": "MyWorkflow",
            "workflowKey": "wf-key /\u{e9}",
        })
    );

    let scoped_body: serde_json::Value = serde_json::from_slice(requests[1].body()).unwrap();
    assert_eq!(
        scoped_body,
        serde_json::json!({
            "target": "workflow",
            "workflowName": "MyWorkflow",
            "workflowKey": "wf-key",
            "scope": "my-scope",
        })
    );
}

#[tokio::test]
async fn workflow_handle_surfaces_lookup_errors() {
    let (client, capture) = client("http://example.test");

    let body = b"workflow not found";
    capture.respond(invocation_response(
        StatusCode::NOT_FOUND,
        "ignored",
        body.as_slice(),
    ));
    let error = expect_error(
        client
            .lookup_workflow::<u64>("MyWorkflow", "missing", None)
            .await,
    );
    assert!(matches!(error, ClientError::Status { .. }));
    assert_preserved_response(error, StatusCode::NOT_FOUND, body);

    let malformed = b"not-json";
    capture.respond(invocation_response(
        StatusCode::OK,
        "ignored",
        malformed.as_slice(),
    ));
    let error = expect_error(
        client
            .lookup_workflow::<u64>("MyWorkflow", "key", None)
            .await,
    );
    assert!(matches!(error, ClientError::PayloadDecode { .. }));
    assert_preserved_response(error, StatusCode::OK, malformed);
}

#[allow(dead_code)]
struct GeneratedService;

#[allow(dead_code)]
#[restate_sdk::service(name = "ConfiguredService")]
impl GeneratedService {
    #[handler(name = "renamedNew")]
    #[allow(clippy::wrong_self_convention, clippy::new_ret_no_self)]
    async fn new(&self, _context: Context<'_>, value: String) -> HandlerResult<String> {
        Ok(value)
    }

    #[handler(name = "withoutInput")]
    async fn no_input(&self, _context: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[allow(dead_code)]
struct GeneratedObject;

#[allow(dead_code)]
#[restate_sdk::object(name = "ConfiguredObject")]
impl GeneratedObject {
    #[handler(name = "readValue")]
    async fn read(&self, _context: SharedObjectContext<'_>, value: u64) -> HandlerResult<u64> {
        Ok(value)
    }

    #[handler]
    async fn write(&self, _context: ObjectContext<'_>, value: u64) -> HandlerResult<()> {
        let _ = value;
        Ok(())
    }
}

#[allow(dead_code)]
struct GeneratedWorkflow;

#[allow(dead_code)]
#[restate_sdk::workflow(name = "ConfiguredWorkflow")]
impl GeneratedWorkflow {
    #[handler]
    async fn run(&self, _context: WorkflowContext<'_>, value: String) -> HandlerResult<String> {
        Ok(value)
    }

    #[handler(name = "notifySignal")]
    async fn notify(&self, _context: SharedWorkflowContext<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

// A workflow whose handler is literally named `handle`, colliding with the injected lookup method.
#[allow(dead_code)]
struct HandleNamedWorkflow;

#[allow(dead_code)]
#[restate_sdk::workflow(name = "HandleNamedWorkflow")]
impl HandleNamedWorkflow {
    #[handler]
    async fn run(&self, _context: WorkflowContext<'_>) -> HandlerResult<()> {
        Ok(())
    }

    #[handler]
    async fn handle(
        &self,
        _context: SharedWorkflowContext<'_>,
        value: String,
    ) -> HandlerResult<String> {
        Ok(value)
    }
}

#[tokio::test]
async fn generated_clients_emit_configured_service_object_and_workflow_targets() {
    let (client, capture) = client("http://example.test");

    capture.respond(invocation_response(
        StatusCode::OK,
        "service",
        br#""reply""#.as_slice(),
    ));
    let service = GeneratedServiceIngressClient::from_client(client.clone());
    assert_eq!(
        service
            .new("request".to_owned())
            .call()
            .await
            .unwrap()
            .into_body()
            .unwrap(),
        "reply"
    );

    capture.respond(invocation_response(
        StatusCode::OK,
        "no-input",
        Bytes::new(),
    ));
    service
        .no_input()
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    capture.respond(invocation_response(
        StatusCode::OK,
        "object",
        b"7".as_slice(),
    ));
    let object = GeneratedObjectIngressClient::from_client(client.clone(), "object/key");
    assert_eq!(object.read(7).call().await.unwrap().into_body().unwrap(), 7);

    capture.respond(invocation_response(
        StatusCode::OK,
        "workflow",
        br#""done""#.as_slice(),
    ));
    let workflow = GeneratedWorkflowIngressClient::from_client(client, "workflow/key");
    assert_eq!(
        workflow
            .run("start".to_owned())
            .call()
            .await
            .unwrap()
            .into_body()
            .unwrap(),
        "done"
    );

    let requests = capture.take_requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.uri().path())
            .collect::<Vec<_>>(),
        [
            "/restate/call/ConfiguredService/renamedNew",
            "/restate/call/ConfiguredService/withoutInput",
            "/restate/call/ConfiguredObject/object%2Fkey/readValue",
            "/restate/call/ConfiguredWorkflow/workflow%2Fkey/run",
        ]
    );
    assert_eq!(requests[0].body().as_ref(), br#""request""#);
    assert!(requests[1].body().is_empty());
    assert_eq!(requests[2].body().as_ref(), b"7");
    assert_eq!(requests[3].body().as_ref(), br#""start""#);
}

#[tokio::test]
async fn generated_workflow_client_exposes_handle_lookup() {
    let (client, capture) = client("http://example.test");
    let workflow = GeneratedWorkflowIngressClient::from_client(client.clone(), "workflow/key");

    capture.respond(raw_response(
        StatusCode::OK,
        serde_json::json!({ "invocationId": "wf-inv" }).to_string(),
    ));
    let handle = workflow.handle().await.unwrap();
    assert_eq!(handle.invocation_id().as_str(), "wf-inv");

    // The handle is typed to the workflow's `run` output (String), so attach decodes a String.
    capture.respond(invocation_response(
        StatusCode::OK,
        "ignored",
        br#""done""#.as_slice(),
    ));
    assert_eq!(handle.attach().await.unwrap().into_body(), "done");

    // A scoped client forwards its scope to the lookup body.
    let scoped_workflow =
        GeneratedWorkflowIngressClient::scoped_client(client, "workflow/key", "prod");
    capture.respond(raw_response(
        StatusCode::OK,
        serde_json::json!({ "invocationId": "wf-inv-2" }).to_string(),
    ));
    let scoped_handle = scoped_workflow.handle().await.unwrap();
    assert_eq!(scoped_handle.invocation_id().as_str(), "wf-inv-2");

    let requests = capture.take_requests();
    assert_eq!(requests[0].method(), Method::POST);
    assert_eq!(requests[0].uri().path(), "/restate/lookup");
    let body: serde_json::Value = serde_json::from_slice(requests[0].body()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "target": "workflow",
            "workflowName": "ConfiguredWorkflow",
            "workflowKey": "workflow/key",
        })
    );
    assert_eq!(requests[1].method(), Method::GET);
    assert_eq!(requests[1].uri().path(), "/restate/attach/wf-inv");
    let scoped: serde_json::Value = serde_json::from_slice(requests[2].body()).unwrap();
    assert_eq!(
        scoped,
        serde_json::json!({
            "target": "workflow",
            "workflowName": "ConfiguredWorkflow",
            "workflowKey": "workflow/key",
            "scope": "prod",
        })
    );
}

#[tokio::test]
async fn scoped_client_prefixes_requests_with_its_scope() {
    let (client, capture) = client("http://example.test");

    // A scoped service client routes through the `/restate/scope/{scope}/...` prefix.
    capture.respond(invocation_response(
        StatusCode::OK,
        "svc",
        br#""ok""#.as_slice(),
    ));
    let service = GeneratedServiceIngressClient::scoped_client(client.clone(), "prod");
    service
        .new("in".to_owned())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    // Objects and workflows carry both their key and the scope.
    capture.respond(invocation_response(StatusCode::OK, "obj", b"7".as_slice()));
    let object = GeneratedObjectIngressClient::scoped_client(client.clone(), "object/key", "prod");
    object.read(7).call().await.unwrap().into_body().unwrap();

    capture.respond(invocation_response(
        StatusCode::OK,
        "wf",
        br#""done""#.as_slice(),
    ));
    let workflow = GeneratedWorkflowIngressClient::scoped_client(client, "workflow/key", "prod");
    workflow
        .run("in".to_owned())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    let requests = capture.take_requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.uri().path())
            .collect::<Vec<_>>(),
        [
            "/restate/scope/prod/call/ConfiguredService/renamedNew",
            "/restate/scope/prod/call/ConfiguredObject/object%2Fkey/readValue",
            "/restate/scope/prod/call/ConfiguredWorkflow/workflow%2Fkey/run",
        ]
    );
}

#[tokio::test]
async fn workflow_handle_collision_renames_user_method() {
    let (client, capture) = client("http://example.test");
    let workflow = HandleNamedWorkflowIngressClient::from_client(client, "k");

    // The user's `handle` handler is reachable via the renamed `_handle` method and still
    // targets the `handle` handler on the wire.
    capture.respond(invocation_response(
        StatusCode::OK,
        "ignored",
        br#""echo""#.as_slice(),
    ));
    let echoed = workflow
        ._handle("echo".to_owned())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    assert_eq!(echoed, "echo");

    // The injected lookup method occupies the plain `handle` name.
    capture.respond(raw_response(
        StatusCode::OK,
        serde_json::json!({ "invocationId": "hc-inv" }).to_string(),
    ));
    let handle = workflow.handle().await.unwrap();
    assert_eq!(handle.invocation_id().as_str(), "hc-inv");

    let requests = capture.take_requests();
    assert_eq!(
        requests[0].uri().path(),
        "/restate/call/HandleNamedWorkflow/k/handle"
    );
    assert_eq!(requests[1].uri().path(), "/restate/lookup");
}

#[tokio::test]
async fn restate_auth_token_is_a_sensitive_default_with_value_semantics() {
    let (original, capture) = client("http://example.test");
    let authenticated = original
        .clone()
        .with_restate_auth_token("cloud-token")
        .unwrap();

    capture.respond(invocation_response(
        StatusCode::OK,
        "generated-auth",
        Bytes::new(),
    ));
    GeneratedServiceIngressClient::from_client(authenticated.clone())
        .no_input()
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    capture.respond(invocation_response(
        StatusCode::OK,
        "overridden-auth",
        Bytes::new(),
    ));
    authenticated
        .request::<(), ()>(RequestTarget::service("Auth", "override"), ())
        .header(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer request-token"),
        )
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    capture.respond(invocation_response(StatusCode::OK, "ignored", Bytes::new()));
    authenticated
        .invocation_handle::<()>("auth-handle")
        .attach()
        .await
        .unwrap()
        .into_body();

    capture.respond(invocation_response(
        StatusCode::OK,
        "original-client",
        Bytes::new(),
    ));
    original
        .request::<(), ()>(RequestTarget::service("Auth", "original"), ())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();

    let requests = capture.take_requests();
    assert_eq!(requests.len(), 4);

    assert_eq!(requests[0].method(), Method::POST);
    assert_eq!(
        requests[0].uri().path(),
        "/restate/call/ConfiguredService/withoutInput"
    );
    let generated_auth = requests[0].headers().get(AUTHORIZATION).unwrap();
    assert_eq!(generated_auth.as_bytes(), b"Bearer cloud-token");
    assert!(generated_auth.is_sensitive());

    let overridden_auth = requests[1].headers().get_all(AUTHORIZATION);
    assert_eq!(overridden_auth.iter().count(), 1);
    assert_eq!(
        overridden_auth.iter().next().unwrap().as_bytes(),
        b"Bearer request-token"
    );

    assert_eq!(requests[2].method(), Method::GET);
    assert_eq!(requests[2].uri().path(), "/restate/attach/auth-handle");
    let handle_auth = requests[2].headers().get(AUTHORIZATION).unwrap();
    assert_eq!(handle_auth.as_bytes(), b"Bearer cloud-token");
    assert!(handle_auth.is_sensitive());

    assert!(!requests[3].headers().contains_key(AUTHORIZATION));
}

#[test]
fn rejects_invalid_restate_auth_tokens_without_executing_requests() {
    let (client, capture) = client("http://example.test");

    for token in ["token\rwith-carriage-return", "token\nwith-newline"] {
        match client.clone().with_restate_auth_token(token) {
            Err(ClientBuildError::InvalidAuthToken { .. }) => {}
            Err(error) => panic!("invalid auth token returned the wrong error: {error:?}"),
            Ok(_) => panic!("invalid auth token was unexpectedly accepted"),
        }
    }

    assert_eq!(capture.request_count(), 0);
}

#[cfg(feature = "reqwest-client")]
#[tokio::test]
async fn reqwest_alias_executes_and_buffers_an_http_exchange() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = vec![0; 4096];
        let count = stream.read(&mut bytes).await.unwrap();
        let request = String::from_utf8_lossy(&bytes[..count]);
        assert!(request.starts_with("POST /restate/call/Reqwest/ping HTTP/1.1\r\n"));
        assert!(request.ends_with("\r\n\r\n"));

        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nx-restate-id: reqwest-id\r\n\
                  content-length: 4\r\nconnection: close\r\n\r\n\"ok\"",
            )
            .await
            .unwrap();
    });

    let base_uri: http::Uri = format!("http://{address}").parse().unwrap();
    let client = restate_sdk::ingress::ReqwestClient::connect(base_uri.clone()).unwrap();
    let body = client
        .request::<(), String>(RequestTarget::service("Reqwest", "ping"), ())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    assert_eq!(body, "ok");
    server.await.unwrap();

    let custom_reqwest = reqwest::Client::builder().build().unwrap();
    let _: restate_sdk::ingress::ReqwestClient =
        restate_sdk::ingress::ReqwestClient::new(base_uri, custom_reqwest).unwrap();
}
