//! Serve a Restate [`Endpoint`] over outbound HTTP/2 connections to Restate
//! Cloud.
//!
//! The tunnel is optional and enabled with the `tunnel` feature together with
//! a crypto provider (`rust_crypto` or `aws_lc_rs`). If both are enabled,
//! `rust_crypto` is selected deterministically. It does not open a local
//! listener: forwarded requests are rewritten and streamed directly into the
//! endpoint.

mod connection;
mod draining;
mod options;
mod supervisor;
mod targets;

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use crate::endpoint::Endpoint;
use options::Config;
use supervisor::Engine;

const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(120);

/// An in-process Restate Cloud tunnel.
///
/// Operator-managed deployments normally need no explicit options: the
/// operator supplies everything needed as `RESTATE_INPROC_*` env variables.
///
/// When running without the operator, supply the configuration either in code or via env variables:
/// - `RESTATE_ENVIRONMENT_ID`: the `env_...` environment id.
/// - `RESTATE_TUNNEL_NAME`: the tunnel rendezvous name.
/// - `RESTATE_SIGNING_PUBLIC_KEY`: the request-identity public key.
/// - `RESTATE_TUNNEL_SERVERS_SRV`: the SRV discovery name.
/// - `RESTATE_AUTH_TOKEN`: the token.
pub struct Tunnel {
    endpoint: Endpoint,
    config: Config,
    shutdown_grace: Duration,
}

impl Tunnel {
    /// Create a tunnel that dispatches forwarded requests into `endpoint`.
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            config: Config::default(),
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }

    /// Set the Restate Cloud region used for SRV discovery.
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.config.region = Some(region.into());
        self
    }

    /// Set an explicit SRV discovery name instead of deriving one from a
    /// Restate Cloud region.
    pub fn tunnel_servers_srv(mut self, name: impl Into<String>) -> Self {
        self.config.tunnel_servers_srv = Some(name.into());
        self
    }

    /// Use a fixed set of tunnel servers. Entries accept `host:port`,
    /// `https://host[:port]`, or (for test/BYOC only) `http://host[:port]`.
    pub fn tunnel_servers<I, S>(mut self, servers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.tunnel_servers = Some(servers.into_iter().map(Into::into).collect());
        self
    }

    /// Set the Restate Cloud environment id (`env_...`).
    pub fn environment_id(mut self, environment_id: impl Into<String>) -> Self {
        self.config.environment_id = Some(environment_id.into());
        self
    }

    /// Set the bearer token directly. A nonempty direct token takes precedence
    /// over any configured token file.
    ///
    /// When neither this nor [`auth_token_file`](Self::auth_token_file) is set,
    /// the token is read from the `RESTATE_AUTH_TOKEN` environment variable, and
    /// failing that from the file named by `RESTATE_INPROC_AUTH_TOKEN_FILE`.
    /// Unlike a token file, an environment-supplied token is not reread and so
    /// does not rotate.
    pub fn auth_token(mut self, token: impl Into<String>) -> Self {
        self.config.auth_token = Some(token.into());
        self
    }

    /// Set a bearer-token file. The path is reopened before every dial so a
    /// projected Kubernetes Secret can rotate through symlink replacement.
    pub fn auth_token_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.auth_token_file = Some(path.into());
        self
    }

    /// Set the request-identity public key used for forwarded requests.
    pub fn signing_public_key(mut self, key: impl Into<String>) -> Self {
        self.config.signing_public_key = Some(key.into());
        self
    }

    /// Set the tunnel rendezvous name shared by replicas of this deployment.
    pub fn tunnel_name(mut self, name: impl Into<String>) -> Self {
        self.config.tunnel_name = Some(name.into());
        self
    }

    /// Set the stable diagnostic identifier sent for this worker/process.
    pub fn tunnel_worker_id(mut self, id: impl Into<String>) -> Self {
        self.config.tunnel_worker_id = Some(id.into());
        self
    }

    /// Set the default graceful-shutdown deadline.
    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    /// Validate, connect, and block until a process signal or fatal tunnel
    /// error. The first SIGINT/SIGTERM drains and a second forces closure.
    pub async fn run(self) -> Result<(), Error> {
        let options = self.config.resolve().map_err(Error::configuration)?;
        let mut signals = Signals::new().map_err(Error::signal)?;
        let engine = Engine::start(self.endpoint, options).map_err(Error::startup)?;

        let ready = tokio::select! {
            ready = engine.wait_ready() => Some(ready),
            _ = signals.recv() => None,
        };

        match ready {
            Some(Ok(info)) => {
                tracing::info!(
                    tunnel_name = info.tunnel_name(),
                    proxy_url = info.proxy_url(),
                    tunnel_url = info.tunnel_url(),
                    "Restate Cloud tunnel connected. Register this deployment with Restate: {}",
                    info.deployment_url()
                );
            }
            Some(Err(error)) => {
                engine.reap().await;
                return Err(error);
            }
            None => return shutdown_after_signal(engine, self.shutdown_grace, &mut signals).await,
        }

        tokio::select! {
            terminal = engine.wait_terminal() => {
                engine.reap().await;
                terminal
            }
            _ = signals.recv() => shutdown_after_signal(engine, self.shutdown_grace, &mut signals).await,
        }
    }

    /// Start the tunnel without installing signal handlers. This completes
    /// only after the first successful Cloud handshake.
    pub async fn connect(self) -> Result<TunnelConnection, Error> {
        let options = self.config.resolve().map_err(Error::configuration)?;
        let engine = Engine::start(self.endpoint, options).map_err(Error::startup)?;
        let info = match engine.wait_ready().await {
            Ok(info) => info,
            Err(error) => {
                // A terminal status is published immediately before the
                // engine task completes. Join it here instead of relying on
                // Engine::drop to abort and detach an already-finished task.
                engine.reap().await;
                return Err(error);
            }
        };
        Ok(TunnelConnection {
            engine: Some(engine),
            info,
            shutdown_grace: self.shutdown_grace,
        })
    }
}

async fn shutdown_after_signal(
    engine: Engine,
    grace: Duration,
    signals: &mut Signals,
) -> Result<(), Error> {
    engine.begin_shutdown();
    let deadline = tokio::time::Instant::now().checked_add(grace);
    let grace_elapsed = async move {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        terminal = engine.wait_terminal() => {
            engine.reap().await;
            terminal
        }
        _ = grace_elapsed => {
            engine.force_close();
            let result = engine.wait_terminal().await;
            engine.reap().await;
            result
        }
        _ = signals.recv() => {
            engine.force_close();
            let result = engine.wait_terminal().await;
            engine.reap().await;
            result
        }
    }
}

/// Metadata learned from a successful Cloud handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelInfo {
    tunnel_name: String,
    proxy_url: String,
    tunnel_url: String,
    deployment_url: String,
}

impl TunnelInfo {
    pub(crate) fn from_handshake(
        tunnel_name: String,
        proxy_url: String,
        tunnel_url: String,
    ) -> Result<Self, &'static str> {
        let mut proxy = url::Url::parse(&proxy_url).map_err(|_| "invalid proxy-url")?;
        if proxy.host_str().is_none() {
            return Err("proxy-url has no host");
        }
        // `url::Url` normalizes an explicit default port (for example
        // `https://host:443`) to `None`. Inspect the original authority so
        // only a genuinely absent port gets the managed-Cloud 9080 default.
        let has_explicit_port = proxy_url
            .parse::<http::Uri>()
            .ok()
            .and_then(|uri| uri.authority().and_then(|authority| authority.port_u16()))
            .is_some();
        let deployment_base = if has_explicit_port {
            proxy_url.trim_end_matches('/').to_owned()
        } else {
            proxy
                .set_port(Some(9080))
                .map_err(|_| "proxy-url cannot accept a port")?;
            proxy.as_str().trim_end_matches('/').to_owned()
        };
        let deployment_url = format!("{deployment_base}/http/in-process/9080/");
        Ok(Self {
            tunnel_name,
            proxy_url,
            tunnel_url,
            deployment_url,
        })
    }

    /// Tunnel name confirmed by Restate Cloud.
    pub fn tunnel_name(&self) -> &str {
        &self.tunnel_name
    }

    /// Proxy URL advertised by Restate Cloud.
    pub fn proxy_url(&self) -> &str {
        &self.proxy_url
    }

    /// Tunnel URL advertised by Restate Cloud.
    pub fn tunnel_url(&self) -> &str {
        &self.tunnel_url
    }

    /// Deployment URL for this in-process endpoint.
    pub fn deployment_url(&self) -> &str {
        &self.deployment_url
    }
}

/// Application-owned handle to a connected tunnel.
///
/// Dropping this value or [`close`](Self::close) is abrupt. Use
/// [`shutdown`](Self::shutdown) to drain gracefully.
pub struct TunnelConnection {
    engine: Option<Engine>,
    info: TunnelInfo,
    shutdown_grace: Duration,
}

impl TunnelConnection {
    /// Metadata from the first successful Cloud handshake.
    pub fn info(&self) -> &TunnelInfo {
        &self.info
    }

    /// Begin a bounded graceful drain using the tunnel's configured grace.
    /// Teardown starts synchronously before this method returns its future.
    pub fn shutdown(self) -> impl Future<Output = Result<(), Error>> + Send + 'static {
        let grace = self.shutdown_grace;
        self.shutdown_with_grace(grace)
    }

    /// Begin a graceful drain with an explicit deadline.
    /// Teardown starts synchronously before this method returns its future.
    pub fn shutdown_with_grace(
        mut self,
        grace: Duration,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'static {
        let engine = self.engine.take().expect("tunnel engine is present");
        engine.begin_shutdown();
        // Anchor the bound to the consuming method call, alongside the
        // synchronous drain transition. Delaying the first poll must not
        // extend an explicitly requested shutdown grace period.
        let deadline = tokio::time::Instant::now().checked_add(grace);
        async move {
            let result = match deadline {
                Some(deadline) => tokio::time::timeout_at(deadline, engine.wait_terminal()).await,
                // A duration too large to represent as an Instant is, for
                // practical purposes, an unbounded grace period. Keep force
                // cancellation available without panicking on public input.
                None => Ok(engine.wait_terminal().await),
            };
            let result = match result {
                Ok(result) => result,
                Err(_) => {
                    engine.force_close();
                    engine.wait_terminal().await
                }
            };
            engine.reap().await;
            result
        }
    }

    /// Abruptly close all tunnel connections.
    /// Teardown starts synchronously before this method returns its future.
    pub fn close(mut self) -> impl Future<Output = Result<(), Error>> + Send + 'static {
        let engine = self.engine.take().expect("tunnel engine is present");
        engine.force_close();
        async move {
            let result = engine.wait_terminal().await;
            engine.reap().await;
            result
        }
    }
}

impl Drop for TunnelConnection {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.abort();
        }
    }
}

/// Tunnel startup, protocol, or lifecycle error.
#[derive(Debug)]
pub struct Error(ErrorInner);

#[derive(Debug, thiserror::Error)]
enum ErrorInner {
    #[error("{0}")]
    Configuration(String),
    #[error("tunnel: failed to start: {0}")]
    Startup(String),
    #[error("tunnel: fatal connection error: {0}")]
    Fatal(String),
    #[error("tunnel: closed before the first successful handshake")]
    ClosedBeforeReady,
    #[error("tunnel: signal setup failed: {0}")]
    Signal(#[source] std::io::Error),
}

impl Error {
    fn configuration(error: impl fmt::Display) -> Self {
        Self(ErrorInner::Configuration(error.to_string()))
    }

    fn startup(error: impl fmt::Display) -> Self {
        Self(ErrorInner::Startup(error.to_string()))
    }

    pub(crate) fn fatal(reason: impl Into<String>) -> Self {
        Self(ErrorInner::Fatal(reason.into()))
    }

    pub(crate) fn closed_before_ready() -> Self {
        Self(ErrorInner::ClosedBeforeReady)
    }

    fn signal(error: std::io::Error) -> Self {
        Self(ErrorInner::Signal(error))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Descriptive alias for callers that prefer a qualified error name.
pub type TunnelError = Error;

struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_url_defaults_proxy_port() {
        let info = TunnelInfo::from_handshake(
            "greeter-v1".into(),
            "https://proxy.example/env/tunnel".into(),
            "https://tunnel.example".into(),
        )
        .unwrap();
        assert_eq!(
            info.deployment_url(),
            "https://proxy.example:9080/env/tunnel/http/in-process/9080/"
        );
        assert_eq!(info.proxy_url(), "https://proxy.example/env/tunnel");

        let explicit_default = TunnelInfo::from_handshake(
            "greeter-v1".into(),
            "https://proxy.example:443/env/tunnel".into(),
            "https://tunnel.example".into(),
        )
        .unwrap();
        assert_eq!(
            explicit_default.deployment_url(),
            "https://proxy.example:443/env/tunnel/http/in-process/9080/"
        );
    }
}
