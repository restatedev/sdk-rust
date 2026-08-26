//! Tunnel-server target parsing and DNS discovery.

use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::{Duration, Instant};

use futures::future::join_all;
use hickory_resolver::TokioResolver;
use hickory_resolver::net::NetError;
use hickory_resolver::proto::rr::RData;
use url::Url;

pub(crate) const DNS_REFRESH_FLOOR: Duration = Duration::from_secs(1);
pub(crate) const DNS_REFRESH_CAP: Duration = Duration::from_secs(30);
pub(crate) const DNS_REFRESH_FALLBACK: Duration = Duration::from_secs(30);

/// How the tunnel server set is discovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Discovery {
    /// Discover Cloud tunnel servers using an SRV query.
    Srv(String),
    /// A fixed set of explicitly configured tunnel servers.
    Explicit(Vec<Target>),
}

/// The network host of a tunnel target.
///
/// SRV-discovered hosts are IP addresses. Explicit hostnames intentionally
/// remain unresolved so a new dial can observe ordinary A/AAAA changes without
/// changing the fixed target set.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TargetHost {
    Ip(IpAddr),
    Name(String),
}

impl TargetHost {
    pub(crate) fn as_str(&self) -> String {
        match self {
            Self::Ip(ip) => ip.to_string(),
            Self::Name(name) => name.clone(),
        }
    }
}

/// A dialable tunnel server and all metadata that affects its connection.
///
/// `Eq` and `Hash` deliberately cover SNI and plaintext in addition to the
/// address. A DNS refresh that changes TLS metadata must replace the old slot.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Target {
    host: TargetHost,
    port: u16,
    server_name: String,
    plaintext: bool,
}

impl Target {
    fn new(host: TargetHost, port: u16, server_name: String, plaintext: bool) -> Self {
        Self {
            host,
            port,
            server_name,
            plaintext,
        }
    }

    pub(crate) fn host(&self) -> &TargetHost {
        &self.host
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// TLS certificate-verification name and SNI value.
    pub(crate) fn server_name(&self) -> &str {
        &self.server_name
    }

    pub(crate) fn is_plaintext(&self) -> bool {
        self.plaintext
    }

    pub(crate) fn socket_addr(&self) -> Option<SocketAddr> {
        match self.host {
            TargetHost::Ip(ip) => Some(SocketAddr::new(ip, self.port)),
            TargetHost::Name(_) => None,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            TargetHost::Ip(ip) => SocketAddr::new(*ip, self.port).fmt(f),
            TargetHost::Name(host) => write!(f, "{host}:{}", self.port),
        }
    }
}

/// Parse an explicit `host:port`, `https://host[:port]`, or
/// `http://host[:port]` tunnel target.
pub(crate) fn parse_server_address(address: &str) -> Result<Target, TargetError> {
    if address.contains("://") {
        return parse_server_url(address);
    }

    let authority = http::uri::Authority::from_str(address)
        .map_err(|_| TargetError::InvalidAddress(address.to_owned()))?;
    let port = authority
        .port_u16()
        .ok_or_else(|| TargetError::MissingPort(address.to_owned()))?;
    if port == 0 {
        return Err(TargetError::InvalidPort(address.to_owned()));
    }
    let host = authority.host();
    if host.is_empty() {
        return Err(TargetError::MissingHost(address.to_owned()));
    }
    let host = parse_host(host);
    let server_name = host.as_str();
    Ok(Target::new(host, port, server_name, false))
}

fn parse_server_url(address: &str) -> Result<Target, TargetError> {
    let url = Url::parse(address).map_err(|_| TargetError::InvalidUrl(address.to_owned()))?;
    let plaintext = match url.scheme() {
        "http" => true,
        "https" => false,
        scheme => return Err(TargetError::UnsupportedScheme(scheme.to_owned())),
    };

    if !url.username().is_empty() || url.password().is_some() {
        return Err(TargetError::UserInfo(address.to_owned()));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(TargetError::PathQueryOrFragment(address.to_owned()));
    }

    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| TargetError::MissingHost(address.to_owned()))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| TargetError::MissingPort(address.to_owned()))?;
    if port == 0 {
        return Err(TargetError::InvalidPort(address.to_owned()));
    }
    let host = parse_host(host);
    let server_name = host.as_str();
    Ok(Target::new(host, port, server_name, plaintext))
}

fn parse_host(host: &str) -> TargetHost {
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    match unbracketed.parse() {
        Ok(ip) => TargetHost::Ip(ip),
        Err(_) => TargetHost::Name(unbracketed.to_owned()),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TargetError {
    #[error(
        "tunnel: invalid tunnel server address {0:?} (expected host:port or an http/https URL)"
    )]
    InvalidAddress(String),
    #[error("tunnel: invalid tunnel server URL {0:?}")]
    InvalidUrl(String),
    #[error("tunnel: tunnel server address {0:?} is missing a host")]
    MissingHost(String),
    #[error("tunnel: tunnel server address {0:?} is missing a port")]
    MissingPort(String),
    #[error("tunnel: tunnel server address {0:?} has an invalid port")]
    InvalidPort(String),
    #[error("tunnel: unsupported tunnel server scheme {0:?} (use http or https)")]
    UnsupportedScheme(String),
    #[error("tunnel: tunnel server URL must not contain user information: {0:?}")]
    UserInfo(String),
    #[error("tunnel: tunnel server URL must not have a path, query, or fragment: {0:?}")]
    PathQueryOrFragment(String),
}

/// The result of one discovery refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub(crate) targets: Vec<Target>,
    /// `None` for a fixed explicit target set.
    pub(crate) refresh_after: Option<Duration>,
}

/// System DNS resolver used by both SRV refreshes and explicit hostname dials.
#[derive(Clone)]
pub(crate) struct TargetResolver {
    inner: TokioResolver,
}

impl TargetResolver {
    pub(crate) fn system() -> Result<Self, ResolveError> {
        let builder = TokioResolver::builder_tokio().map_err(ResolveError::Dns)?;
        let inner = builder.build().map_err(ResolveError::Dns)?;
        Ok(Self { inner })
    }

    /// Resolve the current target set.
    ///
    /// An authoritative no-records response becomes a successful empty set,
    /// allowing the supervisor to reconcile old slots away. Transport and
    /// resolver failures remain errors, so the supervisor can retain healthy
    /// existing slots and retry the refresh.
    pub(crate) async fn resolve(&self, discovery: &Discovery) -> Result<Resolution, ResolveError> {
        match discovery {
            Discovery::Explicit(targets) => Ok(Resolution {
                targets: targets.clone(),
                refresh_after: None,
            }),
            Discovery::Srv(name) => self.resolve_srv(name).await,
        }
    }

    /// Resolve all A and AAAA addresses to try for one dial.
    pub(crate) async fn resolve_dial_addresses(
        &self,
        target: &Target,
    ) -> Result<Vec<SocketAddr>, ResolveError> {
        if let Some(address) = target.socket_addr() {
            return Ok(vec![address]);
        }

        let TargetHost::Name(host) = target.host() else {
            unreachable!("IP targets returned above");
        };
        let lookup = self
            .inner
            .lookup_ip(as_fqdn(host))
            .await
            .map_err(ResolveError::Dns)?;
        let mut seen = HashSet::new();
        let addresses = lookup
            .iter()
            .map(|ip| SocketAddr::new(ip, target.port()))
            .filter(|address| seen.insert(*address))
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(ResolveError::NoAddresses(host.clone()));
        }
        Ok(addresses)
    }

    async fn resolve_srv(&self, query_name: &str) -> Result<Resolution, ResolveError> {
        let lookup = match self.inner.srv_lookup(as_fqdn(query_name)).await {
            Ok(lookup) => lookup,
            Err(error) if error.is_no_records_found() => {
                return Ok(Resolution {
                    targets: Vec::new(),
                    refresh_after: Some(refresh_for_negative(&error)),
                });
            }
            Err(error) => return Err(ResolveError::Dns(error)),
        };

        let now = Instant::now();
        let mut refresh_deadline = lookup.valid_until();
        let mut records = lookup
            .answers()
            .iter()
            .filter_map(|record| match &record.data {
                RData::SRV(srv) => Some(SrvRecord {
                    priority: srv.priority,
                    weight: srv.weight,
                    port: srv.port,
                    host: srv.target.to_utf8().trim_end_matches('.').to_owned(),
                }),
                _ => None,
            })
            // A target of `.` explicitly says that the service is unavailable.
            .filter(|record| !record.host.is_empty())
            .collect::<Vec<_>>();
        records.sort_by_key(|record| (record.priority, std::cmp::Reverse(record.weight)));

        let lookups = join_all(records.into_iter().map(|record| async move {
            let result = self.inner.lookup_ip(as_fqdn(&record.host)).await;
            (record, result)
        }))
        .await;

        let mut resolved = Vec::with_capacity(lookups.len());
        for (record, result) in lookups {
            match result {
                Ok(addresses) => {
                    refresh_deadline = refresh_deadline.min(addresses.valid_until());
                    resolved.push((record, addresses.iter().collect::<Vec<_>>()));
                }
                Err(error) if error.is_no_records_found() => {
                    // This SRV target was authoritatively removed. Other records
                    // remain usable and the negative TTL still informs refresh.
                    refresh_deadline = refresh_deadline.min(now + refresh_for_negative(&error));
                    resolved.push((record, Vec::new()));
                }
                Err(error) => return Err(ResolveError::Dns(error)),
            }
        }

        let targets = expand_srv_addresses(query_name, resolved);
        Ok(Resolution {
            targets,
            refresh_after: Some(clamp_refresh(
                refresh_deadline.saturating_duration_since(now),
            )),
        })
    }
}

fn as_fqdn(name: &str) -> String {
    if name.ends_with('.') {
        name.to_owned()
    } else {
        format!("{name}.")
    }
}

#[derive(Clone, Debug)]
struct SrvRecord {
    priority: u16,
    weight: u16,
    port: u16,
    host: String,
}

fn expand_srv_addresses(
    query_name: &str,
    resolved: impl IntoIterator<Item = (SrvRecord, Vec<IpAddr>)>,
) -> Vec<Target> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for (record, addresses) in resolved {
        for address in addresses {
            let target = Target::new(
                TargetHost::Ip(address),
                record.port,
                query_name.to_owned(),
                false,
            );
            if seen.insert(target.clone()) {
                targets.push(target);
            }
        }
    }
    targets
}

fn refresh_for_negative(error: &NetError) -> Duration {
    use hickory_resolver::net::DnsError;

    let ttl = match error {
        NetError::Dns(DnsError::NoRecordsFound(no_records)) => no_records
            .negative_ttl
            .map(|ttl| Duration::from_secs(ttl.into())),
        _ => None,
    };
    clamp_refresh(ttl.unwrap_or(DNS_REFRESH_FALLBACK))
}

fn clamp_refresh(ttl: Duration) -> Duration {
    ttl.clamp(DNS_REFRESH_FLOOR, DNS_REFRESH_CAP)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ResolveError {
    #[error("tunnel: DNS resolution failed: {0}")]
    Dns(#[source] NetError),
    #[error("tunnel: hostname {0:?} has no A or AAAA addresses")]
    NoAddresses(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parses_explicit_targets_and_plaintext() {
        let target = parse_server_address("example.com:9080").unwrap();
        assert_eq!(target.host(), &TargetHost::Name("example.com".into()));
        assert_eq!(target.port(), 9080);
        assert_eq!(target.server_name(), "example.com");
        assert!(!target.is_plaintext());

        let target = parse_server_address("https://example.com").unwrap();
        assert_eq!(target.port(), 443);
        assert!(!target.is_plaintext());

        let target = parse_server_address("http://127.0.0.1:19080/").unwrap();
        assert_eq!(
            target.socket_addr(),
            Some("127.0.0.1:19080".parse().unwrap())
        );
        assert!(target.is_plaintext());

        let target = parse_server_address("[::1]:9080").unwrap();
        assert_eq!(target.socket_addr(), Some("[::1]:9080".parse().unwrap()));
    }

    #[test]
    fn rejects_malformed_explicit_targets() {
        for address in [
            "example.com",
            "example.com:0",
            "example.com:65536",
            "https://",
            "https://example.com:0",
            "ftp://example.com:21",
            "https://user@example.com",
            "https://example.com/path",
            "https://example.com?query",
            "https://example.com#fragment",
        ] {
            assert!(
                parse_server_address(address).is_err(),
                "unexpectedly accepted {address}"
            );
        }
    }

    #[test]
    fn expands_every_address_and_deduplicates_complete_targets() {
        let record_a = SrvRecord {
            priority: 0,
            weight: 5,
            port: 19080,
            host: "node-a.internal".into(),
        };
        let record_b = SrvRecord {
            priority: 10,
            weight: 1,
            port: 19081,
            host: "node-b.internal".into(),
        };
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);

        let targets = expand_srv_addresses(
            "tunnel.eu.restate.cloud",
            [
                (record_a.clone(), vec![v4, v6, v4]),
                (record_a, vec![v4]),
                (record_b, vec![v4]),
            ],
        );

        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].socket_addr(), Some(SocketAddr::new(v4, 19080)));
        assert_eq!(targets[1].socket_addr(), Some(SocketAddr::new(v6, 19080)));
        assert_eq!(targets[2].socket_addr(), Some(SocketAddr::new(v4, 19081)));
        assert!(
            targets
                .iter()
                .all(|target| target.server_name() == "tunnel.eu.restate.cloud")
        );
    }

    #[test]
    fn target_identity_includes_tls_metadata() {
        let base = Target::new(
            TargetHost::Ip(Ipv4Addr::LOCALHOST.into()),
            443,
            "one.example".into(),
            false,
        );
        let different_sni = Target::new(base.host.clone(), base.port, "two.example".into(), false);
        let plaintext = Target::new(base.host.clone(), base.port, base.server_name.clone(), true);
        let set = HashSet::from([base, different_sni, plaintext]);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn refresh_ttl_has_a_nonzero_floor_and_thirty_second_cap() {
        assert_eq!(clamp_refresh(Duration::ZERO), DNS_REFRESH_FLOOR);
        assert_eq!(
            clamp_refresh(Duration::from_secs(7)),
            Duration::from_secs(7)
        );
        assert_eq!(clamp_refresh(Duration::from_secs(300)), DNS_REFRESH_CAP);
    }

    #[test]
    fn authoritative_empty_and_transport_errors_have_distinct_taxonomy() {
        use hickory_resolver::net::NoRecords;
        use hickory_resolver::proto::op::{Query, ResponseCode};
        use hickory_resolver::proto::rr::{Name, RecordType};

        let query = Query::query(Name::from_ascii("missing.example.").unwrap(), RecordType::A);
        let mut no_records = NoRecords::new(query, ResponseCode::NXDomain);
        no_records.negative_ttl = Some(7);
        let authoritative_empty = NetError::from(no_records);
        assert!(authoritative_empty.is_no_records_found());
        assert_eq!(
            refresh_for_negative(&authoritative_empty),
            Duration::from_secs(7)
        );

        let transport = NetError::Timeout;
        assert!(!transport.is_no_records_found());
        assert_eq!(refresh_for_negative(&transport), DNS_REFRESH_FALLBACK);
    }
}
