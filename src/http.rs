use std::fmt;
use std::io::Read;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig};

use crate::redact::Secret;

pub const USER_AGENT: &str = concat!("myip/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Any,
    V4,
    V6,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProxyMode {
    /// Ignore proxy environment variables (the behavior of every release so far).
    #[default]
    None,
    /// Honor `HTTPS_PROXY`, `ALL_PROXY` and friends.
    Env,
    Url(String),
}

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub timeout: Duration,
    pub max_bytes: usize,
    pub family: Family,
    pub proxy: ProxyMode,
    pub accept: &'static str,
    /// Sent as `Authorization: Bearer` on the first request only, never on a followed
    /// redirect. `Secret` keeps this derived `Debug` from printing it.
    pub bearer: Option<Secret>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Carries the transport's own message (for ureq, `timeout: global`).
    Timeout(String),
    TooLarge(usize),
    /// The host name could not be resolved: no working DNS, or no such name. Carries the
    /// transport's own message.
    Resolve(String),
    /// The host name resolved, but to no address of the family that `-4` or `-6` asks for.
    /// Carries the transport's own message (for ureq, `host not found`).
    NoAddress(String),
    /// The server answered, so the status line and headers arrived, but the answer was refused
    /// or cut short: a redirect that is not followed, too many redirects, or a body that
    /// could not be read. Carries the transport's own message.
    Rejected(String),
    Transport(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(msg)
            | Self::Resolve(msg)
            | Self::NoAddress(msg)
            | Self::Rejected(msg)
            | Self::Transport(msg) => f.write_str(msg),
            Self::TooLarge(limit) => {
                write!(f, "the response body is larger than request limit: {limit}")
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// HTTP GET, the only network seam the rest of the crate depends on.
/// Non-2xx statuses are returned as a `Response`, not an error.
/// Implementations are shared between threads: `myip public --verify` asks every provider
/// at once.
pub trait Fetch: Sync {
    fn get(&self, url: &str, opts: &FetchOptions) -> Result<Response, FetchError>;
}

pub struct UreqFetch;

impl Fetch for UreqFetch {
    fn get(&self, url: &str, opts: &FetchOptions) -> Result<Response, FetchError> {
        let proxy = match &opts.proxy {
            ProxyMode::None => Some(None),
            ProxyMode::Env => None,
            ProxyMode::Url(u) => Some(Some(ureq::Proxy::new(u).map_err(map_error)?)),
        };
        let mut builder = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .user_agent(USER_AGENT)
            .timeout_global(Some(opts.timeout))
            .ip_family(match opts.family {
                Family::Any => ureq::config::IpFamily::Any,
                Family::V4 => ureq::config::IpFamily::Ipv4Only,
                Family::V6 => ureq::config::IpFamily::Ipv6Only,
            })
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            );
        if let Some(proxy) = proxy {
            builder = builder.proxy(proxy);
        }
        let agent: ureq::Agent = builder.build().into();

        let authorization = opts.bearer.as_ref().map(bearer_header).transpose()?;
        let mut current = url.to_owned();
        let mut hops = 0;
        let mut response = loop {
            let mut request = agent.get(&current).header("Accept", opts.accept);
            // Only the first request carries the token: a followed redirect never does.
            if let Some(value) = authorization.as_ref().filter(|_| hops == 0) {
                request = request.header("Authorization", value.clone());
            }
            let response = request.call().map_err(map_error)?;
            let location = match response.status().as_u16() {
                301 | 302 | 303 | 307 | 308 => response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned),
                _ => None,
            };
            let Some(location) = location else {
                break response;
            };
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(FetchError::Rejected("too many redirects".to_owned()));
            }
            current = same_origin_target(&current, &location)?;
        };
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // Read one byte past the limit so an oversized body is detected, not truncated.
        let mut raw = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(opts.max_bytes as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|e| match map_error(ureq::Error::from(e)) {
                // The headers arrived: a timeout keeps its own reason, anything else is an
                // answer that could not be read.
                FetchError::Transport(message) => FetchError::Rejected(message),
                other => other,
            })?;
        if raw.len() > opts.max_bytes {
            return Err(FetchError::TooLarge(opts.max_bytes));
        }
        Ok(Response {
            status,
            content_type,
            body: String::from_utf8_lossy(&raw).into_owned(),
        })
    }
}

/// `Bearer <token>` as a header value, marked sensitive so that even the `Debug` of the
/// header hides it. The error never carries the token.
fn bearer_header(token: &Secret) -> Result<ureq::http::HeaderValue, FetchError> {
    let mut value = ureq::http::HeaderValue::from_str(&format!("Bearer {}", token.expose()))
        .map_err(|_| {
            FetchError::Rejected("the access token cannot be sent as a header value".to_owned())
        })?;
    value.set_sensitive(true);
    Ok(value)
}

fn map_error(e: ureq::Error) -> FetchError {
    match e {
        ureq::Error::Timeout(_) => FetchError::Timeout(e.to_string()),
        ureq::Error::Io(ref io) if is_lookup_failure(io) => FetchError::Resolve(e.to_string()),
        ureq::Error::HostNotFound => FetchError::NoAddress(e.to_string()),
        other => FetchError::Transport(other.to_string()),
    }
}

/// ureq reports a failed lookup as a plain `io::Error` with no variant of its own (its
/// `HostNotFound` only means that no address of the wanted family was found). On Unix, std
/// builds that error from this text and libc's wording, with no OS code. Windows reports
/// a Winsock code instead, which is not recognised, so there the run is not cut short.
fn is_lookup_failure(error: &std::io::Error) -> bool {
    error
        .to_string()
        .starts_with("failed to lookup address information")
}

const MAX_REDIRECTS: usize = 5;

/// Tells the user, after a failed host lookup, that `-4` or `-6` also restricts the proxy
/// host. The proxy host is the one connected to, so with a proxy URL it is the one resolved
/// and the one that has to have an address of the wanted family. With no proxy, or one from
/// the environment, we cannot tell that it is in play, so nothing is said.
pub fn proxy_family_hint(error: &FetchError, options: &FetchOptions) -> Option<String> {
    if !matches!(error, FetchError::Resolve(_) | FetchError::NoAddress(_)) {
        return None;
    }
    if !matches!(options.proxy, ProxyMode::Url(_)) {
        return None;
    }
    let (flag, example) = match options.family {
        Family::V6 => ("-6", "http://[::1]:PORT"),
        Family::V4 => ("-4", "http://127.0.0.1:PORT"),
        Family::Any => return None,
    };
    Some(format!(
        "{flag} also applies to the proxy host; give the proxy an address of that family, \
         such as {example}, or drop {flag}"
    ))
}

/// Resolves a `Location` against `base`, accepting only a target on the same scheme
/// and authority (ADR 0001: no cross-host redirects, no downgrade to http). Relative
/// references are resolved per RFC 3986 section 5; the origin of the result is always
/// `base`'s, so a path can never change the host. The target is deliberately not
/// echoed in the error: it is server-controlled and may carry credentials.
fn same_origin_target(base: &str, location: &str) -> Result<String, FetchError> {
    let refused = || FetchError::Rejected("refused a redirect to a different host".to_owned());
    let base: ureq::http::Uri = base.parse().map_err(|_| refused())?;
    let (scheme, authority) = match (base.scheme_str(), base.authority()) {
        (Some(s), Some(a)) => (s, a),
        _ => return Err(refused()),
    };
    if location.starts_with("//") || location.is_empty() {
        return Err(refused());
    }
    if location.starts_with('/') {
        return Ok(format!(
            "{scheme}://{authority}{}",
            normalize_path(location)
        ));
    }
    if !has_scheme(location) {
        return Ok(format!(
            "{scheme}://{authority}{}",
            resolve_relative(&base, location)
        ));
    }
    let target: ureq::http::Uri = location.parse().map_err(|_| refused())?;
    let same = target.scheme_str() == Some(scheme)
        && target
            .authority()
            .is_some_and(|a| a.as_str().eq_ignore_ascii_case(authority.as_str()));
    if !same {
        return Err(refused());
    }
    // `scheme://authority` is kept as received; only what follows is normalized.
    let head_len = location.find("://").map_or(0, |i| i + 3) + authority.as_str().len();
    Ok(format!(
        "{}{}",
        &location[..head_len],
        normalize_path(&location[head_len..])
    ))
}

/// Removes dot segments from the path of `path_and_tail` (a path optionally followed
/// by `?query` and `#fragment`, which are data and stay untouched).
fn normalize_path(path_and_tail: &str) -> String {
    let cut = path_and_tail
        .find(['?', '#'])
        .unwrap_or(path_and_tail.len());
    let (path, tail) = path_and_tail.split_at(cut);
    if path.is_empty() {
        return path_and_tail.to_owned();
    }
    format!("{}{tail}", remove_dot_segments(path))
}

/// Whether `s` starts with `scheme:` (RFC 3986 section 3.1).
fn has_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))) == Some(':')
}

/// The path and query of a scheme-less, authority-less reference resolved against `base`.
fn resolve_relative(base: &ureq::http::Uri, reference: &str) -> String {
    let cut = reference.find(['?', '#']).unwrap_or(reference.len());
    let (path_ref, tail) = reference.split_at(cut);
    let base_path = base.path();
    let path = if path_ref.is_empty() {
        base_path.to_owned()
    } else {
        let dir_end = base_path.rfind('/').map_or(0, |i| i + 1);
        remove_dot_segments(&format!("{}{path_ref}", &base_path[..dir_end]))
    };
    // A fragment-only reference keeps the base query; anything else replaces it.
    let query = match base.query() {
        Some(q) if path_ref.is_empty() && !tail.starts_with('?') => format!("?{q}"),
        _ => String::new(),
    };
    format!("{path}{query}{tail}")
}

/// RFC 3986 section 5.2.4 for a path that starts with `/`.
fn remove_dot_segments(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, segment) in segments.iter().enumerate().skip(1) {
        let last = i == segments.len() - 1;
        match *segment {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                out.pop();
                if last {
                    out.push("");
                }
            }
            other => out.push(other),
        }
    }
    format!("/{}", out.join("/"))
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::HashMap;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use super::*;

    /// Replays canned answers in order and records every request.
    pub struct FakeFetch {
        answers: Mutex<Vec<Result<Response, FetchError>>>,
        pub requests: Mutex<Vec<(String, FetchOptions)>>,
    }

    impl FakeFetch {
        pub fn new(answers: Vec<Result<Response, FetchError>>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().rev().collect()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl Fetch for FakeFetch {
        fn get(&self, url: &str, opts: &FetchOptions) -> Result<Response, FetchError> {
            self.requests
                .lock()
                .unwrap()
                .push((url.to_owned(), opts.clone()));
            self.answers
                .lock()
                .unwrap()
                .pop()
                .expect("unexpected extra request")
        }
    }

    /// Answers by URL, so the result does not depend on the order in which concurrent
    /// callers arrive, and records every request. A URL without a route is a transport error.
    pub struct RouteFetch {
        routes: HashMap<String, Result<Response, FetchError>>,
        pub requests: Mutex<Vec<(String, FetchOptions)>>,
        rendezvous: Option<Rendezvous>,
    }

    /// Holds every request until `expected` of them are in flight, which only happens if the
    /// caller really runs them concurrently. A request that waits too long fails instead of
    /// hanging the test.
    struct Rendezvous {
        expected: usize,
        patience: Duration,
        arrived: Mutex<usize>,
        changed: Condvar,
    }

    impl RouteFetch {
        pub fn new(routes: Vec<(&str, Result<Response, FetchError>)>) -> Self {
            Self {
                routes: routes
                    .into_iter()
                    .map(|(url, answer)| (url.to_owned(), answer))
                    .collect(),
                requests: Mutex::new(Vec::new()),
                rendezvous: None,
            }
        }

        pub fn requiring_concurrency(mut self, expected: usize, patience: Duration) -> Self {
            self.rendezvous = Some(Rendezvous {
                expected,
                patience,
                arrived: Mutex::new(0),
                changed: Condvar::new(),
            });
            self
        }

        pub fn requested_urls(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|(url, _)| url.clone())
                .collect()
        }
    }

    impl Fetch for RouteFetch {
        fn get(&self, url: &str, opts: &FetchOptions) -> Result<Response, FetchError> {
            self.requests
                .lock()
                .unwrap()
                .push((url.to_owned(), opts.clone()));
            if let Some(gate) = &self.rendezvous {
                let mut arrived = gate.arrived.lock().unwrap();
                *arrived += 1;
                gate.changed.notify_all();
                let (_guard, timeout) = gate
                    .changed
                    .wait_timeout_while(arrived, gate.patience, |n| *n < gate.expected)
                    .unwrap();
                if timeout.timed_out() {
                    return Err(FetchError::Transport(
                        "requests were not made concurrently".to_owned(),
                    ));
                }
            }
            self.routes.get(url).cloned().unwrap_or_else(|| {
                Err(FetchError::Transport(format!(
                    "no route to {url} in this test"
                )))
            })
        }
    }

    pub fn ok(status: u16, body: &str) -> Result<Response, FetchError> {
        reply(status, "application/json", body)
    }

    pub fn reply(status: u16, content_type: &str, body: &str) -> Result<Response, FetchError> {
        Ok(Response {
            status,
            content_type: Some(content_type.to_owned()),
            body: body.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    fn opts() -> FetchOptions {
        FetchOptions {
            timeout: Duration::from_secs(5),
            max_bytes: 1024,
            family: Family::Any,
            proxy: ProxyMode::None,
            accept: "application/json",
            bearer: None,
        }
    }

    /// Serves each canned response to one connection on 127.0.0.1 and returns the base URL.
    fn serve(responses: Vec<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                drain_request(&mut stream);
                let _ = stream.write_all(&response);
            }
        });
        format!("http://{addr}")
    }

    /// Like `serve`, but builds the responses from the base URL and reports each
    /// request line it received.
    fn serve_recording(
        make: impl FnOnce(&str) -> Vec<Vec<u8>>,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let responses = make(&base);
        let (seen, request_lines) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = seen.send(drain_request(&mut stream));
                let _ = stream.write_all(&response);
            }
        });
        (base, request_lines)
    }

    /// Reads one request head and returns its first line.
    fn drain_request(stream: &mut TcpStream) -> String {
        read_head(stream)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    /// Reads one request head, request line and headers.
    fn read_head(stream: &mut TcpStream) -> String {
        let mut seen = Vec::new();
        let mut buf = [0u8; 512];
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    /// Like `serve_recording`, but reports the whole head of each request, lowercased so
    /// header names can be matched whatever their case.
    fn serve_heads(make: impl FnOnce(&str) -> Vec<Vec<u8>>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let responses = make(&base);
        let (seen, heads) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = seen.send(read_head(&mut stream).to_ascii_lowercase());
                let _ = stream.write_all(&response);
            }
        });
        (base, heads)
    }

    /// An HTTP proxy that tunnels one CONNECT and then answers the tunneled request.
    /// Returns its URL and a channel that yields the CONNECT line it received.
    fn serve_connect_proxy(body: &'static [u8]) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (seen, connect_line) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = seen.send(drain_request(&mut stream));
            let _ = stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
            drain_request(&mut stream);
            let _ = stream.write_all(&http_response("200 OK", "", body));
        });
        (format!("http://{addr}"), connect_line)
    }

    fn http_response(status_line: &str, extra_headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status_line}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn returns_body_status_and_content_type() {
        let base = serve(vec![http_response(
            "200 OK",
            "Content-Type: application/json\r\n",
            b"{\"ip\":\"203.0.113.7\"}",
        )]);
        let resp = UreqFetch.get(&format!("{base}/json"), &opts()).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.content_type.as_deref(), Some("application/json"));
        assert_eq!(resp.body, "{\"ip\":\"203.0.113.7\"}");
    }

    #[test]
    fn non_2xx_is_a_response_not_an_error() {
        let base = serve(vec![http_response(
            "429 Too Many Requests",
            "",
            b"slow down",
        )]);
        let resp = UreqFetch.get(&base, &opts()).unwrap();
        assert_eq!(resp.status, 429);
        assert_eq!(resp.body, "slow down");
    }

    #[test]
    fn follows_same_origin_redirects() {
        let base = serve(vec![
            http_response("302 Found", "Location: /x\r\n", b""),
            http_response("200 OK", "", b"final"),
        ]);
        let resp = UreqFetch.get(&base, &opts()).unwrap();
        assert_eq!(resp.body, "final");
    }

    #[test]
    fn a_redirect_to_another_origin_is_refused_and_never_contacted() {
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        other.set_nonblocking(true).unwrap();
        let target = format!("http://{}/x", other.local_addr().unwrap());
        let base = serve(vec![http_response(
            "302 Found",
            &format!("Location: {target}\r\n"),
            b"",
        )]);
        let err = UreqFetch.get(&base, &opts()).unwrap_err();
        assert!(
            matches!(&err, FetchError::Rejected(m) if m.contains("different host")),
            "{err:?}"
        );
        assert!(
            !err.to_string().contains(&target),
            "must not echo the target"
        );
        assert_eq!(
            other.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the other origin must not be contacted"
        );
    }

    #[test]
    fn a_protocol_relative_or_downgrading_redirect_is_refused() {
        for location in [
            "//example.com/x",
            "http://example.com/x",
            "ftp://example.com/x",
        ] {
            let base = serve(vec![http_response(
                "302 Found",
                &format!("Location: {location}\r\n"),
                b"",
            )]);
            let err = UreqFetch.get(&base, &opts()).unwrap_err();
            assert!(
                matches!(&err, FetchError::Rejected(m) if m.contains("different host")),
                "{location}: {err:?}"
            );
        }
    }

    #[test]
    fn a_redirect_loop_gives_up() {
        let hop = http_response("302 Found", "Location: /again\r\n", b"");
        let base = serve(vec![hop; 8]);
        let err = UreqFetch.get(&base, &opts()).unwrap_err();
        assert!(
            matches!(&err, FetchError::Rejected(m) if m.contains("too many redirects")),
            "{err:?}"
        );
    }

    #[test]
    fn origin_check_accepts_only_the_same_scheme_and_authority() {
        let base = "https://ipinfo.io/ip";
        for (location, want) in [
            ("/city", Some("https://ipinfo.io/city")),
            ("https://ipinfo.io/x", Some("https://ipinfo.io/x")),
            ("https://IPINFO.io/x", Some("https://IPINFO.io/x")),
            ("http://ipinfo.io/x", None),
            ("https://ipinfo.io.evil.example/x", None),
            ("https://ipinfo.io:8443/x", None),
            ("https://user@ipinfo.io/x", None),
            ("//evil.example/x", None),
            ("x", Some("https://ipinfo.io/x")),
            ("", None),
        ] {
            assert_eq!(
                same_origin_target(base, location).ok().as_deref(),
                want,
                "{location:?}"
            );
        }
    }

    #[test]
    fn relative_references_resolve_against_the_current_url() {
        let base = "https://ipinfo.io/a/b/c?z=1";
        for (location, want) in [
            ("d", Some("https://ipinfo.io/a/b/d")),
            ("./d", Some("https://ipinfo.io/a/b/d")),
            ("../d", Some("https://ipinfo.io/a/d")),
            ("../../../d", Some("https://ipinfo.io/d")),
            ("d/", Some("https://ipinfo.io/a/b/d/")),
            ("..", Some("https://ipinfo.io/a/")),
            ("?q=2", Some("https://ipinfo.io/a/b/c?q=2")),
            ("d?q=2#f", Some("https://ipinfo.io/a/b/d?q=2#f")),
            ("#f", Some("https://ipinfo.io/a/b/c?z=1#f")),
            ("/abs", Some("https://ipinfo.io/abs")),
            // The origin is always ours, whatever the path looks like.
            (
                "../..//evil.example/x",
                Some("https://ipinfo.io//evil.example/x"),
            ),
            (
                "x@evil.example",
                Some("https://ipinfo.io/a/b/x@evil.example"),
            ),
            ("//evil.example/x", None),
            ("city:80", None),
            ("mailto:a@b", None),
            ("", None),
        ] {
            assert_eq!(
                same_origin_target(base, location).ok().as_deref(),
                want,
                "{location:?}"
            );
        }
    }

    #[test]
    fn dot_segments_are_removed_from_every_kind_of_target() {
        let base = "https://ipinfo.io/ip";
        for (location, want) in [
            ("/a/../city", Some("https://ipinfo.io/city")),
            ("/a/./b", Some("https://ipinfo.io/a/b")),
            ("/a/b/..", Some("https://ipinfo.io/a/")),
            ("/../city", Some("https://ipinfo.io/city")),
            ("/a/../../city?q=1#f", Some("https://ipinfo.io/city?q=1#f")),
            // Dots in the query or fragment are data, not path segments.
            ("/a?q=../b", Some("https://ipinfo.io/a?q=../b")),
            ("/a#../b", Some("https://ipinfo.io/a#../b")),
            ("https://ipinfo.io/a/../x", Some("https://ipinfo.io/x")),
            (
                "https://IPINFO.io/a/./x?q=../y",
                Some("https://IPINFO.io/a/x?q=../y"),
            ),
            ("https://ipinfo.io", Some("https://ipinfo.io")),
            ("https://ipinfo.io?q=1", Some("https://ipinfo.io?q=1")),
            // Still the same origin whatever the path says.
            ("https://ipinfo.io/a/../../x", Some("https://ipinfo.io/x")),
            ("https://evil.example/a/../x", None),
        ] {
            assert_eq!(
                same_origin_target(base, location).ok().as_deref(),
                want,
                "{location:?}"
            );
        }
    }

    #[test]
    fn a_redirect_is_requested_with_dot_segments_removed() {
        for location in ["/a/../city", "http://BASE/a/./b/../../city"] {
            let (base, request_lines) = serve_recording(|base| {
                let location = location.replace("http://BASE", base);
                vec![
                    http_response("302 Found", &format!("Location: {location}\r\n"), b""),
                    http_response("200 OK", "", b"final"),
                ]
            });
            let resp = UreqFetch.get(&format!("{base}/start"), &opts()).unwrap();
            assert_eq!(resp.body, "final", "{location}");
            let lines: Vec<String> = request_lines.try_iter().collect();
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert_eq!(lines[1], "GET /city HTTP/1.1", "{location}");
        }
    }

    #[test]
    fn a_path_relative_redirect_is_followed() {
        let base = serve(vec![
            http_response("302 Found", "Location: x\r\n", b""),
            http_response("200 OK", "", b"final"),
        ]);
        let resp = UreqFetch.get(&base, &opts()).unwrap();
        assert_eq!(resp.body, "final");
    }

    #[test]
    fn a_redirect_without_a_location_is_just_a_response() {
        let base = serve(vec![http_response("302 Found", "", b"moved")]);
        let resp = UreqFetch.get(&base, &opts()).unwrap();
        assert_eq!((resp.status, resp.body.as_str()), (302, "moved"));
    }

    #[test]
    fn a_bearer_is_sent_on_the_first_request_and_not_on_a_followed_redirect() {
        let (base, heads) = serve_heads(|_| {
            vec![
                http_response("302 Found", "Location: /x\r\n", b""),
                http_response("307 Temporary Redirect", "Location: /y\r\n", b""),
                http_response("200 OK", "", b"final"),
            ]
        });
        let options = FetchOptions {
            bearer: Some(Secret::new("T0k.en_~+/=-9")),
            ..opts()
        };
        let resp = UreqFetch.get(&base, &options).unwrap();
        assert_eq!(resp.body, "final");
        let heads: Vec<String> = heads.try_iter().collect();
        assert_eq!(heads.len(), 3, "{heads:?}");
        assert!(
            heads[0].contains("\r\nauthorization: bearer t0k.en_~+/=-9\r\n"),
            "{}",
            heads[0]
        );
        for head in &heads[1..] {
            assert!(!head.contains("authorization"), "{head}");
            assert!(!head.contains("t0k.en"), "{head}");
        }
    }

    #[test]
    fn no_bearer_means_no_authorization_header() {
        let (base, heads) = serve_heads(|_| vec![http_response("200 OK", "", b"ok")]);
        UreqFetch.get(&base, &opts()).unwrap();
        let heads: Vec<String> = heads.try_iter().collect();
        assert_eq!(heads.len(), 1, "{heads:?}");
        assert!(!heads[0].contains("authorization"), "{}", heads[0]);
    }

    #[test]
    fn a_redirect_to_another_origin_with_a_bearer_is_refused_without_the_token() {
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        other.set_nonblocking(true).unwrap();
        let target = format!("http://{}/x", other.local_addr().unwrap());
        let (base, heads) = serve_heads(|_| {
            vec![http_response(
                "302 Found",
                &format!("Location: {target}\r\n"),
                b"",
            )]
        });
        let options = FetchOptions {
            bearer: Some(Secret::new("T0k3n")),
            ..opts()
        };
        let err = UreqFetch.get(&base, &options).unwrap_err();
        assert!(
            matches!(&err, FetchError::Rejected(m) if m.contains("different host")),
            "{err:?}"
        );
        assert!(!format!("{err:?}").contains("T0k3n"), "{err:?}");
        assert_eq!(
            other.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the other origin must not be contacted"
        );
        let heads: Vec<String> = heads.try_iter().collect();
        assert!(
            heads[0].contains("authorization: bearer t0k3n"),
            "{heads:?}"
        );
    }

    #[test]
    fn a_bearer_that_cannot_be_a_header_value_is_rejected_without_showing_it() {
        let (base, _heads) = serve_heads(|_| vec![http_response("200 OK", "", b"ok")]);
        let options = FetchOptions {
            bearer: Some(Secret::new("S3CR3T\r\nX-Injected: 1")),
            ..opts()
        };
        let err = UreqFetch.get(&base, &options).unwrap_err();
        assert!(matches!(&err, FetchError::Rejected(_)), "{err:?}");
        for shown in [err.to_string(), format!("{err:?}")] {
            assert!(!shown.contains("S3CR3T"), "{shown}");
        }
    }

    #[test]
    fn the_debug_of_the_options_does_not_show_the_bearer() {
        let options = FetchOptions {
            bearer: Some(Secret::new("S3CR3T")),
            ..opts()
        };
        for shown in [format!("{options:?}"), format!("{options:#?}")] {
            assert!(shown.contains("***"), "{shown}");
            assert!(!shown.contains("S3CR3T"), "{shown}");
        }
        assert!(options.clone().bearer.is_some());
    }

    #[test]
    fn oversized_body_is_an_error_not_a_truncation() {
        let base = serve(vec![http_response("200 OK", "", &[b'a'; 2000])]);
        let err = UreqFetch.get(&base, &opts()).unwrap_err();
        assert_eq!(err, FetchError::TooLarge(1024));
    }

    #[test]
    fn body_exactly_at_the_limit_is_accepted() {
        let base = serve(vec![http_response("200 OK", "", &[b'a'; 1024])]);
        let resp = UreqFetch.get(&base, &opts()).unwrap();
        assert_eq!(resp.body.len(), 1024);
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        let base = serve(vec![http_response("200 OK", "", &[0xff, b'o', b'k'])]);
        let resp = UreqFetch.get(&base, &opts()).unwrap();
        assert_eq!(resp.body, "\u{fffd}ok");
    }

    #[test]
    fn a_stalled_server_times_out_with_the_ureq_message() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, held) = mpsc::channel::<()>();
        let guard = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            let _ = held.recv();
        });
        let mut o = opts();
        o.timeout = Duration::from_millis(300);
        let err = UreqFetch.get(&format!("http://{addr}"), &o).unwrap_err();
        assert_eq!(err, FetchError::Timeout("timeout: global".to_owned()));
        drop(release);
        guard.join().unwrap();
    }

    /// Sends the headers and part of the body, then stalls or hangs up.
    fn serve_truncated(then_stall: bool) -> (String, mpsc::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, held) = mpsc::channel::<()>();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            drain_request(&mut stream);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial");
            if then_stall {
                let _ = held.recv();
            }
        });
        (format!("http://{addr}"), release)
    }

    #[test]
    fn a_timeout_while_reading_the_body_is_still_a_timeout() {
        let (url, _release) = serve_truncated(true);
        let mut o = opts();
        o.timeout = Duration::from_millis(300);
        let err = UreqFetch.get(&url, &o).unwrap_err();
        assert_eq!(err, FetchError::Timeout("timeout: global".to_owned()));
    }

    #[test]
    fn a_connection_cut_mid_body_is_a_rejected_answer_with_the_io_prefix() {
        let (url, _release) = serve_truncated(false);
        let err = UreqFetch.get(&url, &opts()).unwrap_err();
        match err {
            FetchError::Rejected(msg) => assert!(msg.starts_with("io: "), "{msg}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_peer_that_hangs_up_is_a_transport_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });
        let err = UreqFetch
            .get(&format!("http://{addr}"), &opts())
            .unwrap_err();
        assert!(matches!(err, FetchError::Transport(_)), "{err:?}");
    }

    /// std reports a failed `getaddrinfo` as an `io::Error` without an OS code: its own prefix
    /// and then libc's wording. The first reason is what `docker run --network none` printed.
    #[test]
    fn a_failed_name_lookup_is_a_resolution_failure_whatever_libc_says() {
        for reason in [
            "Temporary failure in name resolution",
            "Name or service not known",
        ] {
            let text = format!("failed to lookup address information: {reason}");
            let error = ureq::Error::Io(std::io::Error::other(text.clone()));
            assert_eq!(map_error(error), FetchError::Resolve(format!("io: {text}")));
        }
    }

    #[test]
    fn other_transport_errors_are_not_resolution_failures() {
        let not_at_the_start = "read failed: failed to lookup address information: x";
        for error in [
            ureq::Error::Io(std::io::Error::from_raw_os_error(111)),
            ureq::Error::Io(std::io::Error::other(not_at_the_start)),
            ureq::Error::ConnectionFailed,
        ] {
            let mapped = map_error(error);
            assert!(matches!(mapped, FetchError::Transport(_)), "{mapped:?}");
        }
    }

    #[test]
    fn a_host_with_no_address_of_the_family_is_told_apart_from_a_failed_lookup() {
        assert_eq!(
            map_error(ureq::Error::HostNotFound),
            FetchError::NoAddress("host not found".to_owned())
        );
    }

    #[test]
    fn the_proxy_hint_names_the_flag_and_an_address_of_that_family() {
        let url = || ProxyMode::Url("http://proxy.test:3128".to_owned());
        for error in [
            FetchError::Resolve("x".to_owned()),
            FetchError::NoAddress("x".to_owned()),
        ] {
            for (family, flag, example) in [
                (Family::V6, "-6", "http://[::1]:PORT"),
                (Family::V4, "-4", "http://127.0.0.1:PORT"),
            ] {
                let o = FetchOptions {
                    family,
                    proxy: url(),
                    ..opts()
                };
                let hint = proxy_family_hint(&error, &o).expect("a hint");
                assert!(hint.starts_with(flag), "{hint}");
                assert!(hint.contains("also applies to the proxy host"), "{hint}");
                assert!(hint.contains(example), "{hint}");
                assert!(hint.ends_with(&format!("drop {flag}")), "{hint}");
            }
        }
    }

    #[test]
    fn there_is_no_proxy_hint_unless_a_family_a_proxy_url_and_a_failed_host_lookup_are_all_there() {
        let url = || ProxyMode::Url("http://proxy.test:3128".to_owned());
        let lookup = || FetchError::Resolve("x".to_owned());
        for (error, family, proxy) in [
            (lookup(), Family::Any, url()),
            (lookup(), Family::V6, ProxyMode::None),
            (lookup(), Family::V6, ProxyMode::Env),
            (FetchError::Transport("x".to_owned()), Family::V6, url()),
            (FetchError::Timeout("x".to_owned()), Family::V6, url()),
            (FetchError::Rejected("x".to_owned()), Family::V6, url()),
            (FetchError::TooLarge(1), Family::V6, url()),
        ] {
            let o = FetchOptions {
                family,
                proxy,
                ..opts()
            };
            assert_eq!(proxy_family_hint(&error, &o), None, "{error:?} {o:?}");
        }
    }

    #[test]
    fn a_refused_connection_is_a_transport_error_not_a_resolution_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let err = UreqFetch
            .get(&format!("http://{addr}"), &opts())
            .unwrap_err();
        match err {
            FetchError::Transport(msg) => assert!(msg.contains("refused"), "{msg}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_tls_failure_is_a_transport_error_not_a_resolution_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        });
        let err = UreqFetch
            .get(&format!("https://{addr}"), &opts())
            .unwrap_err();
        assert!(matches!(err, FetchError::Transport(_)), "{err:?}");
    }

    #[test]
    fn the_family_restriction_is_applied() {
        let ipv4_only = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut o = opts();
        o.family = Family::V6;
        let err = UreqFetch
            .get(&format!("http://{}", ipv4_only.local_addr().unwrap()), &o)
            .unwrap_err();
        assert!(matches!(err, FetchError::NoAddress(_)), "{err:?}");

        let base = serve(vec![http_response("200 OK", "", b"v4")]);
        o.family = Family::V4;
        assert_eq!(UreqFetch.get(&base, &o).unwrap().body, "v4");
    }

    // 192.0.2.1 is unroutable, so only a proxy hit can produce this body.
    #[test]
    fn an_explicit_proxy_url_is_used() {
        let (proxy, connect_line) = serve_connect_proxy(b"via-proxy");
        let mut o = opts();
        o.proxy = ProxyMode::Url(proxy);
        let resp = UreqFetch.get("http://192.0.2.1/", &o).unwrap();
        assert_eq!(resp.body, "via-proxy");
        assert!(
            connect_line
                .recv()
                .unwrap()
                .starts_with("CONNECT 192.0.2.1:80")
        );
    }

    const CHILD_MODE: &str = "MYIP_TEST_PROXY_MODE";
    const CHILD_TARGET: &str = "MYIP_TEST_TARGET";
    const CHILD_TEST: &str = "http::tests::environment_proxies_are_honored_only_on_request";

    /// Mutating this process's environment is unsafe in edition 2024, so the test
    /// re-runs itself as a child process that has the proxy variables set.
    #[test]
    fn environment_proxies_are_honored_only_on_request() {
        if let Ok(mode) = std::env::var(CHILD_MODE) {
            let mut o = opts();
            o.proxy = if mode == "env" {
                ProxyMode::Env
            } else {
                ProxyMode::None
            };
            let resp = UreqFetch
                .get(&std::env::var(CHILD_TARGET).unwrap(), &o)
                .unwrap();
            println!("BODY={}", resp.body);
            return;
        }
        for (mode, want) in [("none", "BODY=direct"), ("env", "BODY=via-proxy")] {
            let direct = serve(vec![http_response("200 OK", "", b"direct")]);
            let (proxy, _) = serve_connect_proxy(b"via-proxy");
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
                .env(CHILD_MODE, mode)
                .env(CHILD_TARGET, &direct)
                .env("HTTP_PROXY", &proxy)
                .env("http_proxy", &proxy)
                .env("ALL_PROXY", &proxy)
                .env("all_proxy", &proxy)
                .env_remove("NO_PROXY")
                .env_remove("no_proxy")
                .env_remove("HTTPS_PROXY")
                .env_remove("https_proxy")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{mode}: {stdout}");
            assert!(stdout.contains(want), "{mode}: {stdout}");
        }
    }

    mod route_fetch {
        use super::super::testing::{RouteFetch, ok};
        use super::*;

        #[test]
        fn answers_by_url_whatever_the_order_and_records_requests() {
            let fetch = RouteFetch::new(vec![("https://a.test", ok(200, "a"))]);
            assert_eq!(fetch.get("https://a.test", &opts()).unwrap().body, "a");
            assert_eq!(fetch.get("https://a.test", &opts()).unwrap().body, "a");
            assert_eq!(fetch.requested_urls(), ["https://a.test"; 2]);
        }

        #[test]
        fn an_unrouted_url_is_a_transport_error_naming_it() {
            let fetch = RouteFetch::new(vec![]);
            let error = fetch.get("https://b.test", &opts()).unwrap_err();
            assert_eq!(
                error,
                FetchError::Transport("no route to https://b.test in this test".to_owned())
            );
        }

        #[test]
        fn the_gate_opens_only_when_the_requests_overlap() {
            let routes = || vec![("https://a.test", ok(200, "a"))];
            let patience = Duration::from_secs(10);

            let overlapping = RouteFetch::new(routes()).requiring_concurrency(3, patience);
            let bodies: Vec<String> = thread::scope(|scope| {
                let handles: Vec<_> = (0..3)
                    .map(|_| scope.spawn(|| overlapping.get("https://a.test", &opts())))
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap().unwrap().body)
                    .collect()
            });
            assert_eq!(bodies, ["a", "a", "a"]);

            let one_at_a_time =
                RouteFetch::new(routes()).requiring_concurrency(2, Duration::from_millis(50));
            assert_eq!(
                one_at_a_time.get("https://a.test", &opts()),
                Err(FetchError::Transport(
                    "requests were not made concurrently".to_owned()
                ))
            );
        }
    }
}
