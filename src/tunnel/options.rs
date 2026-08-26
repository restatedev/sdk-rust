//! Operator configuration resolution and secret handling.

use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use http::HeaderValue;
use restate_sdk_shared_core::{IdentityVerifier, KeyError};

use super::targets::{Discovery, TargetError, parse_server_address};

pub(crate) const TUNNEL_NAME_ENV: &str = "RESTATE_INPROC_TUNNEL_NAME";
pub(crate) const ENVIRONMENT_ID_ENV: &str = "RESTATE_INPROC_ENVIRONMENT_ID";
pub(crate) const CLOUD_REGION_ENV: &str = "RESTATE_INPROC_CLOUD_REGION";
pub(crate) const SIGNING_PUBLIC_KEY_ENV: &str = "RESTATE_INPROC_SIGNING_PUBLIC_KEY";
pub(crate) const AUTH_TOKEN_FILE_ENV: &str = "RESTATE_INPROC_AUTH_TOKEN_FILE";
pub(crate) const AUTH_TOKEN_ENV: &str = "RESTATE_AUTH_TOKEN";
pub(crate) const TUNNEL_WORKER_ID_ENV: &str = "RESTATE_TUNNEL_WORKER_ID";

// Unprefixed aliases matching the names Restate Cloud injects into a deployed
// container (and the standalone tunnel client's own variables). Each is a
// fallback consulted only when the `RESTATE_INPROC_*` primary above is unset,
// so existing operator-managed deployments keep their current behaviour.
pub(crate) const TUNNEL_NAME_ENV_ALIAS: &str = "RESTATE_TUNNEL_NAME";
pub(crate) const ENVIRONMENT_ID_ENV_ALIAS: &str = "RESTATE_ENVIRONMENT_ID";
pub(crate) const SIGNING_PUBLIC_KEY_ENV_ALIAS: &str = "RESTATE_SIGNING_PUBLIC_KEY";
pub(crate) const TUNNEL_SERVERS_SRV_ENV: &str = "RESTATE_TUNNEL_SERVERS_SRV";

const MAX_AUTH_TOKEN_FILE_BYTES: u64 = 64 * 1024;
const DEFAULT_WORKER_HOST_MAX_BYTES: usize = 96;

/// Unresolved tunnel configuration populated by the public builder.
///
/// Every field is crate-visible so `Tunnel` can offer its public, consuming
/// builder methods without duplicating configuration state here.
#[derive(Clone, Default)]
pub(crate) struct Config {
    pub(crate) region: Option<String>,
    pub(crate) tunnel_servers_srv: Option<String>,
    pub(crate) tunnel_servers: Option<Vec<String>>,
    pub(crate) environment_id: Option<String>,
    pub(crate) auth_token: Option<String>,
    pub(crate) auth_token_file: Option<PathBuf>,
    pub(crate) signing_public_key: Option<String>,
    pub(crate) tunnel_name: Option<String>,
    pub(crate) tunnel_worker_id: Option<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("region", &self.region)
            .field("tunnel_servers_srv", &self.tunnel_servers_srv)
            .field("tunnel_servers", &self.tunnel_servers)
            .field("environment_id", &self.environment_id)
            .field("auth_token", &self.auth_token.as_ref().map(|_| Redacted))
            .field("auth_token_file", &self.auth_token_file)
            .field("signing_public_key", &self.signing_public_key)
            .field("tunnel_name", &self.tunnel_name)
            .field("tunnel_worker_id", &self.tunnel_worker_id)
            .finish()
    }
}

impl Config {
    /// Apply operator environment fallbacks and perform all startup
    /// validation, including the initial token-file read and signing-key parse.
    pub(crate) fn resolve(&self) -> Result<ResolvedOptions, ConfigError> {
        self.resolve_with_env(|name| std::env::var_os(name))
    }

    fn resolve_with_env(
        &self,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> Result<ResolvedOptions, ConfigError> {
        let discovery = self.resolve_discovery(&env)?;
        let environment_id = option_or_env_string(
            self.environment_id.as_deref(),
            ENVIRONMENT_ID_ENV,
            ENVIRONMENT_ID_ENV_ALIAS,
            "environment_id",
            &env,
        )?;
        if !valid_environment_id(&environment_id) {
            return Err(ConfigError::InvalidEnvironmentId);
        }

        let tunnel_name = option_or_env_string(
            self.tunnel_name.as_deref(),
            TUNNEL_NAME_ENV,
            TUNNEL_NAME_ENV_ALIAS,
            "tunnel_name",
            &env,
        )?;
        if !valid_tunnel_name(&tunnel_name) {
            return Err(ConfigError::InvalidTunnelName);
        }

        let signing_public_key = option_or_env_string(
            self.signing_public_key.as_deref(),
            SIGNING_PUBLIC_KEY_ENV,
            SIGNING_PUBLIC_KEY_ENV_ALIAS,
            "signing_public_key",
            &env,
        )?;
        let identity_verifier = IdentityVerifier::new(&[signing_public_key.as_str()])
            .map_err(ConfigError::SigningPublicKey)?;

        let tunnel_worker_id = match nonempty(self.tunnel_worker_id.as_deref()) {
            Some(worker_id) => worker_id.to_owned(),
            None => match env_string(TUNNEL_WORKER_ID_ENV, &env)? {
                Some(worker_id) => worker_id,
                None => default_tunnel_worker_id().to_owned(),
            },
        };
        require_header_safe(&tunnel_worker_id, "tunnel_worker_id")?;

        let auth_token = self.resolve_auth_token(&env)?;
        // Fail initial token-file misconfiguration during preflight. Every dial
        // calls the same method again and therefore observes Secret rotation.
        auth_token.authorization_header()?;
        install_identity_crypto_provider();

        Ok(ResolvedOptions {
            discovery,
            environment_id,
            auth_token,
            identity_verifier: Arc::new(identity_verifier),
            tunnel_name,
            tunnel_worker_id,
        })
    }

    fn resolve_discovery(
        &self,
        env: &impl Fn(&str) -> Option<OsString>,
    ) -> Result<Discovery, ConfigError> {
        let explicit_region = nonempty(self.region.as_deref());
        let explicit_srv = nonempty(self.tunnel_servers_srv.as_deref());
        let explicit_servers = self.tunnel_servers.as_ref();

        let explicit_count = usize::from(explicit_region.is_some())
            + usize::from(explicit_srv.is_some())
            + usize::from(explicit_servers.is_some());
        if explicit_count > 1 {
            return Err(ConfigError::MultipleDiscoverySources);
        }

        if let Some(addresses) = explicit_servers {
            if addresses.is_empty() {
                // An explicitly supplied server list suppresses an injected
                // region, even if empty, so configuration mistakes stay loud.
                return Err(ConfigError::EmptyTunnelServers);
            }
            let targets = addresses
                .iter()
                .map(|address| parse_server_address(address))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Discovery::Explicit(targets));
        }

        if let Some(srv) = explicit_srv {
            validate_srv_name(srv)?;
            return Ok(Discovery::Srv(srv.to_owned()));
        }

        // With no explicit builder discovery source, honour the unprefixed SRV
        // name Restate Cloud injects (and the standalone client uses) before
        // deriving one from a region.
        if explicit_region.is_none()
            && let Some(srv) = env_string(TUNNEL_SERVERS_SRV_ENV, env)?
        {
            validate_srv_name(&srv)?;
            return Ok(Discovery::Srv(srv));
        }

        let region = match explicit_region {
            Some(region) => Some(region.to_owned()),
            None => env_string(CLOUD_REGION_ENV, env)?,
        }
        .ok_or(ConfigError::MissingDiscoverySource)?;
        validate_region(&region)?;
        Ok(Discovery::Srv(srv_name_for_region(&region)))
    }

    /// Resolve the token source with the following precedence, highest first:
    /// 1. the explicit builder `auth_token` (direct token),
    /// 2. the explicit builder `auth_token_file`,
    /// 3. the `RESTATE_AUTH_TOKEN` environment variable (direct token),
    /// 4. the `RESTATE_INPROC_AUTH_TOKEN_FILE` environment variable.
    ///
    /// Explicit builder options always win over the environment, and within a
    /// tier a direct token wins over a token file. Only the file source is
    /// reread per dial, so an environment-supplied direct token is fixed for the
    /// process lifetime.
    fn resolve_auth_token(
        &self,
        env: &impl Fn(&str) -> Option<OsString>,
    ) -> Result<AuthTokenSource, ConfigError> {
        if let Some(token) = nonempty(self.auth_token.as_deref()) {
            require_header_safe(token, "auth_token")?;
            return Ok(AuthTokenSource::Literal(Arc::from(token)));
        }

        if let Some(path) = self
            .auth_token_file
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty())
        {
            return Ok(AuthTokenSource::File(Arc::new(path.to_owned())));
        }

        if let Some(token) = env_string(AUTH_TOKEN_ENV, env)? {
            require_header_safe(&token, "auth_token")?;
            return Ok(AuthTokenSource::Literal(Arc::from(token.as_str())));
        }

        let path = env(AUTH_TOKEN_FILE_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or(ConfigError::MissingAuthToken)?;
        Ok(AuthTokenSource::File(Arc::new(path)))
    }
}

/// Select the same deterministic precedence as the tunnel's rustls provider.
/// jsonwebtoken cannot infer a provider when Cargo unifies both backend
/// features, so install RustCrypto during preflight before any signed request
/// can reach the shared-core verifier. An application-installed provider wins.
fn install_identity_crypto_provider() {
    #[cfg(feature = "rust_crypto")]
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();

    #[cfg(all(not(feature = "rust_crypto"), feature = "aws_lc_rs"))]
    let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
}

/// Validated values shared by every connection attempt in one engine.
#[derive(Clone)]
pub(crate) struct ResolvedOptions {
    pub(crate) discovery: Discovery,
    pub(crate) environment_id: String,
    pub(crate) auth_token: AuthTokenSource,
    pub(crate) identity_verifier: Arc<IdentityVerifier>,
    pub(crate) tunnel_name: String,
    pub(crate) tunnel_worker_id: String,
}

impl fmt::Debug for ResolvedOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedOptions")
            .field("discovery", &self.discovery)
            .field("environment_id", &self.environment_id)
            .field("auth_token", &self.auth_token)
            .field("identity_verifier", &"configured")
            .field("tunnel_name", &self.tunnel_name)
            .field("tunnel_worker_id", &self.tunnel_worker_id)
            .finish()
    }
}

/// A process-stable token source. File-backed sources reopen the path on every
/// call so Kubernetes projected-Secret symlink swaps are observed.
#[derive(Clone)]
pub(crate) enum AuthTokenSource {
    Literal(Arc<str>),
    File(Arc<PathBuf>),
}

impl AuthTokenSource {
    /// Read the current token and construct the sensitive Authorization value
    /// for a connection attempt. The token itself has no `Display` or `Debug`
    /// representation in this module.
    pub(crate) fn authorization_header(&self) -> Result<HeaderValue, TokenError> {
        match self {
            Self::Literal(token) => authorization_header(token),
            Self::File(path) => {
                let token = read_token_file(path)?;
                authorization_header(&token)
            }
        }
    }
}

impl fmt::Debug for AuthTokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(_) => f.debug_tuple("Literal").field(&Redacted).finish(),
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
        }
    }
}

#[derive(Clone, Copy)]
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

fn authorization_header(token: &str) -> Result<HeaderValue, TokenError> {
    require_token_header_safe(token)?;
    let mut bytes = Vec::with_capacity("Bearer ".len() + token.len());
    bytes.extend_from_slice(b"Bearer ");
    bytes.extend_from_slice(token.as_bytes());
    let mut value = HeaderValue::from_bytes(&bytes).map_err(|_| TokenError::InvalidHeader)?;
    value.set_sensitive(true);
    Ok(value)
}

fn read_token_file(path: &Path) -> Result<String, TokenError> {
    // `metadata` follows symlinks, which is required for Kubernetes projected
    // Secrets. Checking before open avoids blocking on a configured FIFO.
    let metadata = std::fs::metadata(path).map_err(|source| TokenError::Metadata {
        path: path.to_owned(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(TokenError::NotRegularFile(path.to_owned()));
    }
    if metadata.len() > MAX_AUTH_TOKEN_FILE_BYTES {
        return Err(TokenError::TooLarge {
            path: path.to_owned(),
            size: metadata.len(),
        });
    }

    let file = File::open(path).map_err(|source| TokenError::Open {
        path: path.to_owned(),
        source,
    })?;
    let opened_metadata = file.metadata().map_err(|source| TokenError::Metadata {
        path: path.to_owned(),
        source,
    })?;
    if !opened_metadata.is_file() {
        return Err(TokenError::NotRegularFile(path.to_owned()));
    }

    // Bound the actual read as well as metadata. This catches files that grow
    // between stat and read, and prevents an unbounded read after path races.
    let mut bytes =
        Vec::with_capacity(opened_metadata.len().min(MAX_AUTH_TOKEN_FILE_BYTES) as usize);
    file.take(MAX_AUTH_TOKEN_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| TokenError::Read {
            path: path.to_owned(),
            source,
        })?;
    if bytes.len() as u64 > MAX_AUTH_TOKEN_FILE_BYTES {
        return Err(TokenError::TooLarge {
            path: path.to_owned(),
            size: bytes.len() as u64,
        });
    }

    let token = String::from_utf8(bytes).map_err(|_| TokenError::InvalidUtf8(path.to_owned()))?;
    let token = token.trim();
    if token.is_empty() {
        return Err(TokenError::Empty(path.to_owned()));
    }
    require_token_header_safe(token).map_err(|_| TokenError::InvalidFileHeader(path.to_owned()))?;
    Ok(token.to_owned())
}

fn require_token_header_safe(value: &str) -> Result<(), TokenError> {
    if header_safe(value) {
        Ok(())
    } else {
        Err(TokenError::InvalidHeader)
    }
}

fn require_header_safe(value: &str, field: &'static str) -> Result<(), ConfigError> {
    if header_safe(value) {
        Ok(())
    } else {
        Err(ConfigError::InvalidHeaderValue(field))
    }
}

fn header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn option_or_env_string(
    option: Option<&str>,
    env_name: &'static str,
    alias: &'static str,
    field: &'static str,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Result<String, ConfigError> {
    if let Some(value) = nonempty(option) {
        return Ok(value.to_owned());
    }
    env_string_with_alias(env_name, alias, env)?.ok_or(ConfigError::MissingRequired {
        field,
        env: env_name,
    })
}

/// Read `name`, falling back to the unprefixed `alias` when the primary is
/// unset or empty. The `RESTATE_INPROC_*` primary always wins so operator
/// injection keeps precedence over a stray unprefixed variable.
fn env_string_with_alias(
    name: &'static str,
    alias: &'static str,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Result<Option<String>, ConfigError> {
    if let Some(value) = env_string(name, env)? {
        return Ok(Some(value));
    }
    env_string(alias, env)
}

fn env_string(
    name: &'static str,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Result<Option<String>, ConfigError> {
    let Some(value) = env(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    value
        .into_string()
        .map(Some)
        .map_err(|_| ConfigError::NonUnicodeEnvironment(name))
}

fn valid_environment_id(value: &str) -> bool {
    value
        .strip_prefix("env_")
        .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(is_identifier_byte))
}

fn valid_tunnel_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn validate_region(region: &str) -> Result<(), ConfigError> {
    let valid = !region.is_empty()
        && region.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(ConfigError::InvalidRegion(region.to_owned()))
    }
}

fn validate_srv_name(name: &str) -> Result<(), ConfigError> {
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(ConfigError::InvalidSrvName(name.to_owned()))
    }
}

pub(crate) fn srv_name_for_region(region: &str) -> String {
    format!("tunnel.{region}.restate.cloud")
}

fn default_tunnel_worker_id() -> &'static str {
    static WORKER_ID: OnceLock<String> = OnceLock::new();
    WORKER_ID.get_or_init(|| {
        let host = hostname::get()
            .ok()
            .and_then(|host| host.into_string().ok())
            .unwrap_or_else(|| "worker".to_owned());
        let host = sanitize_worker_id_segment(&host);
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        format!("{host}-{}", &suffix[..8])
    })
}

fn sanitize_worker_id_segment(value: &str) -> String {
    let mut result = String::with_capacity(value.len().min(DEFAULT_WORKER_HOST_MAX_BYTES));
    let mut last_was_dash = false;
    for byte in value.bytes() {
        let allowed = byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-');
        let byte = if allowed { byte } else { b'-' };
        if byte == b'-' && last_was_dash {
            continue;
        }
        result.push(char::from(byte));
        last_was_dash = byte == b'-';
        if result.len() == DEFAULT_WORKER_HOST_MAX_BYTES {
            break;
        }
    }
    let trimmed = result.trim_matches('-');
    if trimmed.is_empty() {
        "worker".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("tunnel: {field} is required (pass the option or set {env})")]
    MissingRequired {
        field: &'static str,
        env: &'static str,
    },
    #[error(
        "tunnel: specify one of region, tunnel_servers_srv, or tunnel_servers (or set {CLOUD_REGION_ENV} or {TUNNEL_SERVERS_SRV_ENV})"
    )]
    MissingDiscoverySource,
    #[error("tunnel: specify exactly one of region, tunnel_servers_srv, or tunnel_servers")]
    MultipleDiscoverySources,
    #[error("tunnel: tunnel_servers must not be empty")]
    EmptyTunnelServers,
    #[error(
        "tunnel: auth_token is required (pass it explicitly, or set {AUTH_TOKEN_ENV} or {AUTH_TOKEN_FILE_ENV})"
    )]
    MissingAuthToken,
    #[error("tunnel: invalid region {0:?}")]
    InvalidRegion(String),
    #[error("tunnel: invalid tunnel_servers_srv {0:?}")]
    InvalidSrvName(String),
    #[error("tunnel: environment_id must be env_ followed by alphanumerics, '_' or '-'")]
    InvalidEnvironmentId,
    #[error("tunnel: tunnel_name may contain only letters, digits, '.', '_' or '-'")]
    InvalidTunnelName,
    #[error(
        "tunnel: {0} contains characters that cannot travel in an HTTP header (whitespace or non-printable)"
    )]
    InvalidHeaderValue(&'static str),
    #[error("tunnel: environment variable {0} is not valid UTF-8")]
    NonUnicodeEnvironment(&'static str),
    #[error("tunnel: invalid signing_public_key: {0}")]
    SigningPublicKey(#[source] KeyError),
    #[error(transparent)]
    Target(#[from] TargetError),
    #[error(transparent)]
    Token(#[from] TokenError),
}

/// Token loading errors never contain the token value.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TokenError {
    #[error("tunnel: cannot inspect auth token file {path:?}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("tunnel: auth token file {0:?} is not a regular file")]
    NotRegularFile(PathBuf),
    #[error("tunnel: auth token file {path:?} is too large ({size} bytes; maximum is 65536)")]
    TooLarge { path: PathBuf, size: u64 },
    #[error("tunnel: cannot open auth token file {path:?}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("tunnel: cannot read auth token file {path:?}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("tunnel: auth token file {0:?} is not valid UTF-8")]
    InvalidUtf8(PathBuf),
    #[error("tunnel: auth token file {0:?} is empty after trimming")]
    Empty(PathBuf),
    #[error("tunnel: auth token contains characters that cannot travel in an HTTP header")]
    InvalidHeader,
    #[error(
        "tunnel: auth token file {0:?} contains characters that cannot travel in an HTTP header"
    )]
    InvalidFileHeader(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::fs;

    const SIGNING_KEY: &str = "publickeyv1_ChjENKeMvCtRnqG2mrBK1HmPKufgFUc98K8B3ononQvp";

    fn valid_config() -> Config {
        Config {
            region: Some("us".into()),
            environment_id: Some("env_abc123".into()),
            auth_token: Some("key_xyz.secret".into()),
            signing_public_key: Some(SIGNING_KEY.into()),
            tunnel_name: Some("greeter-abc123".into()),
            tunnel_worker_id: Some("worker-test".into()),
            ..Config::default()
        }
    }

    fn resolve_with(
        config: &Config,
        values: &[(&str, &OsStr)],
    ) -> Result<ResolvedOptions, ConfigError> {
        let env = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>();
        config.resolve_with_env(|name| env.get(name).cloned())
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "restate-rust-tunnel-options-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_values_override_environment_and_empty_values_fall_back() {
        let mut config = valid_config();
        config.tunnel_name = Some(String::new());
        let options = resolve_with(
            &config,
            &[
                (TUNNEL_NAME_ENV, OsStr::new("from-env")),
                (ENVIRONMENT_ID_ENV, OsStr::new("invalid-env-id")),
                (CLOUD_REGION_ENV, OsStr::new("INVALID-REGION")),
                (SIGNING_PUBLIC_KEY_ENV, OsStr::new("invalid-key")),
                (TUNNEL_WORKER_ID_ENV, OsStr::new("invalid worker id")),
            ],
        )
        .unwrap();

        assert_eq!(options.tunnel_name, "from-env");
        assert_eq!(options.environment_id, "env_abc123");
        assert_eq!(options.discovery, Discovery::Srv(srv_name_for_region("us")));
        assert_eq!(options.tunnel_worker_id, "worker-test");
    }

    #[test]
    fn operator_only_configuration_resolves() {
        let temp = TempDir::new();
        let token = temp.file("token", b"key_from_file\n");
        let config = Config::default();
        let options = resolve_with(
            &config,
            &[
                (TUNNEL_NAME_ENV, OsStr::new("greeter-5b8c7d9f4")),
                (ENVIRONMENT_ID_ENV, OsStr::new("env_abc123")),
                (CLOUD_REGION_ENV, OsStr::new("eu")),
                (SIGNING_PUBLIC_KEY_ENV, OsStr::new(SIGNING_KEY)),
                (AUTH_TOKEN_FILE_ENV, token.as_os_str()),
                (TUNNEL_WORKER_ID_ENV, OsStr::new("worker-1")),
            ],
        )
        .unwrap();

        assert_eq!(options.discovery, Discovery::Srv(srv_name_for_region("eu")));
        assert_eq!(options.tunnel_name, "greeter-5b8c7d9f4");
        assert_eq!(options.tunnel_worker_id, "worker-1");
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer key_from_file"
        );
    }

    #[test]
    fn cloud_injected_unprefixed_env_resolves() {
        // Mirror exactly the unprefixed variables Restate Cloud injects into a
        // deployed container, with no builder options and no RESTATE_INPROC_*.
        let config = Config::default();
        let options = resolve_with(
            &config,
            &[
                (
                    ENVIRONMENT_ID_ENV_ALIAS,
                    OsStr::new("env_201kxe0kdaynq7h1fsn2chhcbwb"),
                ),
                (AUTH_TOKEN_ENV, OsStr::new("key_from_env")),
                (TUNNEL_NAME_ENV_ALIAS, OsStr::new("my-tunnel")),
                (SIGNING_PUBLIC_KEY_ENV_ALIAS, OsStr::new(SIGNING_KEY)),
                (
                    TUNNEL_SERVERS_SRV_ENV,
                    OsStr::new("tunnel.eu.restate.cloud"),
                ),
            ],
        )
        .unwrap();

        assert_eq!(options.environment_id, "env_201kxe0kdaynq7h1fsn2chhcbwb");
        assert_eq!(options.tunnel_name, "my-tunnel");
        assert_eq!(
            options.discovery,
            Discovery::Srv("tunnel.eu.restate.cloud".into())
        );
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer key_from_env"
        );
    }

    #[test]
    fn inproc_env_wins_over_unprefixed_alias() {
        let mut config = valid_config();
        config.environment_id = None;
        config.tunnel_name = None;
        config.signing_public_key = None;
        let options = resolve_with(
            &config,
            &[
                (ENVIRONMENT_ID_ENV, OsStr::new("env_primary")),
                (ENVIRONMENT_ID_ENV_ALIAS, OsStr::new("env_alias")),
                (TUNNEL_NAME_ENV, OsStr::new("primary-name")),
                (TUNNEL_NAME_ENV_ALIAS, OsStr::new("alias-name")),
                (SIGNING_PUBLIC_KEY_ENV, OsStr::new(SIGNING_KEY)),
                (
                    SIGNING_PUBLIC_KEY_ENV_ALIAS,
                    OsStr::new("publickeyv1_bogus"),
                ),
            ],
        )
        .unwrap();
        assert_eq!(options.environment_id, "env_primary");
        assert_eq!(options.tunnel_name, "primary-name");
    }

    #[test]
    fn tunnel_servers_srv_env_drives_discovery_and_yields_to_builder() {
        // The env SRV name is honoured when no builder discovery source is set,
        // and wins over an injected region.
        let mut config = valid_config();
        config.region = None;
        let options = resolve_with(
            &config,
            &[
                (
                    TUNNEL_SERVERS_SRV_ENV,
                    OsStr::new("tunnel.eu.restate.cloud"),
                ),
                (CLOUD_REGION_ENV, OsStr::new("us")),
            ],
        )
        .unwrap();
        assert_eq!(
            options.discovery,
            Discovery::Srv("tunnel.eu.restate.cloud".into())
        );

        // A builder region beats the env SRV name.
        let mut config = valid_config();
        let options = resolve_with(
            &config,
            &[(
                TUNNEL_SERVERS_SRV_ENV,
                OsStr::new("tunnel.eu.restate.cloud"),
            )],
        )
        .unwrap();
        assert_eq!(options.discovery, Discovery::Srv(srv_name_for_region("us")));

        // An invalid env SRV name is rejected like an explicit one.
        config.region = None;
        assert!(matches!(
            resolve_with(&config, &[(TUNNEL_SERVERS_SRV_ENV, OsStr::new("bad srv"))]),
            Err(ConfigError::InvalidSrvName(_))
        ));
    }

    #[test]
    fn explicit_discovery_suppresses_injected_region() {
        let mut config = valid_config();
        config.region = None;
        config.tunnel_servers_srv = Some("tunnel.dev.example.cloud".into());
        let options = resolve_with(&config, &[(CLOUD_REGION_ENV, OsStr::new("eu"))]).unwrap();
        assert_eq!(
            options.discovery,
            Discovery::Srv("tunnel.dev.example.cloud".into())
        );

        config.tunnel_servers_srv = None;
        config.tunnel_servers = Some(Vec::new());
        assert!(matches!(
            resolve_with(&config, &[(CLOUD_REGION_ENV, OsStr::new("eu"))]),
            Err(ConfigError::EmptyTunnelServers)
        ));
    }

    #[test]
    fn validates_discovery_identity_and_header_fields() {
        for region in ["US", ".us", "us.", "us..eu", "us/extra"] {
            let mut config = valid_config();
            config.region = Some(region.into());
            assert!(matches!(
                config.resolve(),
                Err(ConfigError::InvalidRegion(_))
            ));
        }

        let mut config = valid_config();
        config.environment_id = Some("abc123".into());
        assert!(matches!(
            config.resolve(),
            Err(ConfigError::InvalidEnvironmentId)
        ));

        let mut config = valid_config();
        config.tunnel_name = Some("bad/name".into());
        assert!(matches!(
            config.resolve(),
            Err(ConfigError::InvalidTunnelName)
        ));

        let mut config = valid_config();
        config.tunnel_worker_id = Some("worker id".into());
        assert!(matches!(
            config.resolve(),
            Err(ConfigError::InvalidHeaderValue("tunnel_worker_id"))
        ));

        let mut config = valid_config();
        config.signing_public_key = Some("publickeyv1_not-base58!".into());
        assert!(matches!(
            config.resolve(),
            Err(ConfigError::SigningPublicKey(_))
        ));
    }

    #[test]
    fn rejects_missing_empty_and_conflicting_configuration() {
        let mut config = valid_config();
        config.region = None;
        assert!(matches!(
            resolve_with(&config, &[(CLOUD_REGION_ENV, OsStr::new(""))]),
            Err(ConfigError::MissingDiscoverySource)
        ));

        let mut config = valid_config();
        config.environment_id = None;
        let error = resolve_with(&config, &[(ENVIRONMENT_ID_ENV, OsStr::new(""))])
            .unwrap_err()
            .to_string();
        assert!(error.contains(ENVIRONMENT_ID_ENV));

        let mut config = valid_config();
        config.auth_token = None;
        assert!(matches!(
            resolve_with(&config, &[(AUTH_TOKEN_FILE_ENV, OsStr::new(""))]),
            Err(ConfigError::MissingAuthToken)
        ));

        let mut config = valid_config();
        config.tunnel_servers_srv = Some("other.example".into());
        assert!(matches!(
            resolve_with(&config, &[]),
            Err(ConfigError::MultipleDiscoverySources)
        ));

        let mut config = valid_config();
        config.region = None;
        config.tunnel_servers = Some(vec!["good.example:9080".into(), "no-port".into()]);
        assert!(matches!(
            resolve_with(&config, &[]),
            Err(ConfigError::Target(_))
        ));
    }

    #[test]
    fn environment_values_receive_the_same_validation() {
        let mut config = valid_config();
        config.environment_id = None;
        assert!(matches!(
            resolve_with(
                &config,
                &[(ENVIRONMENT_ID_ENV, OsStr::new("not-an-environment"))]
            ),
            Err(ConfigError::InvalidEnvironmentId)
        ));

        let mut config = valid_config();
        config.tunnel_worker_id = None;
        assert!(matches!(
            resolve_with(&config, &[(TUNNEL_WORKER_ID_ENV, OsStr::new("worker id"))]),
            Err(ConfigError::InvalidHeaderValue("tunnel_worker_id"))
        ));
    }

    #[test]
    fn explicit_token_wins_without_touching_the_file() {
        let config = valid_config();
        let options = resolve_with(
            &config,
            &[(AUTH_TOKEN_FILE_ENV, OsStr::new("/does/not/exist"))],
        )
        .unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer key_xyz.secret"
        );
    }

    #[test]
    fn auth_token_env_is_used_and_correctly_ordered() {
        // RESTATE_AUTH_TOKEN supplies a direct token when no builder token is set.
        let mut config = valid_config();
        config.auth_token = None;
        let options =
            resolve_with(&config, &[(AUTH_TOKEN_ENV, OsStr::new("key_from_env"))]).unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer key_from_env"
        );

        // An explicit builder token file beats the environment token.
        let temp = TempDir::new();
        let mut config = valid_config();
        config.auth_token = None;
        config.auth_token_file = Some(temp.file("token", b"key_from_file\n"));
        let options =
            resolve_with(&config, &[(AUTH_TOKEN_ENV, OsStr::new("key_from_env"))]).unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer key_from_file"
        );

        // The direct environment token beats the token-file environment variable.
        let mut config = valid_config();
        config.auth_token = None;
        let options = resolve_with(
            &config,
            &[
                (AUTH_TOKEN_ENV, OsStr::new("key_from_env")),
                (AUTH_TOKEN_FILE_ENV, OsStr::new("/does/not/exist")),
            ],
        )
        .unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer key_from_env"
        );

        // A header-unsafe environment token is rejected during preflight.
        let mut config = valid_config();
        config.auth_token = None;
        assert!(matches!(
            resolve_with(&config, &[(AUTH_TOKEN_ENV, OsStr::new("bad token"))]),
            Err(ConfigError::InvalidHeaderValue("auth_token"))
        ));
    }

    #[test]
    fn token_file_is_reopened_for_rotation_and_transient_failure() {
        let temp = TempDir::new();
        let path = temp.file("token", b"first\n");
        let mut config = valid_config();
        config.auth_token = None;
        config.auth_token_file = Some(path.clone());
        let options = config.resolve().unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer first"
        );

        fs::write(&path, b"second\n").unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer second"
        );
        fs::remove_file(&path).unwrap();
        assert!(options.auth_token.authorization_header().is_err());
        fs::write(&path, b"third").unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer third"
        );
    }

    #[cfg(unix)]
    #[test]
    fn token_file_follows_projected_secret_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        fs::create_dir(temp.0.join("..2026_01")).unwrap();
        fs::write(temp.0.join("..2026_01/token"), b"first").unwrap();
        symlink("..2026_01", temp.0.join("..data")).unwrap();
        symlink("..data/token", temp.0.join("token")).unwrap();

        let mut config = valid_config();
        config.auth_token = None;
        config.auth_token_file = Some(temp.0.join("token"));
        let options = config.resolve().unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer first"
        );

        fs::create_dir(temp.0.join("..2026_02")).unwrap();
        fs::write(temp.0.join("..2026_02/token"), b"second").unwrap();
        symlink("..2026_02", temp.0.join("..data-next")).unwrap();
        fs::rename(temp.0.join("..data-next"), temp.0.join("..data")).unwrap();
        assert_eq!(
            options.auth_token.authorization_header().unwrap(),
            "Bearer second"
        );
    }

    #[test]
    fn rejects_bad_token_files_during_preflight() {
        let temp = TempDir::new();
        let cases = [
            ("empty", b" \n".as_slice()),
            ("utf8", &[0xff, 0xfe]),
            ("header", b"token with spaces".as_slice()),
        ];
        for (name, contents) in cases {
            let mut config = valid_config();
            config.auth_token = None;
            config.auth_token_file = Some(temp.file(name, contents));
            assert!(config.resolve().is_err());
        }

        let mut config = valid_config();
        config.auth_token = None;
        config.auth_token_file = Some(temp.file("large", &vec![b'x'; 65_537]));
        assert!(matches!(
            config.resolve(),
            Err(ConfigError::Token(TokenError::TooLarge { .. }))
        ));

        config.auth_token_file = Some(temp.0.clone());
        assert!(matches!(
            config.resolve(),
            Err(ConfigError::Token(TokenError::NotRegularFile(_)))
        ));
    }

    #[test]
    fn secret_debug_output_is_redacted() {
        let config = valid_config();
        let options = config.resolve().unwrap();
        let output = format!("{config:?} {options:?}");
        assert!(!output.contains("key_xyz.secret"));
        assert!(output.contains("REDACTED"));
    }

    #[test]
    fn default_worker_id_is_process_stable_and_header_safe() {
        let mut config = valid_config();
        config.tunnel_worker_id = None;
        let first = resolve_with(&config, &[]).unwrap().tunnel_worker_id;
        let second = resolve_with(&config, &[]).unwrap().tunnel_worker_id;
        assert_eq!(first, second);
        assert!(header_safe(&first));
    }

    #[test]
    fn sanitizes_worker_host_segments() {
        assert_eq!(
            sanitize_worker_id_segment(" pod name / one "),
            "pod-name-one"
        );
        assert_eq!(sanitize_worker_id_segment("///"), "worker");
        assert_eq!(sanitize_worker_id_segment("a---b"), "a-b");
    }
}
