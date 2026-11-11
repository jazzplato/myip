use std::fmt;
use std::io::Read;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig};

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
    Transport(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(msg) | Self::Transport(msg) => f.write_str(msg),
            Self::TooLarge(limit) => {
                write!(f, "the response body is larger than request limit: {limit}")
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// HTTP GET, the only network seam the rest of the crate depends on.
/// Non-2xx statuses are returned as a `Response`, not an error.
pub trait Fetch {
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

        let mut current = url.to_owned();
        let mut hops = 0;
        let mut response = loop {
            let response = agent
                .get(&current)
                .header("Accept", opts.accept)
                .call()
                .map_err(map_error)?;
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
                return Err(FetchError::Transport("too many redirects".to_owned()));
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
            .map_err(|e| map_error(ureq::Error::from(e)))?;
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

fn map_error(e: ureq::Error) -> FetchError {
    match e {
        ureq::Error::Timeout(_) => FetchError::Timeout(e.to_string()),
        other => FetchError::Transport(other.to_string()),
    }
}

const MAX_REDIRECTS: usize = 5;

/// Resolves a `Location` against `base`, accepting only a target on the same scheme
/// and authority (ADR 0001: no cross-host redirects, no downgrade to http). Relative
/// references are resolved per RFC 3986 section 5; the origin of the result is always
/// `base`'s, so a path can never change the host. The target is deliberately not
/// echoed in the error: it is server-controlled and may carry credentials.
fn same_origin_target(base: &str, location: &str) -> Result<String, FetchError> {
    let refused = || FetchError::Transport("refused a redirect to a different host".to_owned());
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
    use std::cell::RefCell;

    use super::*;

    /// Replays canned answers in order and records every request.
    pub struct FakeFetch {
        answers: RefCell<Vec<Result<Response, FetchError>>>,
        pub requests: RefCell<Vec<(String, FetchOptions)>>,
    }

    impl FakeFetch {
        pub fn new(answers: Vec<Result<Response, FetchError>>) -> Self {
            Self {
                answers: RefCell::new(answers.into_iter().rev().collect()),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl Fetch for FakeFetch {
        fn get(&self, url: &str, opts: &FetchOptions) -> Result<Response, FetchError> {
            self.requests
                .borrow_mut()
                .push((url.to_owned(), opts.clone()));
            self.answers
                .borrow_mut()
                .pop()
                .expect("unexpected extra request")
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
        let mut seen = Vec::new();
        let mut buf = [0u8; 512];
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        }
        String::from_utf8_lossy(&seen)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
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
            matches!(&err, FetchError::Transport(m) if m.contains("different host")),
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
                matches!(&err, FetchError::Transport(m) if m.contains("different host")),
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
            matches!(&err, FetchError::Transport(m) if m.contains("too many redirects")),
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
    fn a_connection_cut_mid_body_is_a_transport_error_with_the_io_prefix() {
        let (url, _release) = serve_truncated(false);
        let err = UreqFetch.get(&url, &opts()).unwrap_err();
        match err {
            FetchError::Transport(msg) => assert!(msg.starts_with("io: "), "{msg}"),
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

    #[test]
    fn the_family_restriction_is_applied() {
        let ipv4_only = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut o = opts();
        o.family = Family::V6;
        let err = UreqFetch
            .get(&format!("http://{}", ipv4_only.local_addr().unwrap()), &o)
            .unwrap_err();
        assert!(matches!(err, FetchError::Transport(_)), "{err:?}");

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
}
