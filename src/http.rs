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
    Timeout,
    TooLarge(usize),
    Transport(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("request timed out"),
            Self::TooLarge(limit) => write!(f, "response larger than {limit} bytes"),
            Self::Transport(msg) => f.write_str(msg),
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

        let mut response = agent
            .get(url)
            .header("Accept", opts.accept)
            .call()
            .map_err(map_error)?;
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
            .map_err(|e| FetchError::Transport(e.to_string()))?;
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
        ureq::Error::Timeout(_) => FetchError::Timeout,
        other => FetchError::Transport(other.to_string()),
    }
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
        Ok(Response {
            status,
            content_type: Some("application/json".to_owned()),
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

    fn drain_request(stream: &mut TcpStream) {
        let mut seen = Vec::new();
        let mut buf = [0u8; 512];
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        }
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
    fn follows_redirects() {
        let target = serve(vec![http_response("200 OK", "", b"final")]);
        let base = serve(vec![http_response(
            "302 Found",
            &format!("Location: {target}/x\r\n"),
            b"",
        )]);
        let resp = UreqFetch.get(&base, &opts()).unwrap();
        assert_eq!(resp.body, "final");
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
    fn a_stalled_server_times_out() {
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
        assert_eq!(err, FetchError::Timeout);
        drop(release);
        guard.join().unwrap();
    }

    #[test]
    fn connection_refused_is_a_transport_error() {
        let addr = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let err = UreqFetch
            .get(&format!("http://{addr}"), &opts())
            .unwrap_err();
        assert!(matches!(err, FetchError::Transport(_)), "{err:?}");
    }

    #[test]
    fn the_family_restriction_is_applied() {
        let base = serve(vec![http_response("200 OK", "", b"v4")]);
        let mut o = opts();
        o.family = Family::V6;
        let err = UreqFetch.get(&base, &o).unwrap_err();
        assert!(matches!(err, FetchError::Transport(_)), "{err:?}");

        let base = serve(vec![http_response("200 OK", "", b"v4")]);
        o.family = Family::V4;
        assert_eq!(UreqFetch.get(&base, &o).unwrap().body, "v4");
    }

    #[test]
    fn an_explicit_proxy_url_is_used() {
        // The target is unroutable, so only a dead-proxy refusal yields `Transport` quickly.
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let mut o = opts();
        o.proxy = ProxyMode::Url(format!("http://{dead}"));
        let err = UreqFetch.get("http://192.0.2.1/", &o).unwrap_err();
        assert!(matches!(err, FetchError::Transport(_)), "{err:?}");
    }
}
