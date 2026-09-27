//! The HTTP/1.1 upgrade that starts a WebSocket connection (RFC 6455, section 4).
//!
//! The dialer sends a browser-like upgrade request. The listener answers `101` only to
//! a well-formed upgrade for the configured path (and host, when set); everything else,
//! including plain HTTP requests from probes, gets the `404` page of a stock nginx.

use std::io;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::prelude::{Engine, BASE64_STANDARD};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::debug;

use crate::config::WsConfig;

/// Request or response heads longer than this are rejected.
pub const MAX_HEAD: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `User-Agent` sent when `tunnel.ws.user_agent` is not set: a current desktop Chrome.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Dialing side: what the upgrade request looks like.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The request up to (not including) the `Sec-WebSocket-Key` line, which is random
    /// per connection and goes last.
    head: String,
}

impl ClientConfig {
    /// `remote` is the address dialed; its host (without a default port) is the `Host`
    /// header unless `tunnel.ws.host` is set. `scheme` is `http` or `https`.
    pub fn new(ws: Option<&WsConfig>, remote: &str, scheme: &str, default_port: u16) -> Self {
        let host = match ws.and_then(|w| w.host.clone()) {
            Some(host) => host,
            None => match split_port(remote) {
                (host, Some(port)) if port == default_port.to_string() => host.to_owned(),
                _ => remote.to_owned(),
            },
        };
        let path = ws.map_or("/", |w| w.path.as_str());
        let user_agent = ws
            .and_then(|w| w.user_agent.as_deref())
            .unwrap_or(DEFAULT_USER_AGENT);
        let extra: Vec<(&str, &str)> = ws
            .map(|w| {
                w.headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        // Header order of a Chrome upgrade request. Configured headers replace the
        // browser defaults of the same name and otherwise go at the end.
        let origin = format!("{scheme}://{host}");
        let defaults: [(&str, &str); 10] = [
            ("Host", &host),
            ("Connection", "Upgrade"),
            ("Pragma", "no-cache"),
            ("Cache-Control", "no-cache"),
            ("User-Agent", user_agent),
            ("Upgrade", "websocket"),
            ("Origin", &origin),
            ("Sec-WebSocket-Version", "13"),
            ("Accept-Encoding", "gzip, deflate, br, zstd"),
            ("Accept-Language", "en-US,en;q=0.9"),
        ];
        let is_default = |name: &str| defaults.iter().any(|(d, _)| d.eq_ignore_ascii_case(name));
        let mut head = format!("GET {path} HTTP/1.1\r\n");
        for (name, value) in defaults {
            let (name, value) = extra
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .copied()
                .unwrap_or((name, value));
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        for (name, value) in extra.iter().filter(|(k, _)| !is_default(k)) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        Self { head }
    }
}

/// Dialing side: sends the upgrade request on `stream` and checks the answer. Returns
/// the bytes that arrived after the response head (the start of the first frames).
pub async fn connect<S>(stream: &mut S, config: &ClientConfig) -> io::Result<Vec<u8>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let key = BASE64_STANDARD.encode(crate::crypto::random_bytes::<16>()?);
    let request = format!("{}Sec-WebSocket-Key: {key}\r\n\r\n", config.head);
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut buf = Vec::with_capacity(1024);
    let head_len = read_head(stream, &mut buf).await?;
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut response = httparse::Response::new(&mut headers);
    match response.parse(&buf[..head_len]) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => return Err(invalid("malformed HTTP response to the websocket upgrade")),
    }
    let code = response.code.unwrap_or(0);
    if code != 101 {
        let reason = response.reason.unwrap_or("");
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "websocket upgrade refused with HTTP {code} {reason} \
                 (check tunnel.ws.path and tunnel.ws.host on both sides)"
            ),
        ));
    }
    let headers = response.headers;
    if !has_token(headers, "upgrade", "websocket") || !has_token(headers, "connection", "upgrade") {
        return Err(invalid("HTTP 101 response is not a websocket upgrade"));
    }
    if header(headers, "sec-websocket-accept") != Some(accept_key(&key).as_bytes()) {
        return Err(invalid(
            "wrong Sec-WebSocket-Accept in the upgrade response",
        ));
    }
    Ok(buf.split_off(head_len))
}

/// Listening side: which upgrade requests are accepted.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    path: String,
    /// When set, the `Host` header must match (case-insensitive; the port is only
    /// compared when this includes one).
    host: Option<String>,
}

impl ServerConfig {
    pub fn new(ws: Option<&WsConfig>) -> Self {
        Self {
            path: ws.map_or_else(|| "/".to_owned(), |w| w.path.clone()),
            host: ws.and_then(|w| w.host.clone()),
        }
    }
}

/// Why a request was not upgraded; only logged, the client just sees an nginx page.
enum Reject {
    BadRequest(&'static str),
    NotFound(&'static str),
}

/// Listening side: reads the upgrade request from `stream` and answers it. Returns the
/// bytes that arrived after the request head. Anything but a valid upgrade for our path
/// is answered with an nginx-style error page and fails with `PermissionDenied`.
///
/// Callers bound the time this takes (the handshake timeout).
pub async fn accept<S>(
    stream: &mut S,
    config: &ServerConfig,
    peer: SocketAddr,
) -> io::Result<Vec<u8>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = Vec::with_capacity(1024);
    let verdict = match read_head(stream, &mut buf).await {
        Ok(head_len) => check_request(&buf[..head_len], config).map(|ok| (ok, head_len)),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            Err(Reject::BadRequest("request head too large"))
        }
        Err(e) => return Err(e),
    };
    let ((key, client), head_len) = match verdict {
        Ok(v) => v,
        Err(reject) => {
            let (response, reason) = match reject {
                Reject::BadRequest(reason) => (error_page(400, "Bad Request"), reason),
                Reject::NotFound(reason) => (error_page(404, "Not Found"), reason),
            };
            debug!(%peer, reason, "websocket upgrade rejected");
            // Best effort: the peer may already be gone.
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("not a valid websocket upgrade: {reason}"),
            ));
        }
    };
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nServer: nginx\r\nDate: {}\r\n\
         Connection: upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        http_date(SystemTime::now()),
        accept_key(&key)
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    debug!(%peer, client = client.as_deref().unwrap_or("-"), "websocket upgrade accepted");
    Ok(buf.split_off(head_len))
}

/// Checks an upgrade request head. Returns the `Sec-WebSocket-Key` and the real client
/// address a CDN or proxy reports, if any (for logs).
fn check_request(head: &[u8], config: &ServerConfig) -> Result<(String, Option<String>), Reject> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(head) {
        Ok(httparse::Status::Complete(_)) if request.version == Some(1) => {}
        _ => return Err(Reject::BadRequest("malformed HTTP/1.1 request")),
    }
    let headers = request.headers;
    let host = header(headers, "host").ok_or(Reject::BadRequest("no Host header"))?;
    let path = request.path.unwrap_or("");
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    if path != config.path {
        return Err(Reject::NotFound("wrong path"));
    }
    if let Some(expected) = &config.host {
        let host = std::str::from_utf8(host).unwrap_or("");
        if !host_matches(expected, host) {
            return Err(Reject::NotFound("wrong host"));
        }
    }
    if request.method != Some("GET")
        || !has_token(headers, "upgrade", "websocket")
        || !has_token(headers, "connection", "upgrade")
        || header(headers, "sec-websocket-version") != Some(b"13")
    {
        return Err(Reject::NotFound("not a websocket upgrade"));
    }
    let key = header(headers, "sec-websocket-key")
        .and_then(|k| std::str::from_utf8(k).ok())
        .filter(|k| BASE64_STANDARD.decode(k).is_ok_and(|d| d.len() == 16))
        .ok_or(Reject::NotFound("bad Sec-WebSocket-Key"))?;
    let client = header(headers, "cf-connecting-ip")
        .or_else(|| header(headers, "x-forwarded-for"))
        .or_else(|| header(headers, "x-real-ip"))
        .and_then(|v| std::str::from_utf8(v).ok())
        .map(|v| v.split(',').next().unwrap_or("").trim().to_owned());
    Ok((key.to_owned(), client))
}

/// Reads until the end of an HTTP head (`\r\n\r\n`). Returns the head length; `buf`
/// may hold more bytes than that. Fails with `InvalidData` past [`MAX_HEAD`].
async fn read_head<S: AsyncRead + Unpin>(stream: &mut S, buf: &mut Vec<u8>) -> io::Result<usize> {
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Ok(i + 4);
        }
        if buf.len() >= MAX_HEAD {
            return Err(invalid("HTTP head too large"));
        }
        let room = (MAX_HEAD - buf.len()).min(chunk.len());
        let n = stream.read(&mut chunk[..room]).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed during the websocket upgrade",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    let mut input = key.as_bytes().to_vec();
    input.extend_from_slice(WS_GUID.as_bytes());
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &input);
    BASE64_STANDARD.encode(digest.as_ref())
}

fn header<'a>(headers: &[httparse::Header<'a>], name: &str) -> Option<&'a [u8]> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value)
}

/// Whether any `name` header lists `token` (comma separated, case-insensitive).
fn has_token(headers: &[httparse::Header<'_>], name: &str, token: &str) -> bool {
    headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(name))
        .filter_map(|h| std::str::from_utf8(h.value).ok())
        .flat_map(|v| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// Splits `host:port` (also `[v6]:port`); the port is `None` when there is none.
pub fn split_port(authority: &str) -> (&str, Option<&str>) {
    let (host, port) = if authority.starts_with('[') {
        match authority.find("]:") {
            Some(i) => (&authority[..=i], Some(&authority[i + 2..])),
            None => (authority, None),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h, Some(p)),
            _ => (authority, None),
        }
    };
    match port {
        Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (host, Some(p)),
        _ => (authority, None),
    }
}

fn host_matches(expected: &str, got: &str) -> bool {
    match split_port(expected) {
        (_, Some(_)) => got.eq_ignore_ascii_case(expected),
        (expected, None) => split_port(got).0.eq_ignore_ascii_case(expected),
    }
}

/// The error page of a stock nginx (`server_tokens off`), headers included.
fn error_page(code: u16, reason: &str) -> String {
    let body = format!(
        "<html>\r\n<head><title>{code} {reason}</title></head>\r\n<body>\r\n\
         <center><h1>{code} {reason}</h1></center>\r\n<hr><center>nginx</center>\r\n\
         </body>\r\n</html>\r\n"
    );
    format!(
        "HTTP/1.1 {code} {reason}\r\nServer: nginx\r\nDate: {}\r\nContent-Type: text/html\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        http_date(SystemTime::now()),
        body.len()
    )
}

/// IMF-fixdate (RFC 9110), e.g. `Sun, 06 Nov 1994 08:49:37 GMT`.
pub fn http_date(time: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[(days % 7) as usize],
        MONTHS[month as usize - 1],
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rfc6455_accept_key() {
        // Section 1.3.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn http_dates() {
        let at = |s| http_date(UNIX_EPOCH + Duration::from_secs(s));
        assert_eq!(at(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(at(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(at(951_782_400), "Tue, 29 Feb 2000 00:00:00 GMT");
        assert_eq!(at(4_107_542_399), "Sun, 28 Feb 2100 23:59:59 GMT");
        assert_eq!(at(1_790_547_845), "Sun, 27 Sep 2026 22:24:05 GMT");
    }

    #[test]
    fn nginx_404_page() {
        let page = error_page(404, "Not Found");
        let (head, body) = page.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 404 Not Found\r\nServer: nginx\r\n"));
        assert!(head.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert_eq!(body.len(), 146);
    }

    #[test]
    fn ports_and_hosts() {
        assert_eq!(split_port("a.example:443"), ("a.example", Some("443")));
        assert_eq!(split_port("a.example"), ("a.example", None));
        assert_eq!(split_port("[::1]:80"), ("[::1]", Some("80")));
        assert_eq!(split_port("[::1]"), ("[::1]", None));
        assert_eq!(split_port("::1"), ("::1", None));
        assert!(host_matches("a.example", "A.Example:8080"));
        assert!(host_matches("a.example:8080", "a.example:8080"));
        assert!(!host_matches("a.example:8080", "a.example:80"));
        assert!(!host_matches("a.example", "b.example"));
    }

    fn request_head(config: &ClientConfig) -> String {
        format!(
            "{}Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
            config.head
        )
    }

    #[test]
    fn client_request_defaults() {
        let c = ClientConfig::new(None, "203.0.113.1:80", "http", 80);
        let head = request_head(&c);
        assert!(head.starts_with("GET / HTTP/1.1\r\nHost: 203.0.113.1\r\nConnection: Upgrade\r\n"));
        assert!(head.contains("\r\nOrigin: http://203.0.113.1\r\n"));
        assert!(head.contains(&format!("\r\nUser-Agent: {DEFAULT_USER_AGENT}\r\n")));
        let c = ClientConfig::new(None, "203.0.113.1:8080", "http", 80);
        assert!(c.head.contains("\r\nHost: 203.0.113.1:8080\r\n"));
    }

    #[test]
    fn client_request_with_config() {
        let ws: WsConfig = toml::from_str(
            r#"
            path = "/api/v1/stream"
            host = "cdn.example"
            user_agent = "UA/1"
            headers = { "accept-language" = "fa-IR", "X-Extra" = "1" }
            "#,
        )
        .unwrap();
        let c = ClientConfig::new(Some(&ws), "104.16.0.1:80", "http", 80);
        let head = request_head(&c);
        assert!(head.starts_with("GET /api/v1/stream HTTP/1.1\r\nHost: cdn.example\r\n"));
        assert!(head.contains("\r\nUser-Agent: UA/1\r\n"));
        // Replaced in place, not sent twice.
        assert_eq!(
            head.to_ascii_lowercase().matches("accept-language").count(),
            1
        );
        assert!(head.contains("\r\naccept-language: fa-IR\r\n"));
        assert!(head.contains("\r\nX-Extra: 1\r\nSec-WebSocket-Key: "));

        // What the dialer sends is what the listener accepts.
        let server = ServerConfig::new(Some(&ws));
        assert!(check_request(head.as_bytes(), &server).is_ok());
    }

    #[test]
    fn server_checks() {
        let ws: WsConfig = toml::from_str("path = \"/p\"\nhost = \"a.example\"").unwrap();
        let server = ServerConfig::new(Some(&ws));
        let ok = request_head(&ClientConfig::new(Some(&ws), "1.2.3.4:80", "http", 80));
        let (key, client) = check_request(ok.as_bytes(), &server).ok().unwrap();
        assert_eq!(key, "dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(client, None);

        let with_cf = ok.replace("\r\n\r\n", "\r\nCF-Connecting-IP: 198.51.100.7\r\n\r\n");
        let (_, client) = check_request(with_cf.as_bytes(), &server).ok().unwrap();
        assert_eq!(client.as_deref(), Some("198.51.100.7"));
        let with_xff = ok.replace(
            "\r\n\r\n",
            "\r\nX-Forwarded-For: 198.51.100.8, 10.0.0.1\r\n\r\n",
        );
        let (_, client) = check_request(with_xff.as_bytes(), &server).ok().unwrap();
        assert_eq!(client.as_deref(), Some("198.51.100.8"));

        // Query strings are ignored, as with most servers.
        assert!(check_request(ok.replace("/p ", "/p?x=1 ").as_bytes(), &server).is_ok());

        for (bad, why) in [
            (ok.replace("GET /p ", "GET /q "), "path"),
            (ok.replace("GET /p ", "GET /p/ "), "path prefix"),
            (ok.replace("GET ", "POST "), "method"),
            (ok.replace("Host: a.example", "Host: b.example"), "host"),
            (ok.replace("Upgrade: websocket", "Upgrade: h2c"), "upgrade"),
            (
                ok.replace("Connection: Upgrade", "Connection: keep-alive"),
                "connection",
            ),
            (ok.replace("Version: 13", "Version: 8"), "version"),
            (ok.replace("dGhlIHNhbXBsZSBub25jZQ==", "short"), "key"),
            (
                "GET /p HTTP/1.1\r\nHost: a.example\r\n\r\n".to_owned(),
                "plain GET",
            ),
        ] {
            assert!(
                matches!(
                    check_request(bad.as_bytes(), &server),
                    Err(Reject::NotFound(_))
                ),
                "{why}"
            );
        }
        for bad in [
            "garbage\r\n\r\n",
            "GET /p HTTP/1.0\r\nHost: a.example\r\n\r\n",
            "GET /p HTTP/1.1\r\n\r\n",
        ] {
            assert!(
                matches!(
                    check_request(bad.as_bytes(), &server),
                    Err(Reject::BadRequest(_))
                ),
                "{bad}"
            );
        }
    }
}
