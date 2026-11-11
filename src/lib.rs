//! Implementation detail of the `myip` binary. The API is not stable.

pub mod cli;
pub mod http;
pub mod json;
pub mod legacy;
pub mod net;
pub mod public;
pub mod redact;

use std::ffi::OsString;
use std::io::Write;

use clap::Parser;

use cli::{Cli, Command};
use http::Fetch;

/// Everything `run` needs from the outside world, so tests can substitute it.
pub struct Context<'a> {
    pub fetch: &'a dyn Fetch,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Runs the CLI with the arguments after the program name and returns the exit code.
pub fn run(args: &[String], ctx: &mut Context<'_>) -> u8 {
    let cli =
        match Cli::try_parse_from(std::iter::once("myip").chain(args.iter().map(String::as_str))) {
            Ok(cli) => cli,
            Err(e) => {
                // `--help` and `--version` arrive here too; clap says which stream they belong on.
                let rendered = redact::redact_userinfo(&e.render().to_string());
                let sink = if e.use_stderr() {
                    &mut ctx.err
                } else {
                    &mut ctx.out
                };
                let _ = write!(sink, "{rendered}");
                return u8::try_from(e.exit_code()).unwrap_or(2);
            }
        };
    match (cli.command, cli.field.as_deref()) {
        (Some(Command::Public(args)), None) => public::run(&cli.options, args.verify, ctx),
        (Some(command), None) => not_implemented(command, ctx),
        (Some(command), Some(_)) => {
            let _ = writeln!(
                ctx.err,
                "error: a FIELD cannot be combined with `{}`\n\nFor more information, try '--help'.",
                command.name()
            );
            2
        }
        (None, field) => legacy::run(field, &cli.options, ctx),
    }
}

fn not_implemented(command: Command, ctx: &mut Context<'_>) -> u8 {
    let _ = writeln!(ctx.err, "myip: `{}` is not implemented yet", command.name());
    1
}

/// Converts OS arguments to UTF-8 without altering them. A non-Unicode argument is
/// reported by its 1-based position, never by content, because it may be a credential
/// (0.0.1 panicked here).
pub fn utf8_args(args: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, usize> {
    args.into_iter()
        .enumerate()
        .map(|(i, arg)| arg.into_string().map_err(|_| i + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::http::testing::{FakeFetch, ok, reply};
    use crate::http::{Family, FetchError, ProxyMode};

    fn exec(fetch: &FakeFetch, args: &[&str]) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Context {
                fetch,
                out: &mut out,
                err: &mut err,
            };
            run(&args, &mut ctx)
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn no_network() -> FakeFetch {
        FakeFetch::new(vec![])
    }

    fn requested(fetch: &FakeFetch) -> Vec<String> {
        fetch
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.0.clone())
            .collect()
    }

    #[test]
    fn help_prints_usage_and_exit_codes_without_a_request() {
        for flag in ["--help", "-h"] {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, &[flag]);
            assert_eq!(code, 0, "{flag}");
            assert!(out.contains("Usage: myip"), "{out}");
            assert!(out.contains("Exit codes"), "{out}");
            assert_eq!(err, "");
            assert!(requested(&fetch).is_empty(), "{flag} must not query ipinfo");
        }
    }

    #[test]
    fn version_prints_the_crate_version() {
        let fetch = no_network();
        let (code, out, _) = exec(&fetch, &["--version"]);
        assert_eq!(code, 0);
        assert_eq!(out, concat!("myip ", env!("CARGO_PKG_VERSION"), "\n"));
        assert!(requested(&fetch).is_empty());
    }

    #[test]
    fn usage_errors_exit_2_with_the_message_on_stderr() {
        let cases: [&[&str]; 5] = [
            &["--bogus"],
            &["-4", "-6"],
            &["--timeout", "abc"],
            &["--timeout", "0"],
            &["--proxy", "ftp://example.com"],
        ];
        for case in cases {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, case);
            assert_eq!(code, 2, "{case:?}");
            assert_eq!(out, "", "{case:?}");
            assert!(err.contains("error:"), "{case:?}: {err}");
            assert!(requested(&fetch).is_empty(), "{case:?}");
        }
    }

    #[test]
    fn invalid_fields_are_rejected_before_any_request() {
        let cases = [
            "",
            "   ",
            "foo bar",
            "a/../b",
            "..",
            "x?y=1",
            "x#f",
            "caf\u{e9}",
            "a\nb",
            "a\\b",
            "/city",
            "//example.com",
            "a//b",
            "https://example.com",
            "file:///etc/passwd",
        ];
        for field in cases {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, &[field]);
            assert_eq!(code, 2, "{field:?}");
            assert_eq!(out, "", "{field:?}");
            assert!(err.contains("error:"), "{field:?}: {err}");
            assert!(requested(&fetch).is_empty(), "{field:?}");
        }
    }

    #[test]
    fn proxy_credentials_are_never_printed() {
        const SECRET: &str = "s3cr3t";
        let proxy = format!("http://user:{SECRET}@proxy.example:3128");

        // A rejected value is not quoted back at all.
        let fetch = no_network();
        let (code, out, err) = exec(&fetch, &["--proxy", &format!("ftp://user:{SECRET}@h:1")]);
        assert_eq!(code, 2);
        assert!(!format!("{out}{err}").contains(SECRET), "{err}");
        assert!(!err.contains("h:1"), "{err}");
        assert!(err.contains("--proxy"), "{err}");

        // A transport error may quote the proxy URL.
        let fetch = FakeFetch::new(vec![Err(FetchError::Transport(format!(
            "proxy {proxy} refused the connection"
        )))]);
        let (code, out, err) = exec(&fetch, &["-v", "--proxy", &proxy, "ip"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(!err.contains(SECRET), "{err}");
        assert!(err.contains("***@proxy.example:3128"), "{err}");

        // A successful verbose run never mentions it either.
        let fetch = FakeFetch::new(vec![ok(200, "1.2.3.4\n")]);
        let (code, out, err) = exec(&fetch, &["-v", "--proxy", &proxy, "ip"]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"));
        assert!(!err.contains(SECRET), "{err}");
    }

    #[test]
    fn valid_fields_select_the_url_path_and_ends_are_trimmed() {
        let cases = [
            ("city", "https://ipinfo.io/city"),
            ("8.8.8.8/org", "https://ipinfo.io/8.8.8.8/org"),
            ("2001:db8::1", "https://ipinfo.io/2001:db8::1"),
            (" 8.8.8.8\n", "https://ipinfo.io/8.8.8.8"),
        ];
        for (field, url) in cases {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, _, err) = exec(&fetch, &[field]);
            assert_eq!(code, 0, "{field:?}: {err}");
            assert_eq!(requested(&fetch), [url], "{field:?}");
        }
    }

    #[test]
    fn no_argument_queries_json() {
        let fetch = FakeFetch::new(vec![ok(200, "{}\n")]);
        let (code, out, _) = exec(&fetch, &[]);
        assert_eq!((code, out.as_str()), (0, "{}\n"));
        assert_eq!(requested(&fetch), ["https://ipinfo.io/json"]);
    }

    #[test]
    fn http_errors_exit_1_with_empty_stdout_and_the_reason_on_stderr() {
        for status in [404, 429, 500, 503] {
            let fetch = FakeFetch::new(vec![ok(status, "{\"error\":\"nope\"}")]);
            let (code, out, err) = exec(&fetch, &[]);
            assert_eq!(code, 1, "{status}");
            assert_eq!(out, "", "{status}");
            assert!(err.starts_with("myip: "), "{err}");
            assert!(err.contains(&status.to_string()), "{err}");
            assert!(err.contains("nope"), "the body explains the failure: {err}");
        }
    }

    #[test]
    fn html_answers_are_errors_not_results() {
        let page = "\n<!DOCTYPE html><html><body>Sign in to the Wi-Fi</body></html>";
        let fetch = FakeFetch::new(vec![reply(200, "text/html", page)]);
        let (code, out, err) = exec(&fetch, &[]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("HTML"), "{err}");
    }

    #[test]
    fn a_portal_page_hidden_behind_a_bom_or_comment_is_still_an_error() {
        for page in [
            "\u{feff}<!DOCTYPE html><html>Sign in</html>",
            "<!-- portal -->\n<html>Sign in</html>",
        ] {
            let fetch = FakeFetch::new(vec![ok(200, page)]);
            let (code, out, err) = exec(&fetch, &["ip"]);
            assert_eq!((code, out.as_str()), (1, ""), "{page:?}");
            assert!(err.contains("HTML"), "{err}");
        }
    }

    #[test]
    fn a_malformed_proxy_url_is_a_usage_error_not_a_transport_error() {
        for value in [
            "http://host:not-a-port",
            "http://host:99999",
            "http://host:0",
            "http://host:",
            "http://user:S3CR3T@host:not-a-port",
            "http://user:S3CR3T/x@host:1",
            "http://host:3128/path",
            "http://host:3128?x=1",
            "http://",
        ] {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, &["--proxy", value, "ip"]);
            assert_eq!((code, out.as_str()), (2, ""), "{value}");
            assert!(err.contains("--proxy"), "{value}: {err}");
            assert!(!err.contains("S3CR3T"), "{value}: {err}");
            assert!(requested(&fetch).is_empty(), "{value}");
        }
    }

    #[test]
    fn well_formed_proxy_urls_are_accepted() {
        for value in [
            "http://host",
            "http://host:3128",
            "http://host:3128/",
            "http://[::1]:3128",
            "http://user:pa%40ss@host:3128",
            "http://127.0.0.1:65535",
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "1.2.3.4\n")]);
            let (code, out, err) = exec(&fetch, &["--proxy", value, "ip"]);
            assert_eq!(
                (code, out.as_str(), err.as_str()),
                (0, "1.2.3.4\n", ""),
                "{value}"
            );
        }
    }

    #[test]
    fn a_non_2xx_body_never_prints_credentials_even_when_truncated() {
        let body = format!("proxy http://user:{}@host:1/x refused", "S3CR3T".repeat(60));
        let fetch = FakeFetch::new(vec![reply(502, "text/plain", &body)]);
        let (code, out, err) = exec(&fetch, &["ip"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("HTTP 502"), "{err}");
        assert!(!err.contains("S3CR3T"), "{err}");
    }

    #[test]
    fn a_portal_page_without_a_doctype_is_still_an_error() {
        let page = "<head><title>Login</title></head><body>Please sign in</body>";
        let fetch = FakeFetch::new(vec![ok(200, page)]);
        let (code, out, err) = exec(&fetch, &["ip"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("HTML"), "{err}");
    }

    // ipinfo.io labels plain-text answers such as `city` as text/html.
    #[test]
    fn a_plain_answer_labelled_text_html_is_still_an_answer() {
        let fetch = FakeFetch::new(vec![reply(200, "text/html; charset=utf-8", "Chicago\n")]);
        let (code, out, _) = exec(&fetch, &["city"]);
        assert_eq!((code, out.as_str()), (0, "Chicago\n"));
    }

    #[test]
    fn an_empty_answer_is_an_error() {
        let fetch = FakeFetch::new(vec![ok(200, " \n")]);
        let (code, out, err) = exec(&fetch, &["city"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn output_always_ends_with_exactly_one_newline() {
        for (body, want) in [("1.2.3.4", "1.2.3.4\n"), ("1.2.3.4\n", "1.2.3.4\n")] {
            let fetch = FakeFetch::new(vec![ok(200, body)]);
            let (_, out, _) = exec(&fetch, &["ip"]);
            assert_eq!(out, want);
        }
    }

    #[test]
    fn transport_errors_go_to_stderr_without_the_old_typo() {
        let fetch = FakeFetch::new(vec![Err(FetchError::Timeout("timeout: global".to_owned()))]);
        let (code, out, err) = exec(&fetch, &[]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert_eq!(err, "myip: request to ipinfo.io timed out after 10s\n");
    }

    #[test]
    fn defaults_are_a_10s_timeout_no_proxy_and_either_family() {
        let fetch = FakeFetch::new(vec![ok(200, "x")]);
        exec(&fetch, &[]);
        let (_, opts) = fetch.requests.lock().unwrap()[0].clone();
        assert_eq!(opts.timeout, Duration::from_secs(10));
        assert_eq!(opts.proxy, ProxyMode::None);
        assert_eq!(opts.family, Family::Any);
        assert_eq!(opts.accept, "application/json");
        assert_eq!(opts.max_bytes, 1024 * 1024);
    }

    #[test]
    fn proxy_values_are_never_echoed_in_usage_errors() {
        const SECRET: &str = "S3CR3T";
        let bad = [
            "user:S3CR3T@proxy.example:3128",
            "ftp://user:S3CR3T@h:1",
            "ftp://user:pa S3CR3T@h:1",
            "ftp://user:pa\"S3CR3T@h:1",
            "ftp://user:pa'S3CR3T@h:1",
            "socks5://user:S3CR3T@h:1",
            "https://user:S3CR3T@h:1",
        ];
        for value in bad {
            for args in [
                vec!["--proxy".to_owned(), value.to_owned()],
                vec![format!("--proxy={value}")],
            ] {
                let fetch = no_network();
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let (code, out, err) = exec(&fetch, &args);
                assert_eq!(code, 2, "{args:?}");
                assert!(!format!("{out}{err}").contains(SECRET), "{args:?}: {err}");
                assert!(err.contains("--proxy"), "{args:?}: {err}");
                assert!(requested(&fetch).is_empty(), "{args:?}");
            }
        }
    }

    #[test]
    fn flags_before_a_reserved_name_do_not_turn_it_into_a_field() {
        let cases: [&[&str]; 5] = [
            &["--json", "local"],
            &["-4", "where"],
            &["--timeout", "5", "all"],
            &["--proxy", "env", "all"],
            &["-6", "-v", "--json", "local"],
        ];
        for args in cases {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert!(err.contains("not implemented"), "{args:?}: {err}");
            assert!(
                requested(&fetch).is_empty(),
                "{args:?} must not query ipinfo"
            );
        }
    }

    #[test]
    fn verify_and_extra_arguments_are_only_valid_for_public() {
        let cases: [&[&str]; 6] = [
            &["--verify"],
            &["ip", "--verify"],
            &["local", "--verify"],
            &["where", "--verify"],
            &["public", "ip"],
            &["public", "--verify=yes"],
        ];
        for args in cases {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!((code, out.as_str()), (2, ""), "{args:?}");
            assert!(err.contains("error:"), "{args:?}: {err}");
            assert!(requested(&fetch).is_empty(), "{args:?}");
        }
    }

    #[test]
    fn public_is_documented_in_the_help_and_verify_is_a_flag_of_public_only() {
        let fetch = no_network();
        let (code, out, _) = exec(&fetch, &["--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("\n  public"), "{out}");
        assert!(!out.contains("      --verify"), "{out}");
        let (code, out, _) = exec(&fetch, &["public", "--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("--verify"), "{out}");
        assert!(out.contains("one.one.one.one"), "{out}");
        assert!(requested(&fetch).is_empty());
    }

    #[test]
    fn a_field_cannot_be_combined_with_another_argument_or_a_reserved_name() {
        let cases: [&[&str]; 4] = [
            &["ip", "city"],
            &["ip", "public"],
            &["-v", "ip", "all"],
            &["8.8.8.8/org", "local"],
        ];
        for args in cases {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!((code, out.as_str()), (2, ""), "{args:?}");
            assert!(err.contains("error:"), "{args:?}: {err}");
            assert!(requested(&fetch).is_empty(), "{args:?}");
        }
    }

    #[test]
    fn flags_reach_the_transport_before_and_after_the_field() {
        for args in [
            ["--timeout", "3", "-4", "--proxy", "env", "ip"],
            ["ip", "--timeout", "3", "-4", "--proxy", "env"],
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, _, err) = exec(&fetch, &args);
            assert_eq!(code, 0, "{args:?}: {err}");
            let (_, opts) = fetch.requests.lock().unwrap()[0].clone();
            assert_eq!(opts.timeout, Duration::from_secs(3));
            assert_eq!(opts.family, Family::V4);
            assert_eq!(opts.proxy, ProxyMode::Env);
        }
    }

    #[test]
    fn an_explicit_proxy_url_and_ipv6_are_forwarded() {
        let fetch = FakeFetch::new(vec![ok(200, "x")]);
        exec(&fetch, &["-6", "--proxy", "http://127.0.0.1:3128"]);
        let (_, opts) = fetch.requests.lock().unwrap()[0].clone();
        assert_eq!(opts.family, Family::V6);
        assert_eq!(
            opts.proxy,
            ProxyMode::Url("http://127.0.0.1:3128".to_owned())
        );
    }

    #[test]
    fn json_is_accepted_and_passes_the_ipinfo_body_through() {
        let fetch = FakeFetch::new(vec![ok(200, "{\"ip\":\"1.2.3.4\"}\n")]);
        let (code, out, _) = exec(&fetch, &["--json"]);
        assert_eq!((code, out.as_str()), (0, "{\"ip\":\"1.2.3.4\"}\n"));
    }

    #[test]
    fn verbose_logs_to_stderr_and_leaves_stdout_alone() {
        let fetch = FakeFetch::new(vec![ok(200, "1.2.3.4\n")]);
        let (code, out, err) = exec(&fetch, &["-v", "ip"]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"));
        assert!(err.contains("https://ipinfo.io/ip"), "{err}");
        assert!(err.contains("200"), "{err}");
    }

    #[test]
    fn the_other_dimension_subcommands_are_reserved_but_not_implemented_yet() {
        for name in ["local", "where", "all"] {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, &[name]);
            assert_eq!((code, out.as_str()), (1, ""), "{name}");
            assert!(err.contains("not implemented"), "{name}: {err}");
            assert!(requested(&fetch).is_empty(), "{name}");
        }
    }

    #[test]
    fn unicode_arguments_pass_through_unchanged() {
        let got = utf8_args(["ip", "8.8.8.8/org", "héllo"].map(OsString::from)).unwrap();
        assert_eq!(got, ["ip", "8.8.8.8/org", "héllo"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_argument_is_rejected_not_substituted() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![b'i', 0xFF, b'p']);
        assert_eq!(utf8_args([OsString::from("ip"), bad]), Err(2));
    }
}
