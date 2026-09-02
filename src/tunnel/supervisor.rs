use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use futures::FutureExt;
use rustls::pki_types::ServerName;
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, sleep, sleep_until, timeout_at};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

use super::connection::{AttemptContext, AttemptEvent, AttemptOutcome, BoxIo, run_attempt};
use super::draining::DrainState;
use super::options::ResolvedOptions;
use super::targets::{Discovery, Target, TargetResolver};
use super::{Error, TunnelInfo};
use crate::endpoint::Endpoint;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TCP_KEEPALIVE: Duration = Duration::from_secs(75);
const DNS_RETRY_FALLBACK: Duration = Duration::from_secs(30);
const RECONNECT_INITIAL: Duration = Duration::from_millis(10);
const RECONNECT_MAX: Duration = Duration::from_secs(120);
const STABLE_CONNECTION: Duration = Duration::from_secs(5);
const SERVER_DRAIN_GRACE: Duration = Duration::from_secs(120);

#[derive(Clone)]
enum EngineStatus {
    Starting,
    Ready(TunnelInfo),
    Terminal {
        result: Result<(), Arc<str>>,
        /// Retain the first successful handshake even if termination races
        /// the waiter that is observing readiness.
        ready: Option<TunnelInfo>,
    },
}

/// Owns the resolver loop and, transitively through its `JoinSet`s, every
/// slot, attempt, socket, handshake reader, and detached draining session.
pub(crate) struct Engine {
    status: watch::Sender<EngineStatus>,
    graceful: CancellationToken,
    force: CancellationToken,
    drain: Arc<DrainState>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Engine {
    pub(crate) fn start(endpoint: Endpoint, options: ResolvedOptions) -> Result<Self, String> {
        let resolver = TargetResolver::system().map_err(|error| error.to_string())?;
        let tls = match &options.discovery {
            Discovery::Explicit(targets) if targets.iter().all(Target::is_plaintext) => None,
            _ => Some(build_tls_connector()?),
        };
        let graceful = CancellationToken::new();
        let force = CancellationToken::new();
        let drain = DrainState::new();
        let (status, _) = watch::channel(EngineStatus::Starting);

        let task = tokio::spawn({
            let status = status.clone();
            let graceful = graceful.clone();
            let force = force.clone();
            let drain = Arc::clone(&drain);
            async move {
                let result = std::panic::AssertUnwindSafe(supervise(
                    endpoint,
                    Arc::new(options),
                    resolver,
                    tls,
                    status.clone(),
                    graceful,
                    force,
                    drain,
                ))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err("tunnel engine panicked".to_owned()));
                let ready = match status.borrow().clone() {
                    EngineStatus::Ready(info) => Some(info),
                    EngineStatus::Terminal { ready, .. } => ready,
                    EngineStatus::Starting => None,
                };
                status.send_replace(EngineStatus::Terminal {
                    result: result.map_err(Arc::<str>::from),
                    ready,
                });
            }
        });

        Ok(Self {
            status,
            graceful,
            force,
            drain,
            task: Mutex::new(Some(task)),
        })
    }

    pub(crate) async fn wait_ready(&self) -> Result<TunnelInfo, Error> {
        let mut status = self.status.subscribe();
        loop {
            match status.borrow_and_update().clone() {
                EngineStatus::Starting => {}
                EngineStatus::Ready(info) => return Ok(info),
                EngineStatus::Terminal {
                    ready: Some(info), ..
                } => return Ok(info),
                EngineStatus::Terminal { result: Ok(()), .. } => {
                    return Err(Error::closed_before_ready());
                }
                EngineStatus::Terminal {
                    result: Err(reason),
                    ..
                } => {
                    return Err(Error::fatal(reason.to_string()));
                }
            }
            if status.changed().await.is_err() {
                return Err(Error::closed_before_ready());
            }
        }
    }

    pub(crate) async fn wait_terminal(&self) -> Result<(), Error> {
        let mut status = self.status.subscribe();
        loop {
            if let EngineStatus::Terminal { result, .. } = status.borrow_and_update().clone() {
                return result.map_err(|reason| Error::fatal(reason.to_string()));
            }
            if status.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    /// Atomically refuse raced requests before waking the supervisor and
    /// connection tasks. This is intentionally synchronous.
    pub(crate) fn begin_shutdown(&self) {
        self.drain.begin();
        self.graceful.cancel();
    }

    /// Force tokens are observed by every live task and socket owner.
    pub(crate) fn force_close(&self) {
        self.drain.begin();
        self.force.cancel();
    }

    pub(crate) fn abort(&self) {
        self.force_close();
        if let Some(task) = self
            .task
            .lock()
            .expect("engine task lock poisoned")
            .as_ref()
        {
            task.abort();
        }
    }

    pub(crate) async fn reap(&self) {
        let task = self.task.lock().expect("engine task lock poisoned").take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.abort();
    }
}

struct Slot {
    generation: u64,
    cancel: CancellationToken,
}

enum SlotResult {
    Stopped,
    Fatal(String),
}

#[allow(clippy::too_many_arguments)]
async fn supervise(
    endpoint: Endpoint,
    options: Arc<ResolvedOptions>,
    resolver: TargetResolver,
    tls: Option<TlsConnector>,
    status: watch::Sender<EngineStatus>,
    graceful: CancellationToken,
    force: CancellationToken,
    drain: Arc<DrainState>,
) -> Result<(), String> {
    let mut slots = HashMap::<Target, Slot>::new();
    let mut tasks = JoinSet::<(Target, u64, SlotResult)>::new();
    let mut generation = 0_u64;
    let mut refresh_at = Instant::now();
    let uses_dns = matches!(options.discovery, Discovery::Srv(_));

    loop {
        if force.is_cancelled() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            return Ok(());
        }
        if graceful.is_cancelled() {
            while !tasks.is_empty() {
                tokio::select! {
                    _ = force.cancelled() => {
                        tasks.abort_all();
                        while tasks.join_next().await.is_some() {}
                        return Ok(());
                    }
                    joined = tasks.join_next() => {
                        if joined.is_none() {
                            break;
                        }
                    }
                }
            }
            tokio::select! {
                _ = force.cancelled() => return Ok(()),
                _ = drain.wait_empty() => return Ok(()),
            }
        }

        if refresh_at <= Instant::now() {
            let resolution = tokio::select! {
                _ = force.cancelled() => continue,
                _ = graceful.cancelled() => continue,
                result = resolver.resolve(&options.discovery) => result,
            };

            match resolution {
                Ok(resolution) => {
                    let desired = resolution
                        .targets
                        .into_iter()
                        .collect::<std::collections::HashSet<_>>();
                    for target in &desired {
                        if slots.contains_key(target) {
                            continue;
                        }
                        generation = generation.wrapping_add(1);
                        let cancel = CancellationToken::new();
                        tasks.spawn({
                            let target = target.clone();
                            let endpoint = endpoint.clone();
                            let options = Arc::clone(&options);
                            let resolver = resolver.clone();
                            let tls = tls.clone();
                            let status = status.clone();
                            let graceful = graceful.clone();
                            let force = force.clone();
                            let drain = Arc::clone(&drain);
                            let cancel_for_task = cancel.clone();
                            let slot_generation = generation;
                            async move {
                                let result = run_slot(
                                    target.clone(),
                                    endpoint,
                                    options,
                                    resolver,
                                    tls,
                                    status,
                                    graceful,
                                    force,
                                    cancel_for_task,
                                    drain,
                                )
                                .await;
                                (target, slot_generation, result)
                            }
                        });
                        slots.insert(target.clone(), Slot { generation, cancel });
                    }
                    slots.retain(|target, slot| {
                        if desired.contains(target) {
                            true
                        } else {
                            slot.cancel.cancel();
                            false
                        }
                    });
                    refresh_at = match resolution.refresh_after {
                        Some(after) => Instant::now() + after,
                        None => Instant::now() + Duration::from_secs(365 * 24 * 60 * 60),
                    };
                }
                Err(error) => {
                    tracing::warn!(%error, "Tunnel target resolution failed; retaining healthy connections");
                    refresh_at = Instant::now() + DNS_RETRY_FALLBACK;
                }
            }
        }

        tokio::select! {
            _ = force.cancelled() => {},
            _ = graceful.cancelled() => {},
            _ = sleep_until(refresh_at), if uses_dns => {},
            joined = tasks.join_next(), if !tasks.is_empty() => {
                match joined {
                    Some(Ok((target, joined_generation, SlotResult::Stopped))) => {
                        if slots.get(&target).is_some_and(|slot| slot.generation == joined_generation) {
                            slots.remove(&target);
                            // A fixed slot should not vanish by itself; allow the next
                            // loop to recreate it from the retained discovery set.
                            if !uses_dns {
                                refresh_at = Instant::now();
                            }
                        }
                    }
                    Some(Ok((_, _, SlotResult::Fatal(reason)))) => {
                        force.cancel();
                        tasks.abort_all();
                        while tasks.join_next().await.is_some() {}
                        return Err(reason);
                    }
                    Some(Err(join_error)) if join_error.is_panic() => {
                        force.cancel();
                        tasks.abort_all();
                        while tasks.join_next().await.is_some() {}
                        return Err(format!("tunnel slot panicked: {join_error}"));
                    }
                    _ => {}
                }
            }
            _ = sleep(Duration::from_millis(10)), if !uses_dns && tasks.is_empty() => {
                refresh_at = Instant::now();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_slot(
    target: Target,
    endpoint: Endpoint,
    options: Arc<ResolvedOptions>,
    resolver: TargetResolver,
    tls: Option<TlsConnector>,
    status: watch::Sender<EngineStatus>,
    graceful: CancellationToken,
    force: CancellationToken,
    slot_cancel: CancellationToken,
    drain: Arc<DrainState>,
) -> SlotResult {
    let mut attempts = JoinSet::<(u64, AttemptOutcome)>::new();
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(32);
    let mut next_attempt_id = 1_u64;
    let mut current = None;
    let mut backoff = Backoff::new();
    let mut next_dial = Instant::now();
    let mut stopping_gracefully = false;

    loop {
        if force.is_cancelled() || slot_cancel.is_cancelled() {
            attempts.abort_all();
            while attempts.join_next().await.is_some() {}
            return SlotResult::Stopped;
        }
        if graceful.is_cancelled() {
            stopping_gracefully = true;
        }
        if stopping_gracefully && attempts.is_empty() {
            return SlotResult::Stopped;
        }

        if !stopping_gracefully && current.is_none() && next_dial <= Instant::now() {
            let attempt_id = next_attempt_id;
            next_attempt_id = next_attempt_id.wrapping_add(1);
            current = Some(attempt_id);
            attempts.spawn({
                let target = target.clone();
                let endpoint = endpoint.clone();
                let options = Arc::clone(&options);
                let resolver = resolver.clone();
                let tls = tls.clone();
                let events = events_tx.clone();
                let graceful = graceful.clone();
                let force = force.clone();
                let slot_cancel = slot_cancel.clone();
                let drain = Arc::clone(&drain);
                async move {
                    let outcome = run_one_attempt(
                        attempt_id,
                        target,
                        endpoint,
                        options,
                        resolver,
                        tls,
                        events,
                        graceful,
                        force,
                        slot_cancel,
                        drain,
                    )
                    .await;
                    (attempt_id, outcome)
                }
            });
        }

        tokio::select! {
            _ = force.cancelled() => {},
            _ = slot_cancel.cancelled() => {},
            _ = graceful.cancelled(), if !stopping_gracefully => {
                stopping_gracefully = true;
            },
            _ = sleep_until(next_dial), if !stopping_gracefully && current.is_none() => {},
            Some(event) = events_rx.recv() => {
                match event {
                    AttemptEvent::Established { attempt_id, info } => {
                        if !graceful.is_cancelled() && !force.is_cancelled() {
                            status.send_if_modified(|state| {
                                if matches!(state, EngineStatus::Starting) {
                                    *state = EngineStatus::Ready(info);
                                    true
                                } else {
                                    false
                                }
                            });
                        }
                        tracing::info!(attempt_id, target = %target, "Tunnel connection established");
                    }
                    AttemptEvent::ServerDrain { attempt_id, uptime } => {
                        if current == Some(attempt_id) {
                            current = None;
                        }
                        if uptime >= STABLE_CONNECTION {
                            backoff.reset();
                        }
                        // A server drain is a handover request, not a failed
                        // connection. Keep this attempt serving while its
                        // replacement starts without a retry delay.
                        if current.is_none() {
                            next_dial = Instant::now();
                        }
                    }
                }
            }
            joined = attempts.join_next(), if !attempts.is_empty() => {
                match joined {
                    Some(Ok((_attempt_id, AttemptOutcome::Fatal(reason)))) => {
                        attempts.abort_all();
                        while attempts.join_next().await.is_some() {}
                        return SlotResult::Fatal(reason);
                    }
                    Some(Ok((attempt_id, outcome))) if current == Some(attempt_id) => {
                        current = None;
                        match outcome {
                            AttemptOutcome::Served { uptime } => {
                                if uptime >= STABLE_CONNECTION {
                                    backoff.reset();
                                }
                            }
                            AttemptOutcome::Retryable(reason) => {
                                tracing::warn!(attempt_id, target = %target, %reason, "Tunnel connection attempt failed; retrying");
                            }
                            AttemptOutcome::Cancelled | AttemptOutcome::Fatal(_) => {}
                        }
                        if !stopping_gracefully {
                            next_dial = Instant::now() + backoff.next_delay();
                        }
                    }
                    Some(Err(error)) if error.is_panic() => {
                        attempts.abort_all();
                        while attempts.join_next().await.is_some() {}
                        return SlotResult::Fatal(format!("connection task panicked: {error}"));
                    }
                    _ => {}
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_one_attempt(
    attempt_id: u64,
    target: Target,
    endpoint: Endpoint,
    options: Arc<ResolvedOptions>,
    resolver: TargetResolver,
    tls: Option<TlsConnector>,
    events: tokio::sync::mpsc::Sender<AttemptEvent>,
    graceful: CancellationToken,
    force: CancellationToken,
    slot_cancel: CancellationToken,
    drain: Arc<DrainState>,
) -> AttemptOutcome {
    let authorization = match options.auth_token.authorization_header() {
        Ok(value) => value,
        Err(error) => return AttemptOutcome::Retryable(error.to_string()),
    };
    let connection_id = uuid::Uuid::new_v4().simple().to_string();
    let io = match dial_target(
        &target,
        &resolver,
        tls.as_ref(),
        &graceful,
        &force,
        &slot_cancel,
    )
    .await
    {
        Ok(io) => io,
        Err(error) => return AttemptOutcome::Retryable(error),
    };

    run_attempt(
        io,
        AttemptContext {
            attempt_id,
            endpoint,
            identity_verifier: Arc::clone(&options.identity_verifier),
            authorization,
            environment_id: options.environment_id.clone(),
            tunnel_name: options.tunnel_name.clone(),
            tunnel_worker_id: options.tunnel_worker_id.clone(),
            tunnel_connection_id: connection_id,
            target: target.to_string(),
            drain,
            graceful,
            force,
            slot_cancel,
            events,
            server_drain_grace: SERVER_DRAIN_GRACE,
        },
    )
    .await
}

async fn dial_target(
    target: &Target,
    resolver: &TargetResolver,
    tls: Option<&TlsConnector>,
    graceful: &CancellationToken,
    force: &CancellationToken,
    slot_cancel: &CancellationToken,
) -> Result<BoxIo, String> {
    let addresses = tokio::select! {
        _ = graceful.cancelled() => return Err("tunnel is draining".into()),
        _ = force.cancelled() => return Err("tunnel closed".into()),
        _ = slot_cancel.cancelled() => return Err("target removed".into()),
        result = resolver.resolve_dial_addresses(target) => result.map_err(|error| error.to_string())?,
    };
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut last_error = None;

    for address in addresses {
        let connect = timeout_at(deadline, TcpStream::connect(address));
        let stream = match tokio::select! {
            _ = graceful.cancelled() => return Err("tunnel is draining".into()),
            _ = force.cancelled() => return Err("tunnel closed".into()),
            _ = slot_cancel.cancelled() => return Err("target removed".into()),
            result = connect => result,
        } {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                last_error = Some(error.to_string());
                continue;
            }
            Err(_) => return Err("TCP/TLS connect deadline elapsed".into()),
        };
        if let Err(error) = configure_tcp(&stream) {
            last_error = Some(error.to_string());
            continue;
        }
        if target.is_plaintext() {
            return Ok(Box::new(stream));
        }

        let tls = tls.ok_or_else(|| "TLS connector is unavailable".to_owned())?;
        let server_name = ServerName::try_from(target.server_name().to_owned())
            .map_err(|_| format!("invalid TLS server name {:?}", target.server_name()))?;
        let connect = timeout_at(deadline, tls.connect(server_name, stream));
        let stream = match tokio::select! {
            _ = graceful.cancelled() => return Err("tunnel is draining".into()),
            _ = force.cancelled() => return Err("tunnel closed".into()),
            _ = slot_cancel.cancelled() => return Err("target removed".into()),
            result = connect => result,
        } {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                last_error = Some(error.to_string());
                continue;
            }
            Err(_) => return Err("TCP/TLS connect deadline elapsed".into()),
        };
        if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
            return Err("tunnel server did not negotiate h2 ALPN".into());
        }
        return Ok(Box::new(stream));
    }

    Err(last_error.unwrap_or_else(|| "no address could be connected".into()))
}

fn configure_tcp(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    SockRef::from(stream).set_tcp_keepalive(&TcpKeepalive::new().with_time(TCP_KEEPALIVE))
}

fn build_tls_connector() -> Result<TlsConnector, String> {
    #[cfg(feature = "rust_crypto")]
    let provider = rustls::crypto::ring::default_provider();

    #[cfg(all(not(feature = "rust_crypto"), feature = "aws_lc_rs"))]
    let provider = rustls::crypto::aws_lc_rs::default_provider();

    #[cfg(not(any(feature = "rust_crypto", feature = "aws_lc_rs")))]
    return Err("the tunnel feature requires either the rust_crypto or aws_lc_rs feature".into());

    #[cfg(any(feature = "rust_crypto", feature = "aws_lc_rs"))]
    {
        let native = rustls_native_certs::load_native_certs();
        let mut roots = rustls::RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(native.certs);
        if added == 0 {
            return Err(format!(
                "no usable native TLS roots were found{}",
                if native.errors.is_empty() {
                    String::new()
                } else {
                    format!(" ({:?})", native.errors)
                }
            ));
        }
        if !native.errors.is_empty() {
            tracing::warn!(errors = ?native.errors, "Some native TLS roots could not be loaded");
        }
        let mut config = rustls::ClientConfig::builder_with_provider(provider.into())
            .with_safe_default_protocol_versions()
            .map_err(|error| format!("TLS protocol configuration failed: {error}"))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        Ok(TlsConnector::from(Arc::new(config)))
    }
}

struct Backoff {
    next: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            next: RECONNECT_INITIAL,
        }
    }

    fn reset(&mut self) {
        self.next = RECONNECT_INITIAL;
    }

    fn next_delay(&mut self) -> Duration {
        let millis = self.next.as_millis().min(u128::from(u64::MAX)) as u64;
        let lower = millis / 2;
        let upper = millis.saturating_mul(3) / 2;
        let jittered = Duration::from_millis(rand::random_range(lower..=upper));
        self.next = self.next.saturating_mul(2).min(RECONNECT_MAX);
        jittered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded_and_resets() {
        let mut backoff = Backoff::new();
        for _ in 0..64 {
            assert!(backoff.next_delay() <= RECONNECT_MAX + RECONNECT_MAX / 2);
        }
        backoff.reset();
        assert!(backoff.next_delay() <= RECONNECT_INITIAL + RECONNECT_INITIAL / 2);
    }

    #[tokio::test]
    async fn terminal_state_retains_raced_readiness() {
        let info = TunnelInfo::from_handshake(
            "requested".into(),
            "https://proxy.example/environment/requested".into(),
            "https://tunnel.example/requested".into(),
        )
        .unwrap();
        let (status, _) = watch::channel(EngineStatus::Terminal {
            result: Ok(()),
            ready: Some(info.clone()),
        });
        let engine = Engine {
            status,
            graceful: CancellationToken::new(),
            force: CancellationToken::new(),
            drain: DrainState::new(),
            task: Mutex::new(None),
        };

        assert_eq!(engine.wait_ready().await.unwrap(), info);
    }
}
