//! The 0.0.1 behavior: print whatever `https://ipinfo.io/<query>` returned.

use std::time::Duration;

use crate::Context;
use crate::http::{Family, FetchOptions, ProxyMode};

const URL_IPINFO: &str = "https://ipinfo.io/";
const DEFAULT_QUERY: &str = "json";

/// Builds the request URL, normalizing the query the way the WHATWG URL parser
/// (used by the old `reqwest::get`) did. `ureq` hands the string to
/// `http::Uri`, which rejects spaces and control characters outright.
pub fn build_url(query: &str) -> String {
    let query = query.trim_end_matches(|c: char| c <= ' ');
    let mut url = String::from(URL_IPINFO);
    for c in query.chars() {
        match c {
            '\t' | '\n' | '\r' => {}
            '\\' => url.push('/'),
            c if c <= ' '
                || !c.is_ascii()
                || matches!(c, '\u{7f}' | '"' | '<' | '>' | '`' | '{' | '}') =>
            {
                let mut buf = [0; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    url.push_str(&format!("%{b:02X}"));
                }
            }
            c => url.push(c),
        }
    }
    url
}

pub fn run(query: Option<&str>, ctx: &mut Context<'_>) -> u8 {
    let opts = FetchOptions {
        timeout: Duration::from_secs(30),
        max_bytes: 10 * 1024 * 1024,
        family: Family::Any,
        proxy: ProxyMode::None,
        accept: "application/json",
    };
    let url = build_url(query.unwrap_or(DEFAULT_QUERY));
    match ctx.fetch.get(&url, &opts) {
        Ok(response) => match write!(ctx.out, "{}", response.body) {
            Ok(()) => 0,
            Err(_) => 1,
        },
        Err(e) => {
            let _ = write!(ctx.out, "Error occured: \n{e}\n");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::testing::{FakeFetch, ok};
    use crate::http::{Fetch, FetchError};

    fn run_with(fetch: &FakeFetch, query: Option<&str>) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Context {
                fetch: fetch as &dyn Fetch,
                out: &mut out,
                err: &mut err,
            };
            run(query, &mut ctx)
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn build_url_appends_query_to_base() {
        assert_eq!(build_url("json"), "https://ipinfo.io/json");
        assert_eq!(build_url("ip"), "https://ipinfo.io/ip");
    }

    #[test]
    fn build_url_ignores_trailing_whitespace_and_embedded_newlines() {
        assert_eq!(build_url("8.8.8.8 "), "https://ipinfo.io/8.8.8.8");
        assert_eq!(build_url("8.8.8.8\n"), "https://ipinfo.io/8.8.8.8");
        assert_eq!(build_url("8.8.8.8\r\n"), "https://ipinfo.io/8.8.8.8");
        assert_eq!(build_url("8.8\n.8.8"), "https://ipinfo.io/8.8.8.8");
    }

    #[test]
    fn build_url_percent_encodes_unsafe_characters() {
        assert_eq!(build_url("foo bar"), "https://ipinfo.io/foo%20bar");
        assert_eq!(build_url(" 8.8.8.8"), "https://ipinfo.io/%208.8.8.8");
        assert_eq!(build_url("a\"b<c>d"), "https://ipinfo.io/a%22b%3Cc%3Ed");
        assert_eq!(build_url("8.8.8.8\\org"), "https://ipinfo.io/8.8.8.8/org");
        assert_eq!(build_url("café"), "https://ipinfo.io/caf%C3%A9");
    }

    #[test]
    fn no_argument_queries_json() {
        let fetch = FakeFetch::new(vec![ok(200, "{}")]);
        let (code, out, _) = run_with(&fetch, None);
        assert_eq!((code, out.as_str()), (0, "{}"));
        assert_eq!(fetch.requests.borrow()[0].0, "https://ipinfo.io/json");
    }

    #[test]
    fn the_argument_selects_the_path() {
        let fetch = FakeFetch::new(vec![ok(200, "Chicago")]);
        let (_, out, _) = run_with(&fetch, Some("city"));
        assert_eq!(out, "Chicago");
        assert_eq!(fetch.requests.borrow()[0].0, "https://ipinfo.io/city");
    }

    #[test]
    fn requests_keep_the_historical_transport_settings() {
        let fetch = FakeFetch::new(vec![ok(200, "")]);
        run_with(&fetch, None);
        let (_, opts) = fetch.requests.borrow()[0].clone();
        assert_eq!(opts.timeout, Duration::from_secs(30));
        assert_eq!(opts.proxy, ProxyMode::None);
        assert_eq!(opts.family, Family::Any);
        assert_eq!(opts.accept, "application/json");
    }

    // The next two tests pin 0.0.1 behavior that the CLI-contract spec (#14) deliberately changes.
    #[test]
    fn non_2xx_bodies_are_printed_with_exit_zero() {
        let fetch = FakeFetch::new(vec![ok(429, "{\"error\":\"rate limited\"}")]);
        let (code, out, _) = run_with(&fetch, None);
        assert_eq!((code, out.as_str()), (0, "{\"error\":\"rate limited\"}"));
    }

    #[test]
    fn errors_go_to_stdout_with_exit_one() {
        let fetch = FakeFetch::new(vec![Err(FetchError::Timeout("timeout: global".to_owned()))]);
        let (code, out, err) = run_with(&fetch, None);
        assert_eq!(code, 1);
        assert_eq!(out, "Error occured: \ntimeout: global\n");
        assert_eq!(err, "");
    }
}
