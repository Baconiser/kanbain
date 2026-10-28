//! Egress proxy for sandbox containers (D38).
//!
//! Containers get `HTTPS_PROXY=http://<run>:<token>@kanbain-gateway:4001`.
//! Only `CONNECT host:443` to allowlisted hosts is tunnelled; everything else
//! (other hosts, other ports, plain HTTP, hosts resolving to private
//! addresses) is refused and reported to the [`EgressPolicy`] so it can be
//! logged as an event on the run.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use base64::Engine;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// Hosts reachable by default: package registries and GitHub release
/// downloads. `*.example.com` matches subdomains only, `example.com` only
/// itself.
pub const DEFAULT_ALLOWLIST: &[&str] = &[
    // npm / yarn
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    // Rust
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "static.rust-lang.org",
    // Python
    "pypi.org",
    "files.pythonhosted.org",
    // Go
    "proxy.golang.org",
    "sum.golang.org",
    // Docker Hub
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
    // GitHub release downloads
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

const MAX_HEAD_BYTES: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Hostname allowlist with `*.` wildcard entries.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    entries: Vec<String>,
}

impl Allowlist {
    pub fn new<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            entries: entries
                .into_iter()
                .map(|e| e.as_ref().trim().trim_end_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
        }
    }

    pub fn with_defaults<I, S>(extra: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut list = Self::new(DEFAULT_ALLOWLIST);
        list.entries.extend(Self::new(extra).entries);
        list
    }

    pub fn allows(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.entries
            .iter()
            .any(|entry| match entry.strip_prefix("*.") {
                Some(suffix) => host.len() > suffix.len() && host.ends_with(&format!(".{suffix}")),
                None => *entry == host,
            })
    }
}

/// Outcome of checking one proxy request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressVerdict {
    /// Credentials missing or unknown.
    Unauthorized,
    Allowed,
    Blocked {
        reason: String,
    },
}

/// Decides and records egress for runs. Implementations map the proxy
/// credentials to a run and log blocked attempts.
#[async_trait]
pub trait EgressPolicy: Send + Sync {
    /// Check a `CONNECT host:port` (or a plain HTTP request when `plain_http`).
    async fn check(
        &self,
        user: &str,
        token: &str,
        host: &str,
        port: u16,
        plain_http: bool,
    ) -> EgressVerdict;

    /// Report a request refused after `check` allowed it (for example
    /// because the host resolved to a private address).
    async fn report_blocked(&self, user: &str, host: &str, port: u16, reason: &str);
}

/// Bind the egress proxy on `ip` with an OS-assigned port.
pub async fn bind(ip: IpAddr) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::new(ip, 0)).await
}

pub async fn serve(listener: TcpListener, policy: Arc<dyn EgressPolicy>) {
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("Egress proxy accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let policy = policy.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, policy).await {
                tracing::debug!("Egress proxy connection ended: {e}");
            }
        });
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RequestHead {
    method: String,
    target: String,
    credentials: Option<(String, String)>,
}

async fn handle(mut client: TcpStream, policy: Arc<dyn EgressPolicy>) -> std::io::Result<()> {
    let (head, rest) = match read_head(&mut client).await? {
        Some(parsed) => parsed,
        None => return respond(&mut client, 400, "Bad Request").await,
    };
    let Some((user, token)) = head.credentials.clone() else {
        return respond_auth_required(&mut client).await;
    };

    let plain_http = head.method != "CONNECT";
    let Some((host, port)) = (if plain_http {
        parse_absolute_url(&head.target)
    } else {
        parse_authority(&head.target)
    }) else {
        return respond(&mut client, 400, "Bad Request").await;
    };

    match policy.check(&user, &token, &host, port, plain_http).await {
        EgressVerdict::Unauthorized => return respond_auth_required(&mut client).await,
        EgressVerdict::Blocked { reason } => {
            return respond(&mut client, 403, &format!("Blocked by KanbAIn: {reason}")).await;
        }
        EgressVerdict::Allowed if plain_http => {
            return respond(&mut client, 403, "Blocked by KanbAIn: plain HTTP").await;
        }
        EgressVerdict::Allowed => {}
    }

    let mut upstream = match connect_public(&host, port).await {
        Ok(stream) => stream,
        Err(reason) => {
            policy.report_blocked(&user, &host, port, &reason).await;
            return respond(&mut client, 403, &format!("Blocked by KanbAIn: {reason}")).await;
        }
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    if !rest.is_empty() {
        upstream.write_all(&rest).await?;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Resolve `host` and connect, refusing loopback, private and link-local
/// addresses so allowlisted names can't be pointed at the host or the LAN.
async fn connect_public(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .collect();
    let public: Vec<SocketAddr> = addrs.into_iter().filter(|a| is_public(a.ip())).collect();
    if public.is_empty() {
        return Err(format!("{host} resolves to a non-public address"));
    }
    let mut last_err = String::new();
    for addr in public {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => last_err = e.to_string(),
            Err(_) => last_err = "connect timed out".into(),
        }
    }
    Err(format!("cannot connect to {host}:{port}: {last_err}"))
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                // carrier-grade NAT 100.64.0.0/10
                || (a == 100 && (64..128).contains(&b))
                || a == 0)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // unique local fc00::/7 and link-local fe80::/10
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// Read the request head; returns it plus any bytes read past it.
async fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<(RequestHead, Vec<u8>)>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "head timeout"))??;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = find_head_end(&buf) {
            let rest = buf.split_off(end);
            return Ok(parse_head(&buf).map(|head| (head, rest)));
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Ok(None);
        }
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn parse_head(buf: &[u8]) -> Option<RequestHead> {
    let text = std::str::from_utf8(buf).ok()?;
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let credentials = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("proxy-authorization"))
        .and_then(|(_, value)| parse_basic(value.trim()));
    Some(RequestHead {
        method,
        target,
        credentials,
    })
}

fn parse_basic(value: &str) -> Option<(String, String)> {
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

fn parse_authority(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

fn parse_absolute_url(target: &str) -> Option<(String, u16)> {
    let rest = target.strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    let authority = authority.rsplit('@').next()?;
    match parse_authority(authority) {
        Some(parsed) => Some(parsed),
        None if !authority.is_empty() => Some((authority.to_string(), 80)),
        None => None,
    }
}

async fn respond(stream: &mut TcpStream, code: u16, reason: &str) -> std::io::Result<()> {
    let body = format!("{reason}\n");
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

async fn respond_auth_required(stream: &mut TcpStream) -> std::io::Result<()> {
    let response = "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"kanbain\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn allowlist_matching() {
        let list = Allowlist::with_defaults(["*.example.com", "Exact.ORG."]);
        assert!(list.allows("registry.npmjs.org"));
        assert!(list.allows("api.example.com"));
        assert!(!list.allows("example.com"));
        assert!(!list.allows("evilexample.com"));
        assert!(list.allows("exact.org"));
        assert!(!list.allows("sub.exact.org"));
        assert!(!list.allows("example.org"));
    }

    #[test]
    fn rejects_private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.20.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public("93.184.216.34".parse().unwrap()));
        assert!(is_public("2606:4700::1".parse().unwrap()));
    }

    #[test]
    fn parses_connect_head() {
        let auth = base64::engine::general_purpose::STANDARD.encode("run1:secret");
        let head = format!(
            "CONNECT registry.npmjs.org:443 HTTP/1.1\r\nHost: registry.npmjs.org:443\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
        );
        let parsed = parse_head(head.as_bytes()).unwrap();
        assert_eq!(parsed.method, "CONNECT");
        assert_eq!(
            parse_authority(&parsed.target),
            Some(("registry.npmjs.org".into(), 443))
        );
        assert_eq!(
            parsed.credentials,
            Some(("run1".to_string(), "secret".to_string()))
        );
        assert_eq!(
            parse_absolute_url("http://example.com/path"),
            Some(("example.com".into(), 80))
        );
        assert_eq!(
            parse_absolute_url("http://example.com:8080/"),
            Some(("example.com".into(), 8080))
        );
    }

    struct TestPolicy {
        blocked: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl EgressPolicy for TestPolicy {
        async fn check(
            &self,
            user: &str,
            token: &str,
            host: &str,
            _port: u16,
            _plain_http: bool,
        ) -> EgressVerdict {
            if user != "run" || token != "ok" {
                return EgressVerdict::Unauthorized;
            }
            if host == "allowed.test" {
                EgressVerdict::Allowed
            } else {
                self.blocked.lock().unwrap().push(host.to_string());
                EgressVerdict::Blocked {
                    reason: format!("{host} is not allowlisted"),
                }
            }
        }

        async fn report_blocked(&self, _user: &str, host: &str, _port: u16, _reason: &str) {
            self.blocked.lock().unwrap().push(host.to_string());
        }
    }

    async fn send(addr: SocketAddr, request: String) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn proxy_enforces_auth_and_allowlist() {
        let listener = bind(IpAddr::from([127, 0, 0, 1])).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let policy = Arc::new(TestPolicy {
            blocked: Mutex::new(vec![]),
        });
        tokio::spawn(serve(listener, policy.clone()));

        let no_auth = send(addr, "CONNECT example.com:443 HTTP/1.1\r\n\r\n".into()).await;
        assert!(no_auth.starts_with("HTTP/1.1 407"), "{no_auth}");

        let auth = base64::engine::general_purpose::STANDARD.encode("run:ok");
        let blocked = send(
            addr,
            format!(
                "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
            ),
        )
        .await;
        assert!(blocked.starts_with("HTTP/1.1 403"), "{blocked}");

        // Allowed by policy, but the name does not resolve to a public address.
        let private = send(
            addr,
            format!(
                "CONNECT allowed.test:443 HTTP/1.1\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
            ),
        )
        .await;
        assert!(private.starts_with("HTTP/1.1 403"), "{private}");

        assert_eq!(
            *policy.blocked.lock().unwrap(),
            vec!["example.com".to_string(), "allowed.test".to_string()]
        );
    }
}
