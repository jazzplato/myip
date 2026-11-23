//! `myip [FIELD]`: print what `https://ipinfo.io/<FIELD>` returns.

use std::time::Duration;

use crate::Context;
use crate::cli::Options;
use crate::hint;
use crate::http::{FetchError, FetchOptions, Response, proxy_family_hint};
use crate::redact::{redact_arg_values, redact_query, redact_userinfo};
use crate::sanitize;

const URL_IPINFO: &str = "https://ipinfo.io/";
const DEFAULT_FIELD: &str = "json";
const MAX_BODY_BYTES: usize = 1 << 20;
const MAX_DETAIL_CHARS: usize = 200;

fn redact_credentials(text: &str) -> String {
    redact_query(&redact_userinfo(text))
}

pub fn run(field: Option<&str>, args: &[String], options: &Options, ctx: &mut Context<'_>) -> u8 {
    let fetch_options = FetchOptions {
        timeout: Duration::from_secs(options.timeout),
        max_bytes: MAX_BODY_BYTES,
        family: options.family(),
        proxy: options.proxy.clone(),
        accept: "application/json",
    };
    let url = format!("{URL_IPINFO}{}", field.unwrap_or(DEFAULT_FIELD));
    if options.verbose {
        // FIELD may carry an access token as a query parameter.
        let _ = writeln!(ctx.err, "myip: GET {}", redact_query(&url));
    }

    let response = match ctx.fetch.get(&url, &fetch_options) {
        Ok(response) => response,
        Err(e) => {
            let mut message = describe(&e, options.timeout);
            if let Some(family_hint) = proxy_family_hint(&e, &fetch_options) {
                message.push_str("; hint: ");
                message.push_str(&family_hint);
            }
            let code = fail(ctx, &message);
            if matches!(&e, FetchError::Transport(message) if hint::is_no_ca(message)) {
                let _ = writeln!(ctx.err, "{}", hint::NO_CA_HINT);
            }
            return code;
        }
    };
    if options.verbose {
        let _ = writeln!(
            ctx.err,
            "myip: HTTP {}, {} bytes",
            response.status,
            response.body.len()
        );
    }
    match answer(&response, args) {
        Ok(body) => print_line(ctx, body),
        Err(message) => fail(ctx, &message),
    }
}

fn print_line(ctx: &mut Context<'_>, body: &str) -> u8 {
    let newline = if body.ends_with('\n') { "" } else { "\n" };
    match write!(ctx.out, "{body}{newline}") {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Transport errors and error bodies come from the network, so they are shown with their
/// control characters escaped, after the credentials are redacted.
fn fail(ctx: &mut Context<'_>, message: &str) -> u8 {
    let _ = writeln!(
        ctx.err,
        "myip: {}",
        sanitize::escape(&redact_credentials(message))
    );
    1
}

fn describe(error: &FetchError, timeout_secs: u64) -> String {
    match error {
        FetchError::Timeout(_) => format!("request to ipinfo.io timed out after {timeout_secs}s"),
        FetchError::TooLarge(limit) => {
            format!("response from ipinfo.io is larger than {limit} bytes")
        }
        FetchError::Resolve(message)
        | FetchError::NoAddress(message)
        | FetchError::Rejected(message)
        | FetchError::Transport(message) => message.clone(),
    }
}

/// Accepts only a real answer: 2xx, non-empty, and not a web page.
/// ipinfo.io labels plain-text answers (`city`, `8.8.8.8/org`) as text/html, so the
/// content type cannot be trusted and the body is inspected instead.
///
/// An error body may echo a query value from the command line (a rejected token, say), so
/// those values are masked first, and before `detail` cuts the body to its length limit.
fn answer<'a>(response: &'a Response, args: &[String]) -> Result<&'a str, String> {
    let html = looks_like_html(&response.body);
    if !(200..300).contains(&response.status) {
        let mut message = format!("ipinfo.io returned HTTP {}", response.status);
        if !html && !response.body.trim().is_empty() {
            message.push_str(": ");
            message.push_str(&detail(&redact_arg_values(&response.body, args)));
        }
        return Err(message);
    }
    if response.body.trim().is_empty() {
        return Err("empty response from ipinfo.io".to_owned());
    }
    if html {
        return Err(
            "received an HTML page instead of an answer (captive portal or proxy?)".to_owned(),
        );
    }
    Ok(&response.body)
}

pub(crate) fn looks_like_html(body: &str) -> bool {
    let mut rest = skip_blank(body);
    while let Some(after) = rest.strip_prefix("<!--") {
        match after.split_once("-->") {
            Some((_, next)) => rest = skip_blank(next),
            // A comment that never closes is not a plain answer either.
            None => return true,
        }
    }
    let head: String = rest
        .chars()
        .take(15)
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "<!doctype html",
        "<html",
        "<head",
        "<body",
        "<title",
        "<meta",
    ]
    .iter()
    .any(|start| head.starts_with(start))
}

/// Drops leading whitespace and byte-order marks.
fn skip_blank(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// The body on one line, bounded, so an error message stays readable. Credentials are
/// redacted before truncating, otherwise a cut between the userinfo and the `@` (or
/// inside a query value) would leave the secret prefix unredactable.
pub(crate) fn detail(body: &str) -> String {
    let flat = redact_credentials(&body.split_whitespace().collect::<Vec<_>>().join(" "));
    let mut chars = flat.chars();
    let shown: String = chars.by_ref().take(MAX_DETAIL_CHARS).collect();
    if chars.next().is_some() {
        format!("{shown}...")
    } else {
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_detection_ignores_case_and_leading_whitespace() {
        assert!(looks_like_html("\n  <!DOCTYPE HTML><html>"));
        assert!(looks_like_html("<HTML lang=en>"));
        assert!(!looks_like_html("Chicago"));
        assert!(!looks_like_html(
            "<note>xml is not a captive portal page</note>"
        ));
    }

    #[test]
    fn html_detection_skips_a_bom_and_leading_comments() {
        for page in [
            "\u{feff}<!DOCTYPE html><html></html>",
            "\u{feff}\n <html>",
            "<!-- portal -->\n<html><body>Sign in</body></html>",
            "<!-- a --><!-- b -->\n  <!doctype html>",
            "\u{feff}<!-- a -->\u{feff}\n<HTML>",
            "<!-- never closed <html>",
        ] {
            assert!(looks_like_html(page), "{page:?}");
        }
        for answer in ["<!-- note -->\nChicago", "\u{feff}Chicago", "<!-- x -->"] {
            assert!(!looks_like_html(answer), "{answer:?}");
        }
    }

    #[test]
    fn html_detection_recognizes_documents_without_a_doctype_or_html_tag() {
        for page in [
            "<head><title>Login</title></head><body>Please sign in</body>",
            "<BODY onload=x>Please sign in",
            "\u{feff}<!-- portal -->\n<title>Sign in</title>",
            "  <meta charset=utf-8><p>Sign in",
        ] {
            assert!(looks_like_html(page), "{page:?}");
        }
        for answer in [
            "Chicago",
            "<note>xml</note>",
            "{\"ip\": \"1.2.3.4\"}",
            "AS15169 Google LLC",
        ] {
            assert!(!looks_like_html(answer), "{answer:?}");
        }
    }

    #[test]
    fn credentials_are_redacted_before_the_detail_is_truncated() {
        let body = format!("see http://user:{}@host:1/x for details", "p".repeat(400));
        let shown = detail(&body);
        assert!(!shown.contains("ppp"), "{shown}");
        assert!(shown.contains("http://***@host:1/x"), "{shown}");
    }

    #[test]
    fn detail_is_flattened_and_bounded() {
        assert_eq!(detail("{\n  \"a\": 1\n}\n"), "{ \"a\": 1 }");
        let long = "x".repeat(500);
        let shown = detail(&long);
        assert_eq!(shown.chars().count(), MAX_DETAIL_CHARS + 3);
        assert!(shown.ends_with("..."));
    }
}
