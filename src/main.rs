use std::env;
use std::process;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig};

const URL_IPINFO: &str = "https://ipinfo.io/";
const URL_ININFO_JSON: &str = "json";
const USER_AGENT: &str = concat!("myip/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the request URL, normalizing the query the way the WHATWG URL parser
/// (used by the old `reqwest::get`) did. `ureq` hands the string to
/// `http::Uri`, which rejects spaces and control characters outright.
fn build_url(query: &str) -> String {
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

fn run(query: &str) -> Result<(), ureq::Error> {
    // Preserve the historical behavior of printing whatever body came back,
    // even for non-2xx responses. ipinfo.io picks HTML vs JSON error pages
    // from the User-Agent, so ask for JSON explicitly to keep output stable.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .timeout_global(Some(TIMEOUT))
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let body = agent
        .get(build_url(query))
        .header("Accept", "application/json")
        .call()?
        .body_mut()
        .read_to_string()?;
    print!("{body}");

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut query = URL_ININFO_JSON;
    if args.len() > 1 {
        query = &args[1];
    }

    if let Err(e) = run(query) {
        println!("Error occured: \n{e}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
