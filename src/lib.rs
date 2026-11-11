//! Implementation detail of the `myip` binary. The API is not stable.

pub mod all;
pub mod cli;
pub mod egress;
pub mod env;
pub mod fact;
pub mod http;
pub mod ifaces;
pub mod json;
pub mod legacy;
pub mod local;
pub mod location;
pub mod net;
pub mod output;
pub mod public;
pub mod redact;

use std::ffi::OsString;
use std::io::Write;

use clap::Parser;

use cli::{Cli, Command};
use egress::{EgressProbe, UdpProbe};
use env::{Env, SystemEnv};
use http::Fetch;
use ifaces::{Interfaces, SystemInterfaces};

/// The machine `local`, `where` and `all` look at. Tests substitute fixtures for all three.
pub struct Host<'a> {
    pub env: &'a dyn Env,
    pub interfaces: &'a dyn Interfaces,
    pub probe: &'a dyn EgressProbe,
}

impl Host<'static> {
    /// The real machine.
    pub fn system() -> Self {
        Self {
            env: &SystemEnv,
            interfaces: &SystemInterfaces,
            probe: &UdpProbe,
        }
    }
}

/// Everything `run` needs from the outside world, so tests can substitute it.
pub struct Context<'a> {
    pub fetch: &'a dyn Fetch,
    pub host: &'a Host<'a>,
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
        (Some(Command::Local), None) => local::run(&cli.options, ctx),
        (Some(Command::Where), None) => location::run(&cli.options, ctx),
        (Some(Command::All(args)), None) => all::run(&cli.options, args.verify, ctx),
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
    use crate::egress::PROBE_V4;
    use crate::egress::testing::FakeProbe;
    use crate::env::RecordingEnv;
    use crate::env::testing::FakeEnv;
    use crate::http::testing::{FakeFetch, RouteFetch, ok, reply};
    use crate::http::{Family, FetchError, ProxyMode};
    use crate::ifaces::testing::{FakeInterfaces, iface};
    use crate::public::PROVIDERS;

    /// A global address, so the public lookup accepts it.
    const PUBLIC_IP: &str = "1.2.3.4";
    const OTHER_IP: &str = "5.6.7.8";
    const PUBLIC_IPV6: &str = "2606:4700:4700::1111";

    /// A fixture machine; nothing here depends on the host the tests run on.
    struct Machine {
        env: FakeEnv,
        interfaces: FakeInterfaces,
        probe: FakeProbe,
    }

    impl Machine {
        fn quiet() -> Self {
            Self {
                env: FakeEnv::new(),
                interfaces: FakeInterfaces::with(vec![]),
                probe: FakeProbe::new(),
            }
        }

        fn laptop() -> Self {
            Self {
                env: FakeEnv::new(),
                interfaces: FakeInterfaces::with(vec![
                    iface("lo", 1, &[("127.0.0.1", 8)]),
                    iface("en0", 2, &[("192.168.1.23", 24)]),
                    iface("utun3", 9, &[("100.64.0.7", 32)]),
                ]),
                probe: FakeProbe::new().route(PROBE_V4, "192.168.1.23"),
            }
        }

        fn without_interface_access() -> Self {
            Self {
                interfaces: FakeInterfaces::failing(std::io::ErrorKind::PermissionDenied),
                ..Self::laptop()
            }
        }

        fn host(&self) -> Host<'_> {
            Host {
                env: &self.env,
                interfaces: &self.interfaces,
                probe: &self.probe,
            }
        }
    }

    fn run_on(host: &Host<'_>, fetch: &dyn Fetch, args: &[&str]) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Context {
                fetch,
                host,
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

    fn exec(fetch: &FakeFetch, args: &[&str]) -> (u8, String, String) {
        run_on(&Machine::quiet().host(), fetch, args)
    }

    fn exec_on(machine: &Machine, args: &[&str]) -> (u8, String, String) {
        run_on(&machine.host(), &no_network(), args)
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

    /// What the six public-address providers say, in ADR order; `None` is one that cannot be
    /// reached. The first speaks the Cloudflare trace format, the others answer a bare address.
    fn providers(answers: [Option<&str>; 6]) -> RouteFetch {
        let routes = PROVIDERS
            .iter()
            .zip(answers)
            .map(|(provider, answer)| {
                let response = match answer {
                    Some(ip) if provider.name == "one.one.one.one" => {
                        reply(200, "text/plain", &format!("fl=1\nip={ip}\nts=1\n"))
                    }
                    Some(ip) => reply(200, "text/plain", &format!("{ip}\n")),
                    None => Err(FetchError::Transport("connection refused".to_owned())),
                };
                (provider.url, response)
            })
            .collect();
        RouteFetch::new(routes)
    }

    fn everyone_says(ip: &str) -> RouteFetch {
        providers([Some(ip); 6])
    }

    fn nobody_answers() -> RouteFetch {
        providers([None; 6])
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
    fn help_lists_every_command() {
        let (code, out, _) = exec(&no_network(), &["--help"]);
        assert_eq!(code, 0);
        let commands = out
            .split("Commands:")
            .nth(1)
            .and_then(|rest| rest.split("Options:").next())
            .unwrap_or_else(|| panic!("no Commands section in\n{out}"));
        for name in ["public", "local", "where", "all"] {
            assert!(commands.contains(name), "{name} missing in\n{commands}");
        }
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
    fn verify_and_extra_arguments_are_only_valid_for_public_and_all() {
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
    fn flags_around_a_command_name_do_not_turn_it_into_a_field() {
        let cases: [&[&str]; 8] = [
            &["--json", "local"],
            &["-4", "where"],
            &["--timeout", "5", "all"],
            &["--proxy", "env", "all"],
            &["-6", "-v", "--json", "local"],
            &["where", "--json"],
            &["local", "-4"],
            &["-v", "where"],
        ];
        for args in cases {
            let fetch = everyone_says(PUBLIC_IP);
            let (code, out, err) = run_on(&Machine::laptop().host(), &fetch, args);
            assert!(!out.is_empty(), "{args:?}: {err}");
            assert!(!err.contains("error:"), "{args:?}: {err}");
            assert_eq!(code, 0, "{args:?}: {err}");
            assert_eq!(
                !fetch.requested_urls().is_empty(),
                args.contains(&"all"),
                "{args:?}: only `all` may ask the public providers"
            );
        }
    }

    #[test]
    fn flags_reach_the_public_lookup_of_all() {
        for (args, family, timeout, proxy) in [
            (
                &["--timeout", "5", "-4", "all"][..],
                Family::V4,
                5,
                ProxyMode::None,
            ),
            (
                &["all", "-6", "--timeout", "7"],
                Family::V6,
                7,
                ProxyMode::None,
            ),
            (&["--proxy", "env", "all"], Family::Any, 10, ProxyMode::Env),
        ] {
            let ip = if family == Family::V6 {
                PUBLIC_IPV6
            } else {
                PUBLIC_IP
            };
            let fetch = everyone_says(ip);
            let (code, _, err) = run_on(&Machine::laptop().host(), &fetch, args);
            assert_eq!(code, 0, "{args:?}: {err}");
            let requests = fetch.requests.lock().unwrap();
            assert!(!requests.is_empty(), "{args:?}");
            for (_, opts) in requests.iter() {
                assert_eq!(opts.family, family, "{args:?}");
                assert_eq!(opts.timeout, Duration::from_secs(timeout), "{args:?}");
                assert_eq!(opts.proxy, proxy, "{args:?}");
            }
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
    fn local_lists_interfaces_with_scope_and_marks_the_primary_egress() {
        let fetch = no_network();
        let (code, out, err) = run_on(&Machine::laptop().host(), &fetch, &["local"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.contains("en0"), "{out}");
        assert!(out.contains("192.168.1.23/24"), "{out}");
        assert!(out.contains("private"), "{out}");
        assert!(out.contains("primary egress (IPv4)"), "{out}");
        assert!(out.contains("utun3 (likely virtual)"), "{out}");
        assert!(out.contains("cgnat"), "{out}");
        assert!(
            requested(&fetch).is_empty(),
            "local must not use the network"
        );
    }

    #[test]
    fn local_json_is_one_compact_document_with_the_schema_version_first() {
        let (code, out, err) = exec_on(&Machine::laptop(), &["--json", "local"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(
            out.starts_with("{\"schema_version\":1,\"interfaces\":[{\"name\":\"lo\""),
            "{out}"
        );
        assert!(out.ends_with("}\n"), "{out}");
        assert_eq!(out.matches('\n').count(), 1, "{out}");
        assert!(out.contains("\"primary_egress\":true"), "{out}");
        assert!(out.contains("\"error\":null"), "{out}");
    }

    #[test]
    fn local_filters_by_family_and_probes_only_that_family() {
        let machine = Machine::laptop();
        let (code, out, _) = exec_on(&machine, &["-4", "--json", "local"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"egress\":{\"ipv4\":{"), "{out}");
        assert!(!out.contains("ipv6"), "{out}");
        assert_eq!(machine.probe.asked(), [PROBE_V4]);
    }

    #[test]
    fn local_exits_1_only_when_the_interfaces_cannot_be_listed() {
        let (code, out, err) = exec_on(&Machine::without_interface_access(), &["local"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(
            err.starts_with("myip: cannot list network interfaces"),
            "{err}"
        );
    }

    #[test]
    fn local_json_still_prints_a_document_when_the_interfaces_cannot_be_listed() {
        let (code, out, err) = exec_on(&Machine::without_interface_access(), &["--json", "local"]);
        assert_eq!(code, 1);
        assert!(out.contains("\"interfaces\":null"), "{out}");
        assert!(
            out.contains("\"error\":\"cannot list network interfaces"),
            "{out}"
        );
        assert_eq!(out.matches('\n').count(), 1, "{out}");
        assert!(err.contains("cannot list network interfaces"), "{err}");
    }

    #[test]
    fn where_says_host_on_a_bare_machine_and_exits_0() {
        let (code, out, err) = exec_on(&Machine::quiet(), &["where"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.contains("host"), "{out}");
    }

    #[test]
    fn where_json_is_one_compact_document_with_the_schema_version_first() {
        let (code, out, _) = exec_on(&Machine::quiet(), &["--json", "where"]);
        assert_eq!(code, 0);
        assert!(
            out.starts_with("{\"schema_version\":1,\"environment\":\"host\""),
            "{out}"
        );
        assert_eq!(out.matches('\n').count(), 1, "{out}");
    }

    #[test]
    fn where_ignores_the_family_flags() {
        let machine = Machine::laptop();
        let plain = exec_on(&machine, &["where"]);
        assert_eq!(exec_on(&machine, &["-4", "where"]), plain);
        assert_eq!(exec_on(&machine, &["-6", "where"]), plain);
    }

    #[test]
    fn where_exits_0_even_when_every_file_is_unreadable() {
        let mut machine = Machine::quiet();
        machine.env = FakeEnv::new()
            .unreadable("/proc/self/cgroup")
            .unreadable("/proc/self/mountinfo")
            .unreadable("/etc/resolv.conf");
        let (code, _, _) = exec_on(&machine, &["where"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn where_recognises_a_pod() {
        let mut machine = Machine::quiet();
        machine.env = FakeEnv::new().var("KUBERNETES_SERVICE_HOST", "10.96.0.1");
        let (code, out, _) = exec_on(&machine, &["--json", "where"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"environment\":\"kubernetes\""), "{out}");
    }

    #[test]
    fn all_prints_every_section_with_the_public_answer_and_exits_0() {
        let fetch = everyone_says(PUBLIC_IP);
        let (code, out, err) = run_on(&Machine::laptop().host(), &fetch, &["all"]);
        assert_eq!((code, err.as_str()), (0, ""));
        for heading in ["[public]", "[local]", "[where]", "[highlights]"] {
            assert!(out.contains(heading), "{heading} missing in\n{out}");
        }
        assert!(
            out.contains("1.2.3.4 (IPv4, observed by one.one.one.one)"),
            "{out}"
        );
        assert!(
            out.contains(
                "this machine sends from 192.168.1.23 (private) but the internet sees 1.2.3.4"
            ),
            "{out}"
        );
        assert_eq!(
            fetch.requested_urls(),
            [PROVIDERS[0].url],
            "without --verify the first valid answer ends the lookup"
        );
    }

    #[test]
    fn all_json_embeds_the_public_report_between_the_version_and_local() {
        let fetch = everyone_says(PUBLIC_IP);
        let (code, out, _) = run_on(&Machine::laptop().host(), &fetch, &["--json", "all"]);
        assert_eq!(code, 0);
        assert!(
            out.starts_with(
                "{\"schema_version\":1,\"public\":{\"ip\":\"1.2.3.4\",\"family\":\"ipv4\",\"provider\":\"one.one.one.one\","
            ),
            "{out}"
        );
        for key in [
            "\"error\":null,\"attempts\":[{",
            "\"local\":{",
            "\"where\":{",
            "\"highlights\":{\"nat\":{\"family\":\"ipv4\",\"local\":\"192.168.1.23\",\"public\":\"1.2.3.4\",\"translated\":true}",
        ] {
            assert!(out.contains(key), "{key} missing in {out}");
        }
        assert_eq!(out.matches('\n').count(), 1, "{out}");
        assert_eq!(out.matches("schema_version").count(), 1, "{out}");
    }

    #[test]
    fn all_verify_asks_every_provider_and_reports_the_agreement() {
        let fetch = everyone_says(PUBLIC_IP);
        let (code, out, err) = run_on(&Machine::laptop().host(), &fetch, &["all", "--verify"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(fetch.requested_urls().len(), PROVIDERS.len());
        assert!(out.contains("verified: 6 of 6 providers agree"), "{out}");

        let (code, out, _) = run_on(
            &Machine::laptop().host(),
            &everyone_says(PUBLIC_IP),
            &["--json", "all", "--verify"],
        );
        assert_eq!(code, 0);
        assert!(
            out.contains("\"verified\":{\"agree\":6,\"total\":6}"),
            "{out}"
        );
        assert!(out.contains("\"families\":[{"), "{out}");
    }

    #[test]
    fn all_verify_exits_3_on_a_disagreement_and_still_prints_every_section() {
        let answers = [
            Some(PUBLIC_IP),
            Some(OTHER_IP),
            Some(PUBLIC_IP),
            None,
            None,
            None,
        ];
        let (code, out, err) = run_on(
            &Machine::laptop().host(),
            &providers(answers),
            &["all", "--verify"],
        );
        assert_eq!(code, 3, "{err}");
        for heading in ["[public]", "[local]", "[where]", "[highlights]"] {
            assert!(out.contains(heading), "{heading} missing in\n{out}");
        }
        assert!(out.contains("DISAGREE"), "{out}");
        assert!(out.contains("api64.ipify.org"), "{out}");
        assert!(out.contains("5.6.7.8  <- differs"), "{out}");
        assert!(out.contains("NAT: unknown"), "{out}");
        assert!(
            err.contains("myip: public: providers disagree: ipv4"),
            "{err}"
        );

        let (code, out, _) = run_on(
            &Machine::laptop().host(),
            &providers(answers),
            &["--json", "all", "--verify"],
        );
        assert_eq!(code, 3);
        assert!(
            out.starts_with("{\"schema_version\":1,\"public\":{\"ip\":null,"),
            "{out}"
        );
        assert!(out.contains("\"status\":\"disagree\""), "{out}");
        assert!(out.contains("\"nat\":null"), "{out}");
        assert_eq!(out.matches('\n').count(), 1, "{out}");
    }

    #[test]
    fn all_verify_with_fewer_than_two_answers_is_reported_in_band_with_exit_0() {
        let one_answer = [Some(PUBLIC_IP), None, None, None, None, None];
        let (code, out, err) = run_on(
            &Machine::laptop().host(),
            &providers(one_answer),
            &["all", "--verify"],
        );
        assert_eq!(code, 0, "{err}");
        assert!(
            out.contains("[public]\n  unavailable: the address could not be verified"),
            "{out}"
        );
        assert!(
            err.contains("myip: public: the address could not be verified"),
            "{err}"
        );

        let (code, out, _) = run_on(
            &Machine::laptop().host(),
            &providers(one_answer),
            &["--json", "all", "--verify"],
        );
        assert_eq!(code, 0);
        assert!(out.contains("\"public\":{\"ip\":null,"), "{out}");
        assert!(
            out.contains("\"error\":\"the address could not be verified"),
            "{out}"
        );
    }

    #[test]
    fn all_never_exits_1_when_no_provider_answers() {
        for args in [&["all"][..], &["all", "--verify"], &["--json", "all"]] {
            let fetch = nobody_answers();
            let (code, out, err) = run_on(&Machine::laptop().host(), &fetch, args);
            assert_eq!(code, 0, "{args:?}: {err}");
            assert!(
                out.contains("\"local\":{") || out.contains("[local]"),
                "{args:?}: {out}"
            );
            assert!(
                err.contains("myip: public: no provider returned a valid public address"),
                "{args:?}: {err}"
            );
            assert_eq!(fetch.requested_urls().len(), PROVIDERS.len(), "{args:?}");
        }
    }

    #[test]
    fn all_never_exits_1_and_reports_failed_sections_on_stderr() {
        let (code, out, err) = run_on(
            &Machine::without_interface_access().host(),
            &nobody_answers(),
            &["all"],
        );
        assert_eq!(code, 0);
        assert!(out.contains("[local]"), "{out}");
        assert!(
            err.contains("myip: local: cannot list network interfaces"),
            "{err}"
        );
        assert!(
            err.contains("myip: public: no provider returned a valid public address"),
            "{err}"
        );
    }

    #[test]
    fn all_filters_the_public_lookup_by_family() {
        let fetch = everyone_says(PUBLIC_IPV6);
        let (code, out, _) = run_on(&Machine::laptop().host(), &fetch, &["--json", "all", "-6"]);
        assert_eq!(code, 0);
        assert!(
            out.contains("\"ip\":\"2606:4700:4700::1111\",\"family\":\"ipv6\""),
            "{out}"
        );
        let asked = fetch.requested_urls();
        for provider in PROVIDERS.iter().filter(|p| !p.ipv6) {
            assert!(
                !asked.contains(&provider.url.to_owned()),
                "{}",
                provider.name
            );
        }
        assert!(
            out.contains("\"nat\":null"),
            "an IPv6 answer has no IPv6 egress to compare: {out}"
        );
    }

    #[test]
    fn all_passes_the_family_to_the_local_section_through_run() {
        let local_section = |out: &str| {
            out.split("\"local\":")
                .nth(1)
                .and_then(|rest| rest.split(",\"where\":").next())
                .unwrap_or_else(|| panic!("no local section in {out}"))
                .to_owned()
        };
        for (flag, kept, dropped, target, answer) in [
            ("-4", "ipv4", "ipv6", PROBE_V4, PUBLIC_IP),
            ("-6", "ipv6", "ipv4", egress::PROBE_V6, PUBLIC_IPV6),
        ] {
            let machine = Machine::laptop();
            let fetch = everyone_says(answer);
            let (code, out, err) = run_on(&machine.host(), &fetch, &["--json", flag, "all"]);
            assert_eq!((code, err.as_str()), (0, ""), "{flag}");
            let local = local_section(&out);
            assert!(
                local.contains(&format!("\"egress\":{{\"{kept}\":")),
                "{flag}: {local}"
            );
            assert!(
                !local.contains(&format!("\"{dropped}\":")),
                "{flag}: {local}"
            );
            assert_eq!(machine.probe.asked(), [target], "{flag}");
        }
    }

    /// Everything that can be put into a variable, a file or an interface name and must
    /// never reach a terminal as itself.
    const HOSTILE: [&str; 6] = ["\u{1b}[2J", "\n", "\r", "\0", "\u{85}", "\u{202e}"];

    const SERVICE_ACCOUNT_NAMESPACE: &str =
        "/var/run/secrets/kubernetes.io/serviceaccount/namespace";

    /// Every place `local`, `where` and `all` take free text from.
    const SOURCES: [&str; 24] = [
        "env KUBERNETES_SERVICE_HOST",
        "env KUBERNETES_SERVICE_PORT",
        "env POD_NAME",
        "env POD_NAMESPACE",
        "env POD_IP",
        "env POD_IPS",
        "env NODE_NAME",
        "env HOST_IP",
        "serviceaccount namespace file",
        "resolv.conf search domain",
        "resolv.conf namespace",
        "resolv.conf nameserver",
        "cgroup docker id",
        "cgroup kubepods id",
        "cgroup lxc name",
        "mountinfo container id",
        "containerenv id",
        "systemd container name",
        "dockerenv with hostile cgroup",
        "interface name",
        "route table interface",
        "route table gateway",
        "ipv6 route table interface",
        "ipv6 route table gateway",
    ];

    fn machine_with(source: &str, payload: &str) -> Machine {
        let pod = || FakeEnv::new().var("KUBERNETES_SERVICE_HOST", "10.96.0.1");
        let mut machine = Machine::laptop();
        machine.probe = FakeProbe::new()
            .route(PROBE_V4, "192.168.1.23")
            .route(egress::PROBE_V6, "2001:db8::5");
        machine.interfaces = FakeInterfaces::with(vec![
            iface("lo", 1, &[("127.0.0.1", 8)]),
            iface("en0", 2, &[("192.168.1.23", 24), ("2001:db8::5", 64)]),
        ]);
        let env = match source {
            "env KUBERNETES_SERVICE_HOST" => FakeEnv::new().var("KUBERNETES_SERVICE_HOST", payload),
            "serviceaccount namespace file" => {
                FakeEnv::new().file(SERVICE_ACCOUNT_NAMESPACE, payload)
            }
            "resolv.conf search domain" => FakeEnv::new().file(
                "/etc/resolv.conf",
                &format!("search prod.svc.{payload} svc.{payload}\nnameserver 10.96.0.10\n"),
            ),
            "resolv.conf namespace" => FakeEnv::new().file(
                "/etc/resolv.conf",
                &format!("search {payload}.svc.cluster.local svc.cluster.local cluster.local\n"),
            ),
            "resolv.conf nameserver" => FakeEnv::new().file(
                "/etc/resolv.conf",
                &format!(
                    "search prod.svc.cluster.local svc.cluster.local\nnameserver {payload}\n"
                ),
            ),
            "cgroup docker id" => FakeEnv::new().file("/proc/self/cgroup", &format!("0::/docker/{payload}\n")),
            "cgroup kubepods id" => FakeEnv::new().file(
                "/proc/self/cgroup",
                &format!("0::/kubepods/besteffort/pod1/cri-containerd-{payload}.scope\n"),
            ),
            "cgroup lxc name" => FakeEnv::new().file("/proc/self/cgroup", &format!("0::/lxc/{payload}\n")),
            "mountinfo container id" => FakeEnv::new().file(
                "/proc/self/mountinfo",
                &format!("1 2 0:3 /var/lib/docker/containers/{payload}/hostname /etc/hostname rw - ext4 /dev/sda rw\n"),
            ),
            "containerenv id" => FakeEnv::new().file("/run/.containerenv", &format!("id=\"{payload}\"\n")),
            "systemd container name" => FakeEnv::new().file("/run/systemd/container", payload),
            "dockerenv with hostile cgroup" => FakeEnv::new()
                .file("/.dockerenv", "")
                .file("/proc/self/cgroup", &format!("0::/{payload}/kubepods\n")),
            "route table interface" => FakeEnv::new().file(
                "/proc/net/route",
                &format!("Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n{payload}\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n"),
            ),
            "route table gateway" => FakeEnv::new().file(
                "/proc/net/route",
                &format!("Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nen0\t00000000\t{payload}\t0003\t0\t0\t0\t00000000\t0\t0\t0\n"),
            ),
            "ipv6 route table interface" => FakeEnv::new().file(
                "/proc/net/ipv6_route",
                &format!("00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000a0027fffe7d0e5b 00000064 00000001 00000000 00450003 {payload}\n"),
            ),
            "ipv6 route table gateway" => FakeEnv::new().file(
                "/proc/net/ipv6_route",
                &format!("00000000000000000000000000000000 00 00000000000000000000000000000000 00 {payload} 00000064 00000001 00000000 00450003 en0\n"),
            ),
            "interface name" => FakeEnv::new(),
            other => other
                .strip_prefix("env ")
                .map_or_else(FakeEnv::new, |var| pod().var(var, payload)),
        };
        machine.env = env;
        if source == "interface name" {
            machine.interfaces = FakeInterfaces::with(vec![
                iface("lo", 1, &[("127.0.0.1", 8)]),
                iface(
                    &format!("en{payload}"),
                    2,
                    &[("192.168.1.23", 24), ("2001:db8::5", 64)],
                ),
            ]);
        }
        machine
    }

    #[test]
    fn no_free_text_reaches_a_terminal_unescaped_in_any_mode() {
        let commands: [&[&str]; 4] = [&["local"], &["where"], &["all"], &["all", "--verify"]];
        for source in SOURCES {
            for bad in HOSTILE {
                let payload = format!("a{bad}INJECT");
                for json in [false, true] {
                    for command in commands {
                        let machine = machine_with(source, &payload);
                        let mut args = vec!["-v"];
                        if json {
                            args.push("--json");
                        }
                        args.extend_from_slice(command);
                        let (_, out, err) =
                            run_on(&machine.host(), &everyone_says(PUBLIC_IP), &args);
                        let label = format!("{source} / {bad:?} / {args:?}");
                        for forbidden in ['\u{1b}', '\r', '\0', '\u{85}', '\u{202e}'] {
                            assert!(
                                !out.contains(forbidden) && !err.contains(forbidden),
                                "{label}: {forbidden:?} reached the terminal\nstdout: {out:?}\nstderr: {err:?}"
                            );
                        }
                        for line in out.lines().chain(err.lines()) {
                            assert!(
                                !line.trim_start().starts_with("INJECT"),
                                "{label}: the payload started a line of its own: {line:?}"
                            );
                        }
                        for line in err.lines() {
                            assert!(
                                line.starts_with("myip: "),
                                "{label}: stray stderr line {line:?}"
                            );
                        }
                        if json {
                            assert_eq!(out.matches('\n').count(), 1, "{label}: {out:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_hostile_interface_name_is_still_recognisable_in_text() {
        let machine = machine_with("interface name", "\u{1b}[2J0");
        let (code, out, err) = exec_on(&machine, &["local"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.contains("en\\x1b[2J0"), "{out:?}");
        assert!(out.contains("on en\\x1b[2J0"), "{out:?}");
        assert!(!out.contains('\u{1b}'), "{out:?}");
    }

    #[test]
    fn verbose_diagnostics_are_escaped_too() {
        let machine = machine_with("interface name", "\u{1b}[2J0");
        let (_, _, err) = exec_on(&machine, &["-v", "all"]);
        assert!(!err.contains('\u{1b}'), "{err:?}");
    }

    #[test]
    fn verbose_all_logs_each_provider_attempt() {
        let fetch = providers([None, Some(PUBLIC_IP), None, None, None, None]);
        let (code, _, err) = run_on(&Machine::laptop().host(), &fetch, &["-v", "all"]);
        assert_eq!(code, 0);
        assert!(
            err.contains("myip: one.one.one.one: connection refused"),
            "{err}"
        );
        assert!(err.contains("myip: api64.ipify.org: 1.2.3.4 ("), "{err}");
        assert!(err.contains("myip: read /proc/net/route"), "{err}");
    }

    #[test]
    fn verify_is_a_flag_of_all_and_documented_there() {
        let (code, out, _) = exec(&no_network(), &["all", "--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("--verify"), "{out}");
        for command in ["local", "where"] {
            let (_, out, _) = exec(&no_network(), &[command, "--help"]);
            assert!(!out.contains("--verify"), "{command}: {out}");
        }
        let (code, out, err) = exec(&no_network(), &["all", "--verify=yes"]);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(err.contains("error:"), "{err}");
    }

    #[test]
    fn verbose_lists_what_was_read_and_why_a_probe_failed() {
        let (code, out, err) = exec_on(&Machine::laptop(), &["-v", "local"]);
        assert_eq!(code, 0);
        assert!(!out.contains("/proc/"), "{out}");
        assert!(err.contains("myip: read /proc/net/route"), "{err}");

        let (_, _, err) = exec_on(&Machine::laptop(), &["-v", "where"]);
        assert!(err.contains("myip: read /proc/self/cgroup"), "{err}");
        assert!(err.contains("myip: stat /.dockerenv"), "{err}");

        let (_, _, quiet) = exec_on(&Machine::laptop(), &["where"]);
        assert_eq!(quiet, "");
    }

    #[test]
    fn no_command_opens_the_service_account_token() {
        const DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
        let fake = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .file(&format!("{DIR}/namespace"), "default\n")
            .file(&format!("{DIR}/token"), "S3CR3T-TOKEN")
            .file(&format!("{DIR}/ca.crt"), "S3CR3T-CERT");
        let commands: [&[&str]; 4] = [&["local"], &["where"], &["all"], &["all", "--verify"]];
        for command in commands {
            let spy = RecordingEnv::new(&fake);
            let machine = Machine::laptop();
            let host = Host {
                env: &spy,
                interfaces: &machine.interfaces,
                probe: &machine.probe,
            };
            let fetch = everyone_says(PUBLIC_IP);
            let args: Vec<&str> = ["--json", "-v"]
                .into_iter()
                .chain(command.iter().copied())
                .collect();
            let (code, out, err) = run_on(&host, &fetch, &args);
            assert_eq!(code, 0, "{command:?}: {err}");
            let touched: Vec<String> = spy.accesses().iter().map(ToString::to_string).collect();
            assert!(
                touched
                    .iter()
                    .all(|a| !a.contains("/token") && !a.contains("ca.crt")),
                "{command:?} touched a credential: {touched:?}"
            );
            assert!(!format!("{out}{err}").contains("S3CR3T"), "{command:?}");
            if command[0] == "all" {
                assert!(!fetch.requested_urls().is_empty(), "{command:?}");
            } else {
                assert!(
                    fetch.requested_urls().is_empty(),
                    "{command:?} must not use the network"
                );
            }
            if command[0] != "local" {
                assert!(
                    touched
                        .iter()
                        .any(|a| a.ends_with("serviceaccount/namespace")),
                    "{command:?} must read the namespace file or this test proves nothing: {touched:?}"
                );
            }
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
