//! One role-reversed HTTP/2 tunnel connection.
//!
//! The tunnel server is the HTTP/2 client on an already-connected socket. This
//! module owns the corresponding Hyper server connection, the handshake body,
//! every per-stream executor task, and all drain timers for one attempt.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::AUTHORIZATION;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body::{Body, Frame, SizeHint};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::rt::Executor;
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use restate_sdk_shared_core::IdentityVerifier;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::TunnelInfo;
use super::draining::{DrainState, InFlightPermit};
use crate::endpoint::Endpoint;

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

const START_TUNNEL_PATH: &str = "/_/start-tunnel";
const HEALTH_PATH: &str = "/_/health";
const DRAIN_TUNNEL_PATH: &str = "/_/drain-tunnel";
const TUNNEL_DRAINING_HEADER: &str = "x-restate-tunnel-draining";

struct ConnectionSettings {
    first_request_timeout: Duration,
    handshake_trailers_timeout: Duration,
    keep_alive_interval: Duration,
    keep_alive_timeout: Duration,
    max_concurrent_streams: u32,
    initial_stream_window_size: u32,
    initial_connection_window_size: u32,
    max_frame_size: u32,
}

/// The single source of truth for per-connection protocol settings.
const CONNECTION_SETTINGS: ConnectionSettings = ConnectionSettings {
    first_request_timeout: Duration::from_secs(5),
    handshake_trailers_timeout: Duration::from_secs(5),
    keep_alive_interval: Duration::from_secs(75),
    keep_alive_timeout: Duration::from_secs(10),
    max_concurrent_streams: 4_096,
    initial_stream_window_size: 1024 * 1024,
    initial_connection_window_size: 16 * 1024 * 1024,
    max_frame_size: 65_536,
};

/// Tokio transport accepted by a connection attempt.
pub(crate) trait TunnelIo: AsyncRead + AsyncWrite {}

impl<T> TunnelIo for T where T: AsyncRead + AsyncWrite + ?Sized {}

pub(crate) type BoxIo = Box<dyn TunnelIo + Unpin + Send>;

pub(crate) struct AttemptContext {
    pub(crate) attempt_id: u64,
    pub(crate) endpoint: Endpoint,
    pub(crate) identity_verifier: Arc<IdentityVerifier>,
    pub(crate) authorization: HeaderValue,
    pub(crate) environment_id: String,
    pub(crate) tunnel_name: String,
    pub(crate) tunnel_worker_id: String,
    pub(crate) tunnel_connection_id: String,
    pub(crate) target: String,
    pub(crate) drain: Arc<DrainState>,
    pub(crate) graceful: CancellationToken,
    pub(crate) force: CancellationToken,
    pub(crate) slot_cancel: CancellationToken,
    pub(crate) events: mpsc::Sender<AttemptEvent>,
    pub(crate) server_drain_grace: Duration,
}

#[derive(Debug)]
pub(crate) enum AttemptEvent {
    Established { attempt_id: u64, info: TunnelInfo },
    ServerDrain { attempt_id: u64, uptime: Duration },
}

#[derive(Debug)]
pub(crate) enum AttemptOutcome {
    Retryable(String),
    Fatal(String),
    Served { uptime: Duration },
    Cancelled,
}

#[derive(Clone, Copy, Debug)]
enum StreamGate {
    Waiting,
    Open,
    Closed,
}

struct ServiceState {
    endpoint: Endpoint,
    identity_verifier: Arc<IdentityVerifier>,
    drain: Arc<DrainState>,
    credentials: HeaderMap,
    handshake_started: AtomicBool,
    handshake_body: mpsc::Sender<Incoming>,
    gate: watch::Sender<StreamGate>,
    server_drain: watch::Sender<bool>,
}

/// Executor whose join handles remain owned by the attempt.
///
/// Hyper needs concurrent stream futures for HTTP/2. Tokio's stock executor
/// detaches those futures; retaining the handles here lets `run_attempt` abort
/// and reap every stream before it returns.
#[derive(Default)]
struct TaskRegistry {
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for TaskRegistry {
    fn drop(&mut self) {
        // `run_attempt` itself lives in a supervisor JoinSet and can therefore
        // be aborted. Abort synchronously here as the final safety net; merely
        // dropping JoinHandles would detach the Hyper stream tasks.
        let tasks = match self.tasks.get_mut() {
            Ok(tasks) => tasks,
            Err(poisoned) => poisoned.into_inner(),
        };
        for task in tasks {
            task.abort();
        }
    }
}

#[derive(Clone, Default)]
struct AttemptExecutor {
    registry: Arc<TaskRegistry>,
}

impl<F> Executor<F> for AttemptExecutor
where
    F: Future<Output = ()> + Send + 'static,
{
    fn execute(&self, future: F) {
        let task = tokio::spawn(future);
        match self.registry.tasks.lock() {
            Ok(mut tasks) => {
                // Completed handles no longer own live work. Pruning on each
                // new stream bounds this registry by recent/concurrent work
                // instead of the lifetime request count of an H2 session.
                tasks.retain(|task| !task.is_finished());
                tasks.push(task);
            }
            Err(_) => task.abort(),
        }
    }
}

impl AttemptExecutor {
    async fn abort_and_reap(&self) {
        let tasks = match self.registry.tasks.lock() {
            Ok(mut tasks) => std::mem::take(&mut *tasks),
            Err(mut poisoned) => std::mem::take(&mut **poisoned.get_mut()),
        };
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// Run one attempt over an already-dialed plaintext or TLS transport.
pub(crate) async fn run_attempt(io: BoxIo, context: AttemptContext) -> AttemptOutcome {
    let executor = AttemptExecutor::default();
    let outcome = run_attempt_inner(io, context, executor.clone()).await;
    executor.abort_and_reap().await;
    outcome
}

async fn run_attempt_inner(
    io: BoxIo,
    context: AttemptContext,
    executor: AttemptExecutor,
) -> AttemptOutcome {
    let credentials = match credential_headers(&context) {
        Ok(headers) => headers,
        Err(reason) => return AttemptOutcome::Fatal(reason),
    };

    let (handshake_body_tx, mut handshake_body_rx) = mpsc::channel(1);
    let (gate, _) = watch::channel(StreamGate::Waiting);
    let (server_drain, mut server_drain_rx) = watch::channel(false);
    let state = Arc::new(ServiceState {
        endpoint: context.endpoint.clone(),
        identity_verifier: Arc::clone(&context.identity_verifier),
        drain: Arc::clone(&context.drain),
        credentials,
        handshake_started: AtomicBool::new(false),
        handshake_body: handshake_body_tx,
        gate: gate.clone(),
        server_drain,
    });

    let service = service_fn({
        let state = Arc::clone(&state);
        move |request| handle_request(Arc::clone(&state), request)
    });

    let mut builder = http2::Builder::new(executor);
    builder
        .timer(TokioTimer::new())
        .max_concurrent_streams(CONNECTION_SETTINGS.max_concurrent_streams)
        .initial_stream_window_size(CONNECTION_SETTINGS.initial_stream_window_size)
        .initial_connection_window_size(CONNECTION_SETTINGS.initial_connection_window_size)
        .max_frame_size(CONNECTION_SETTINGS.max_frame_size)
        .keep_alive_interval(Some(CONNECTION_SETTINGS.keep_alive_interval))
        .keep_alive_timeout(CONNECTION_SETTINGS.keep_alive_timeout);
    // Hyper's server implementation hard-codes keepalive-while-idle when an
    // interval is configured, and closes after a single missed ACK. We use
    // those tested single-miss semantics rather than exposing an unused
    // "max missed" setting.
    let connection = builder.serve_connection(TokioIo::new(io), service);
    tokio::pin!(connection);

    let first_request = tokio::time::sleep(CONNECTION_SETTINGS.first_request_timeout);
    tokio::pin!(first_request);
    let body = tokio::select! {
        biased;
        _ = context.force.cancelled() => return close_gate(&gate, AttemptOutcome::Cancelled),
        _ = context.slot_cancel.cancelled() => return close_gate(&gate, AttemptOutcome::Cancelled),
        _ = context.graceful.cancelled() => {
            gate.send_replace(StreamGate::Closed);
            connection.as_mut().graceful_shutdown();
            return finish_gracefully(&mut connection, &context).await;
        }
        result = &mut connection => {
            return close_gate(&gate, before_ready_connection_outcome(result, &context.target));
        }
        body = handshake_body_rx.recv() => match body {
            Some(body) => body,
            None => return close_gate(&gate, AttemptOutcome::Retryable("handshake request channel closed".into())),
        },
        _ = &mut first_request => {
            return close_gate(&gate, AttemptOutcome::Retryable("server did not open GET /_/start-tunnel within 5s".into()));
        }
    };

    let handshake = read_handshake(body, &context.tunnel_name);
    tokio::pin!(handshake);
    let info = tokio::select! {
        biased;
        _ = context.force.cancelled() => return close_gate(&gate, AttemptOutcome::Cancelled),
        _ = context.slot_cancel.cancelled() => return close_gate(&gate, AttemptOutcome::Cancelled),
        _ = context.graceful.cancelled() => {
            gate.send_replace(StreamGate::Closed);
            connection.as_mut().graceful_shutdown();
            return finish_gracefully(&mut connection, &context).await;
        }
        result = &mut connection => {
            return close_gate(&gate, before_ready_connection_outcome(result, &context.target));
        }
        result = &mut handshake => match result {
            Ok(info) => info,
            Err(HandshakeError::Fatal(reason)) => {
                return close_gate(&gate, AttemptOutcome::Fatal(reason));
            }
            Err(HandshakeError::Retryable(reason)) => {
                return close_gate(&gate, AttemptOutcome::Retryable(reason));
            }
        }
    };

    if context.force.is_cancelled()
        || context.slot_cancel.is_cancelled()
        || context.graceful.is_cancelled()
    {
        return close_gate(&gate, AttemptOutcome::Cancelled);
    }

    let established_at = Instant::now();
    gate.send_replace(StreamGate::Open);
    if context
        .events
        .send(AttemptEvent::Established {
            attempt_id: context.attempt_id,
            info,
        })
        .await
        .is_err()
    {
        return close_gate(&gate, AttemptOutcome::Cancelled);
    }

    loop {
        if *server_drain_rx.borrow_and_update() {
            break;
        }
        tokio::select! {
            biased;
            _ = context.force.cancelled() => return AttemptOutcome::Cancelled,
            _ = context.slot_cancel.cancelled() => return AttemptOutcome::Cancelled,
            _ = context.graceful.cancelled() => {
                connection.as_mut().graceful_shutdown();
                return finish_gracefully(&mut connection, &context).await;
            }
            result = &mut connection => {
                log_connection_error(result, &context.target);
                return AttemptOutcome::Served { uptime: established_at.elapsed() };
            }
            changed = server_drain_rx.changed() => {
                if changed.is_err() {
                    return AttemptOutcome::Served { uptime: established_at.elapsed() };
                }
            }
        }
    }

    let uptime = established_at.elapsed();
    if context
        .events
        .send(AttemptEvent::ServerDrain {
            attempt_id: context.attempt_id,
            uptime,
        })
        .await
        .is_err()
    {
        return AttemptOutcome::Cancelled;
    }

    let server_grace = tokio::time::sleep(context.server_drain_grace);
    tokio::pin!(server_grace);
    tokio::select! {
        biased;
        _ = context.force.cancelled() => AttemptOutcome::Cancelled,
        _ = context.slot_cancel.cancelled() => AttemptOutcome::Cancelled,
        _ = context.graceful.cancelled() => {
            connection.as_mut().graceful_shutdown();
            finish_gracefully(&mut connection, &context).await
        }
        result = &mut connection => {
            log_connection_error(result, &context.target);
            AttemptOutcome::Served { uptime: established_at.elapsed() }
        }
        _ = &mut server_grace => AttemptOutcome::Served { uptime: established_at.elapsed() },
    }
}

fn close_gate(gate: &watch::Sender<StreamGate>, outcome: AttemptOutcome) -> AttemptOutcome {
    gate.send_replace(StreamGate::Closed);
    outcome
}

async fn finish_gracefully<I, S, E, B>(
    connection: &mut Pin<&mut http2::Connection<I, S, E>>,
    context: &AttemptContext,
) -> AttemptOutcome
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin,
    S: hyper::service::Service<Request<Incoming>, Response = Response<B>>,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: 'static,
    B: Body + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    E: hyper::rt::bounds::Http2ServerConnExec<S::Future, B>,
{
    tokio::select! {
        biased;
        _ = context.force.cancelled() => AttemptOutcome::Cancelled,
        _ = context.slot_cancel.cancelled() => AttemptOutcome::Cancelled,
        _ = connection.as_mut() => AttemptOutcome::Cancelled,
    }
}

fn before_ready_connection_outcome(
    result: Result<(), hyper::Error>,
    target: &str,
) -> AttemptOutcome {
    match result {
        Ok(()) => AttemptOutcome::Retryable(format!(
            "HTTP/2 connection to {target} closed before the handshake completed"
        )),
        Err(error) => AttemptOutcome::Retryable(format!(
            "HTTP/2 connection to {target} failed before the handshake completed: {error}"
        )),
    }
}

fn log_connection_error(result: Result<(), hyper::Error>, target: &str) {
    if let Err(error) = result {
        tracing::debug!(%error, %target, "Tunnel HTTP/2 connection ended");
    }
}

fn credential_headers(context: &AttemptContext) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, context.authorization.clone());
    insert_context_header(&mut headers, "environment-id", &context.environment_id)?;
    insert_context_header(&mut headers, "tunnel-name", &context.tunnel_name)?;
    insert_context_header(&mut headers, "tunnel-worker-id", &context.tunnel_worker_id)?;
    insert_context_header(
        &mut headers,
        "tunnel-connection-id",
        &context.tunnel_connection_id,
    )?;
    headers.insert("supports-drain", HeaderValue::from_static("true"));
    headers.insert("supports-client-drain", HeaderValue::from_static("true"));
    Ok(headers)
}

fn insert_context_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), String> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| format!("invalid value for tunnel handshake header {name}"))?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

async fn handle_request(
    state: Arc<ServiceState>,
    request: Request<Incoming>,
) -> Result<Response<TunnelBody>, Infallible> {
    let raw_path = request
        .uri()
        .path_and_query()
        .map_or("", |path| path.as_str());

    if request.method() == Method::GET && raw_path == START_TUNNEL_PATH {
        return Ok(handle_start(state, request));
    }
    if request.uri().path() == HEALTH_PATH {
        return Ok(simple_response(StatusCode::OK, Bytes::new()));
    }
    if request.uri().path() == DRAIN_TUNNEL_PATH {
        if state.handshake_started.load(Ordering::Acquire) {
            state.server_drain.send_replace(true);
        }
        return Ok(simple_response(StatusCode::OK, Bytes::new()));
    }

    if state.drain.is_draining() {
        return Ok(draining_response());
    }

    let mut gate = state.gate.subscribe();
    loop {
        match *gate.borrow_and_update() {
            StreamGate::Waiting => {}
            StreamGate::Open => return Ok(dispatch_forwarded(&state, request)),
            StreamGate::Closed => return Ok(draining_response()),
        }
        if gate.changed().await.is_err() {
            return Ok(draining_response());
        }
    }
}

fn handle_start(state: Arc<ServiceState>, request: Request<Incoming>) -> Response<TunnelBody> {
    if state
        .handshake_started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return draining_response();
    }

    if state.handshake_body.try_send(request.into_body()).is_err() {
        state.gate.send_replace(StreamGate::Closed);
        return draining_response();
    }

    let mut response = simple_response(StatusCode::OK, Bytes::new());
    *response.headers_mut() = state.credentials.clone();
    response
}

fn dispatch_forwarded(
    state: &Arc<ServiceState>,
    mut request: Request<Incoming>,
) -> Response<TunnelBody> {
    let rewritten = match rewrite_forwarded_uri(request.uri()) {
        Ok(uri) => uri,
        Err(()) => {
            return simple_response(
                StatusCode::BAD_REQUEST,
                Bytes::from_static(b"tunnel: malformed forwarded path"),
            );
        }
    };

    let Some(permit) = state.drain.try_start() else {
        return draining_response();
    };
    *request.uri_mut() = rewritten;
    let response = state
        .endpoint
        .handle_tunnel(request, state.identity_verifier.as_ref());
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, TunnelBody::tracked(body.boxed_unsync(), permit))
}

fn rewrite_forwarded_uri(uri: &Uri) -> Result<Uri, ()> {
    let raw = uri
        .path_and_query()
        .map_or(uri.path(), |value| value.as_str());
    let (path, query) = match raw.find('?') {
        Some(index) => (&raw[..index], &raw[index..]),
        None => (raw, ""),
    };
    let mut segments = path.split('/');
    if segments.next() != Some("") {
        return Err(());
    }
    let scheme = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or(())?;
    let host = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or(())?;
    let port = segments
        .next()
        .filter(|segment| !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or(())?;
    let _ = (scheme, host, port);
    let tail = format!("/{}{}", segments.collect::<Vec<_>>().join("/"), query);
    tail.parse().map_err(|_| ())
}

fn draining_response() -> Response<TunnelBody> {
    let mut response = simple_response(StatusCode::SERVICE_UNAVAILABLE, Bytes::new());
    response
        .headers_mut()
        .insert(TUNNEL_DRAINING_HEADER, HeaderValue::from_static("true"));
    response
}

fn simple_response(status: StatusCode, bytes: Bytes) -> Response<TunnelBody> {
    let mut response = Response::new(TunnelBody::plain(
        Full::new(bytes)
            .map_err(|never| match never {})
            .boxed_unsync(),
    ));
    *response.status_mut() = status;
    response
}

struct TunnelBody {
    inner: UnsyncBoxBody<Bytes, BoxError>,
    permit: Option<InFlightPermit>,
}

impl TunnelBody {
    fn plain(inner: UnsyncBoxBody<Bytes, BoxError>) -> Self {
        Self {
            inner,
            permit: None,
        }
    }

    fn tracked(inner: UnsyncBoxBody<Bytes, BoxError>, permit: InFlightPermit) -> Self {
        Self {
            inner,
            permit: Some(permit),
        }
    }
}

impl Body for TunnelBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_frame(cx);
        if matches!(result, Poll::Ready(None)) {
            this.permit.take();
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[derive(Debug)]
enum HandshakeError {
    Fatal(String),
    Retryable(String),
}

async fn read_handshake(
    mut body: Incoming,
    requested_tunnel_name: &str,
) -> Result<TunnelInfo, HandshakeError> {
    let frame = tokio::time::timeout(CONNECTION_SETTINGS.handshake_trailers_timeout, body.frame())
        .await
        .map_err(|_| {
            HandshakeError::Retryable("handshake trailers not received within 5s".into())
        })?;
    let frame = match frame {
        Some(Ok(frame)) => frame,
        Some(Err(error)) => {
            return Err(HandshakeError::Retryable(format!(
                "handshake stream error: {error}"
            )));
        }
        None => {
            return Err(HandshakeError::Retryable(
                "handshake stream closed before trailers".into(),
            ));
        }
    };
    if frame.is_data() {
        return Err(HandshakeError::Retryable(
            "handshake stream contained unexpected DATA".into(),
        ));
    }
    if frame.is_trailers() {
        let trailers = frame
            .into_trailers()
            .expect("frame reported trailers immediately before conversion");
        return parse_handshake_trailers(&trailers, requested_tunnel_name);
    }
    Err(HandshakeError::Retryable(
        "handshake stream contained an unexpected frame".into(),
    ))
}

fn parse_handshake_trailers(
    trailers: &HeaderMap,
    requested_tunnel_name: &str,
) -> Result<TunnelInfo, HandshakeError> {
    let status = required_single_header(trailers, "tunnel-status")
        .map_err(|reason| HandshakeError::Retryable(reason.into()))?;
    match status {
        "unauthorized" | "bad-tunnel-name" => {
            return Err(HandshakeError::Fatal(format!("tunnel-status: {status}")));
        }
        "ok" => {}
        other => {
            return Err(HandshakeError::Retryable(format!("tunnel-status: {other}")));
        }
    }

    let tunnel_name = required_single_header(trailers, "tunnel-name")
        .map_err(|reason| HandshakeError::Retryable(reason.into()))?;
    let proxy_url = required_single_header(trailers, "proxy-url")
        .map_err(|reason| HandshakeError::Retryable(reason.into()))?;
    let tunnel_url = required_single_header(trailers, "tunnel-url")
        .map_err(|reason| HandshakeError::Retryable(reason.into()))?;
    if tunnel_name != requested_tunnel_name {
        return Err(HandshakeError::Fatal(format!(
            "tunnel-name mismatch: requested {requested_tunnel_name:?}, got {tunnel_name:?}"
        )));
    }
    TunnelInfo::from_handshake(
        tunnel_name.to_owned(),
        proxy_url.to_owned(),
        tunnel_url.to_owned(),
    )
    .map_err(|reason| HandshakeError::Retryable(format!("handshake metadata: {reason}")))
}

fn required_single_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<&'a str, &'static str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or("required handshake trailer missing")?;
    if values.next().is_some() {
        return Err("duplicate handshake trailer");
    }
    let value = value.to_str().map_err(|_| "malformed handshake trailer")?;
    if value.is_empty() {
        return Err("empty handshake trailer");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_uri_preserves_raw_tail_and_query() {
        let uri: Uri = "/http/service.example/9080/invoke/My%20Service/run?x=%2F%3F"
            .parse()
            .unwrap();
        assert_eq!(
            rewrite_forwarded_uri(&uri).unwrap().to_string(),
            "/invoke/My%20Service/run?x=%2F%3F"
        );

        let empty_tail: Uri = "/HTTP/Service.Example/9080?raw=%2f".parse().unwrap();
        assert_eq!(
            rewrite_forwarded_uri(&empty_tail).unwrap().to_string(),
            "/?raw=%2f"
        );
    }

    #[test]
    fn forwarded_uri_requires_numeric_port_and_complete_prefix() {
        for uri in [
            "/invoke/Service/handler",
            "/discover",
            "//host/9080/discover",
            "/http/host/not-a-port/discover",
            "/http//9080/discover",
            "/http/host",
        ] {
            assert!(
                rewrite_forwarded_uri(&uri.parse().unwrap()).is_err(),
                "{uri}"
            );
        }
    }

    #[test]
    fn handshake_status_taxonomy_and_name_check() {
        let trailers = |status: &'static str, name: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert("tunnel-status", HeaderValue::from_static(status));
            headers.insert("tunnel-name", HeaderValue::from_static(name));
            headers.insert(
                "proxy-url",
                HeaderValue::from_static("https://tunnel.example/env/requested"),
            );
            headers.insert(
                "tunnel-url",
                HeaderValue::from_static("https://tunnel.example"),
            );
            headers
        };

        assert!(matches!(
            parse_handshake_trailers(&trailers("unauthorized", "requested"), "requested"),
            Err(HandshakeError::Fatal(_))
        ));
        assert!(matches!(
            parse_handshake_trailers(&trailers("bad-tunnel-name", "requested"), "requested"),
            Err(HandshakeError::Fatal(_))
        ));
        assert!(matches!(
            parse_handshake_trailers(&trailers("too-many-tunnels", "requested"), "requested"),
            Err(HandshakeError::Retryable(_))
        ));
        assert!(matches!(
            parse_handshake_trailers(&trailers("future-status", "requested"), "requested"),
            Err(HandshakeError::Retryable(_))
        ));
        assert!(matches!(
            parse_handshake_trailers(&trailers("ok", "other"), "requested"),
            Err(HandshakeError::Fatal(_))
        ));
        assert!(parse_handshake_trailers(&trailers("ok", "requested"), "requested").is_ok());

        let mut missing_status = trailers("ok", "requested");
        missing_status.remove("tunnel-status");
        assert!(matches!(
            parse_handshake_trailers(&missing_status, "requested"),
            Err(HandshakeError::Retryable(_))
        ));

        let mut duplicate_status = trailers("ok", "requested");
        duplicate_status.append("tunnel-status", HeaderValue::from_static("ok"));
        assert!(matches!(
            parse_handshake_trailers(&duplicate_status, "requested"),
            Err(HandshakeError::Retryable(_))
        ));
    }
}
