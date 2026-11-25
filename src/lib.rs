//! Implementation detail of the `myip` binary. The API is not stable.

pub mod all;
pub mod cli;
pub mod egress;
pub mod env;
pub mod fact;
pub mod hint;
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
pub mod sanitize;
pub mod style;
pub mod version;
pub mod yaml;

use std::ffi::OsString;
use std::io::Write;

use clap::Parser;
use clap::error::ErrorKind;
use cli::{Cli, Command};
use egress::{EgressProbe, UdpProbe};
use env::{Env, SystemEnv};
use http::Fetch;
use ifaces::{Interfaces, SystemInterfaces};
use style::{Style, Terminal};

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
    /// What is known about stdout and the colour variables; only decides colour.
    pub terminal: Terminal,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

impl Context<'_> {
    /// How text on stdout is written. Documents are never styled, whatever `--color` says.
    pub fn style(&self, options: &cli::Options) -> Style {
        if options.format().is_document() {
            Style::Plain
        } else {
            Style::choose(options.color, &self.terminal)
        }
    }
}

/// Runs the CLI with the arguments after the program name and returns the exit code.
pub fn run(args: &[String], ctx: &mut Context<'_>) -> u8 {
    let cli =
        match Cli::try_parse_from(std::iter::once("myip").chain(args.iter().map(String::as_str))) {
            Ok(cli) => cli,
            Err(e) => {
                // `--help` and `--version` arrive here too; clap says which stream they belong on.
                // Only `--help` can be coloured, and only when `style::enabled` says so: the
                // rendered text without style is the form for everything else.
                let rendered = e.render();
                let text = if e.kind() == ErrorKind::DisplayHelp
                    && style::enabled(cli::prescan_color(args), &ctx.terminal)
                {
                    rendered.ansi().to_string()
                } else {
                    rendered.to_string()
                };
                let rendered = redact::redact_arg_queries(&redact::redact_userinfo(&text), args);
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
        (Some(Command::Public(public)), None) => match public.field.as_deref() {
            Some(field) => passthrough(Some(field), args, &cli.options, ctx),
            None => public::run(&cli.options, public.verify, ctx),
        },
        (Some(Command::Local), None) => local::run(&cli.options, ctx),
        (Some(Command::Where), None) => location::run(&cli.options, ctx),
        (Some(Command::All(all)), None) => all::run(&cli.options, all.verify, ctx),
        (Some(command), Some(_)) => {
            let _ = writeln!(
                ctx.err,
                "error: a FIELD cannot be combined with `{}`\n\nFor more information, try '--help'.",
                command.name()
            );
            2
        }
        (None, field) => passthrough(field, args, &cli.options, ctx),
    }
}

/// Prints what ipinfo.io returns for `field`. That body is passed through as it came, so
/// `--format` has nothing to shape and is a usage error here (ADR 0002), before any request.
/// `args` is the command line, so that an error body cannot echo a value from it.
fn passthrough(
    field: Option<&str>,
    args: &[String],
    options: &cli::Options,
    ctx: &mut Context<'_>,
) -> u8 {
    if options.format.is_some() {
        let _ = writeln!(
            ctx.err,
            "error: --format applies to `public`, `local`, `where` and `all`, not to the ipinfo.io \
             passthrough (a FIELD, or no command)\n\nFor more information, try '--help'."
        );
        return 2;
    }
    legacy::run(field, args, options, ctx)
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
    use crate::env::testing::FakeEnv;
    use crate::env::{Access, RecordingEnv};
    use crate::http::testing::{FakeFetch, RouteFetch, ok, reply};
    use crate::http::{Family, FetchError, FetchOptions, ProxyMode, Response};
    use crate::ifaces::testing::{FakeInterfaces, iface};
    use crate::public::PROVIDERS;
    use crate::style::testing::{assert_balanced, strip};

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

        /// A pod that declares some of its identity and leaves the rest unknown.
        fn pod() -> Self {
            Self {
                env: FakeEnv::new()
                    .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
                    .var("POD_NAME", "web-0")
                    .var("POD_IP", "10.244.1.7")
                    .var("HOST_IP", "192.168.49.2")
                    .file(SERVICE_ACCOUNT_NAMESPACE, "prod\n"),
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
        run_in(host, fetch, Terminal::default(), args)
    }

    /// Like [`run_on`], on a terminal with these facts.
    fn run_in(
        host: &Host<'_>,
        fetch: &dyn Fetch,
        terminal: Terminal,
        args: &[&str],
    ) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Context {
                fetch,
                host,
                terminal,
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
        providers_failing_with(answers, "connection refused")
    }

    fn providers_failing_with(answers: [Option<&str>; 6], reason: &str) -> RouteFetch {
        let routes = PROVIDERS
            .iter()
            .zip(answers)
            .map(|(provider, answer)| {
                let response = match answer {
                    Some(ip) if provider.name == "one.one.one.one" => {
                        reply(200, "text/plain", &format!("fl=1\nip={ip}\nts=1\n"))
                    }
                    Some(ip) => reply(200, "text/plain", &format!("{ip}\n")),
                    None => Err(FetchError::Transport(reason.to_owned())),
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
    fn version_prints_the_version_text() {
        let fetch = no_network();
        let (code, out, _) = exec(&fetch, &["--version"]);
        assert_eq!(code, 0);
        assert_eq!(out, format!("myip {}\n", *version::VERSION));
        assert!(
            out.starts_with(concat!("myip ", env!("CARGO_PKG_VERSION"))),
            "{out}"
        );
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
            "x#f",
            "caf\u{e9}",
            "a\nb",
            "a\\b",
            "//example.com",
            "///x",
            "a//b",
            "/",
            "?",
            "ip?x=1#f",
            "ip??x=1",
            "ip?x=1?y=2",
            "ip?x=a b",
            "ip?x=\"1\"",
            "ip?x=a/b",
            "ip?x=\u{e9}",
            "a/../b?x=1",
            "..?x=1",
            "//evil.example?x=1",
            "https://example.com",
            "https://evil.example?x=1",
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
            ("/ip", "https://ipinfo.io/ip"),
            ("json?token=abc123", "https://ipinfo.io/json?token=abc123"),
            (
                "8.8.8.8/org?token=abc",
                "https://ipinfo.io/8.8.8.8/org?token=abc",
            ),
            ("?token=abc", "https://ipinfo.io/?token=abc"),
            ("ip?", "https://ipinfo.io/ip"),
            (
                "ip?a=1&b=x%20y+z,w:v~._-",
                "https://ipinfo.io/ip?a=1&b=x%20y+z,w:v~._-",
            ),
        ];
        for (field, url) in cases {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, _, err) = exec(&fetch, &[field]);
            assert_eq!(code, 0, "{field:?}: {err}");
            assert_eq!(requested(&fetch), [url], "{field:?}");
        }
    }

    #[test]
    fn a_query_value_is_never_printed() {
        const SECRET: &str = "tok3n-S3CRET";

        // The verbose request line shows the parameter names only.
        let fetch = FakeFetch::new(vec![ok(200, "1.2.3.4\n")]);
        let (code, out, err) = exec(&fetch, &["-v", &format!("ip?token={SECRET}&x=1")]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"));
        assert!(
            err.contains("https://ipinfo.io/ip?token=***&x=***"),
            "{err}"
        );
        assert!(!err.contains(SECRET), "{err}");

        // So does a bare token.
        let fetch = FakeFetch::new(vec![ok(200, "1.2.3.4\n")]);
        let (_, _, err) = exec(&fetch, &["-v", &format!("?{SECRET}")]);
        assert!(err.contains("https://ipinfo.io/?***"), "{err}");
        assert!(!err.contains(SECRET), "{err}");

        // A transport error may quote the URL it was fetching.
        let url = format!("https://ipinfo.io/json?token={SECRET}");
        let fetch = FakeFetch::new(vec![Err(FetchError::Transport(format!(
            "GET {url} failed"
        )))]);
        let (code, out, err) = exec(&fetch, &[&format!("json?token={SECRET}")]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(
            err.contains("GET https://ipinfo.io/json?token=***"),
            "{err}"
        );
        assert!(!err.contains(SECRET), "{err}");

        // An error body may quote it too.
        let body = format!("{{\"error\":\"bad {url}\"}}");
        let fetch = FakeFetch::new(vec![ok(403, &body)]);
        let (code, out, err) = exec(&fetch, &[&format!("json?token={SECRET}")]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("HTTP 403"), "{err}");
        assert!(!err.contains(SECRET), "{err}");
    }

    #[test]
    fn an_error_body_that_echoes_a_query_value_does_not_print_it() {
        const SECRET: &str = "SECRET123";
        let body = r#"{"error":"token SECRET123 rejected"}"#;
        for args in [
            &["json?token=SECRET123"][..],
            &["public", "json?token=SECRET123"],
            &["-v", "ip?x=1&token=SECRET123"],
            &["json?token=SECRET%31%32%33"],
            &["json?token=SECRET123\n"],
        ] {
            let fetch = FakeFetch::new(vec![ok(403, body)]);
            let (code, out, err) = exec(&fetch, args);
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert!(!err.contains(SECRET), "{args:?}: {err}");
            assert!(
                err.contains(
                    r#"myip: ipinfo.io returned HTTP 403: {"error":"token *** rejected"}"#
                ),
                "{args:?}: {err}"
            );
        }
    }

    #[test]
    fn a_masked_value_that_breaks_a_url_does_not_hide_its_credentials_from_the_redaction() {
        // `:` is a legal query character; masking it first would destroy the `://`.
        let body = "proxy http://user:S3CR3T@host:1/x refused";
        let fetch = FakeFetch::new(vec![ok(403, body)]);
        let (code, out, err) = exec(&fetch, &["json?x=:"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("HTTP 403"), "{err}");
        assert!(!err.contains("S3CR3T"), "{err}");
    }

    #[test]
    fn a_value_that_the_credential_redaction_would_change_is_still_masked() {
        // The decoded value is `SECRET123://host?x=y`; the credential redaction alone would
        // turn the body into `SECRET123://host?x=***` and the value would no longer be found.
        let fetch = FakeFetch::new(vec![ok(403, "token SECRET123://host?x=y rejected")]);
        let (code, out, err) = exec(&fetch, &["json?token=SECRET123:%2F%2Fhost%3Fx%3Dy"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("HTTP 403: token *** rejected"), "{err}");
    }

    #[test]
    fn a_short_query_value_is_hidden_like_any_other() {
        let fetch = FakeFetch::new(vec![ok(403, r#"{"token":"abc"}"#)]);
        let (code, out, err) = exec(&fetch, &["json?token=abc"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(
            err.contains(r#"myip: ipinfo.io returned HTTP 403: {"token":"***"}"#),
            "{err}"
        );
    }

    #[test]
    fn a_query_value_cut_by_the_length_limit_does_not_leave_its_start_behind() {
        const SECRET: &str = "SECRET123";
        let body = format!("{}{SECRET} and more", "x".repeat(195));
        let fetch = FakeFetch::new(vec![ok(403, &body)]);
        let (code, _, err) = exec(&fetch, &[&format!("json?token={SECRET}")]);
        assert_eq!(code, 1);
        assert!(err.contains("HTTP 403"), "{err}");
        assert!(!err.contains("SECR"), "{err}");
    }

    #[test]
    fn an_error_body_is_printed_unchanged_when_nothing_on_the_command_line_is_a_query_value() {
        let body = r#"{"error":"token SECRET123 rejected, retry 1 of 12"}"#;
        for args in [
            &[][..],
            &["json"],
            &["-v", "public", "city"],
            // No value: nothing to mask.
            &["json?a="],
        ] {
            let fetch = FakeFetch::new(vec![ok(403, body)]);
            let (code, out, err) = exec(&fetch, args);
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert!(
                err.contains(&format!("myip: ipinfo.io returned HTTP 403: {body}\n")),
                "{args:?}: {err}"
            );
        }
    }

    const PROXY: &str = "http://proxy.test:3128";
    const PROXY_HINT: &str = "also applies to the proxy host";

    #[test]
    fn a_failed_host_lookup_with_a_family_and_a_proxy_says_the_proxy_host_is_the_one_resolved() {
        let failures = [
            FetchError::Resolve(
                "io: failed to lookup address information: Temporary failure in name resolution"
                    .to_owned(),
            ),
            FetchError::NoAddress("host not found".to_owned()),
        ];
        for (flag, example) in [("-6", "http://[::1]:PORT"), ("-4", "http://127.0.0.1:PORT")] {
            for failure in &failures {
                let fetch = FakeFetch::new(vec![Err(failure.clone())]);
                let (code, out, err) = exec(&fetch, &[flag, "--proxy", PROXY, "json"]);
                assert_eq!((code, out.as_str()), (1, ""), "{flag} {failure:?}");
                assert_eq!(err.matches(PROXY_HINT).count(), 1, "{flag}: {err}");
                assert!(err.starts_with(&format!("myip: {failure}")), "{err}");
                assert!(err.contains(example), "{flag}: {err}");
                assert!(err.contains(&format!("drop {flag}")), "{flag}: {err}");
            }
        }
    }

    #[test]
    fn the_proxy_hint_needs_a_family_a_proxy_url_and_a_failed_host_lookup() {
        let dns = || Err(FetchError::Resolve("lookup failed".to_owned()));
        let cases: [(&[&str], Result<crate::http::Response, FetchError>); 8] = [
            (&["--proxy", PROXY, "json"], dns()),
            (&["-6", "json"], dns()),
            (&["-6", "--proxy", "env", "json"], dns()),
            (&["-6", "--proxy", "none", "json"], dns()),
            (
                &["-6", "--proxy", PROXY, "json"],
                Err(FetchError::Transport("io: Connection refused".to_owned())),
            ),
            (
                &["-6", "--proxy", PROXY, "json"],
                Err(FetchError::Timeout("timeout: global".to_owned())),
            ),
            (&["-6", "--proxy", PROXY, "json"], ok(503, "down")),
            (&["-4", "--proxy", PROXY, "json"], ok(200, "{}\n")),
        ];
        for (args, answer) in cases {
            let fetch = FakeFetch::new(vec![answer]);
            let (_, _, err) = exec(&fetch, args);
            assert!(!err.contains(PROXY_HINT), "{args:?}: {err}");
        }
    }

    #[test]
    fn a_successful_body_is_passed_through_even_if_it_contains_a_url_query() {
        let body = "{\"readme\":\"https://ipinfo.io/missingauth?a=b\"}\n";
        let fetch = FakeFetch::new(vec![ok(200, body)]);
        let (code, out, _) = exec(&fetch, &["json?token=abc"]);
        assert_eq!((code, out.as_str()), (0, body));
    }

    #[test]
    fn no_argument_queries_json() {
        let fetch = FakeFetch::new(vec![ok(200, "{}\n")]);
        let (code, out, _) = exec(&fetch, &[]);
        assert_eq!((code, out.as_str()), (0, "{}\n"));
        assert_eq!(requested(&fetch), ["https://ipinfo.io/json"]);
    }

    #[test]
    fn public_with_a_field_is_the_legacy_passthrough() {
        let body = "Seattle\n";
        for field in [
            "city",
            "region",
            "country",
            "ip",
            "8.8.8.8/org",
            "/ip",
            "json?token=abc",
        ] {
            let direct = FakeFetch::new(vec![ok(200, body)]);
            let expected = exec(&direct, &[field]);
            let spelled = FakeFetch::new(vec![ok(200, body)]);
            let actual = exec(&spelled, &["public", field]);
            assert_eq!(actual, expected, "public {field}");
            assert_eq!(actual.0, 0, "public {field}: {}", actual.2);
            assert_eq!(requested(&spelled), requested(&direct), "public {field}");
            let (_, a) = spelled.requests.lock().unwrap()[0].clone();
            let (_, b) = direct.requests.lock().unwrap()[0].clone();
            assert_eq!(
                (a.timeout, a.max_bytes, a.family, a.proxy, a.accept),
                (b.timeout, b.max_bytes, b.family, b.proxy, b.accept),
                "public {field}"
            );
        }
    }

    #[test]
    fn public_with_a_field_honors_the_global_options_on_either_side() {
        let cases: [(&[&str], Family, u64); 5] = [
            (&["public", "-4", "city"], Family::V4, 10),
            (&["-6", "public", "city"], Family::V6, 10),
            (&["public", "city", "-4"], Family::V4, 10),
            (&["--timeout", "3", "public", "city"], Family::Any, 3),
            (&["public", "--timeout=7", "city"], Family::Any, 7),
        ];
        for (args, family, timeout) in cases {
            let fetch = FakeFetch::new(vec![ok(200, "Seattle\n")]);
            let (code, out, err) = exec(&fetch, args);
            assert_eq!((code, out.as_str()), (0, "Seattle\n"), "{args:?}: {err}");
            let (url, opts) = fetch.requests.lock().unwrap()[0].clone();
            assert_eq!(url, "https://ipinfo.io/city", "{args:?}");
            assert_eq!(opts.family, family, "{args:?}");
            assert_eq!(opts.timeout, Duration::from_secs(timeout), "{args:?}");
        }

        // --json changes nothing for a FIELD: the raw ipinfo body.
        let fetch = FakeFetch::new(vec![ok(200, "Seattle\n")]);
        let (code, out, _) = exec(&fetch, &["--json", "public", "city"]);
        assert_eq!((code, out.as_str()), (0, "Seattle\n"));
    }

    #[test]
    fn public_with_a_field_never_prints_a_query_value() {
        const SECRET: &str = "tok3n-S3CRET";
        let fetch = FakeFetch::new(vec![ok(200, "1.2.3.4\n")]);
        let (code, out, err) = exec(&fetch, &["-v", "public", &format!("ip?token={SECRET}")]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"));
        assert!(err.contains("https://ipinfo.io/ip?token=***"), "{err}");
        assert!(!err.contains(SECRET), "{err}");
        assert_eq!(
            requested(&fetch),
            [format!("https://ipinfo.io/ip?token={SECRET}")]
        );
    }

    #[test]
    fn public_with_a_field_rejects_combinations_that_cannot_mean_anything() {
        const SECRET: &str = "S3CR3T";
        let cases: [&[&str]; 8] = [
            &["public", "--verify", "city"],
            &["public", "city", "--verify"],
            &["public", "--verify", "json?token=S3CR3T"],
            &["city", "public", "ip"],
            &["public", "city", "ip"],
            &["public", "a b"],
            &["public", "../etc"],
            &["public", "//evil.example"],
        ];
        for args in cases {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!(code, 2, "{args:?}: {err}");
            assert_eq!(out, "", "{args:?}");
            assert!(err.contains("error:"), "{args:?}: {err}");
            assert!(!err.contains(SECRET), "{args:?}: {err}");
            assert!(requested(&fetch).is_empty(), "{args:?}");
        }
    }

    #[test]
    fn public_help_says_a_field_is_answered_by_ipinfo_alone() {
        let (code, out, _) = exec(&no_network(), &["public", "--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("[FIELD]"), "{out}");
        assert!(out.contains("ipinfo.io"), "{out}");
        assert!(out.contains("not verified"), "{out}");
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
    fn a_query_value_is_never_echoed_in_usage_errors() {
        const SECRET: &str = "S3CR3T";
        let cases: [&[&str]; 8] = [
            &["json?token=S3CR3T\"x"],
            &["json?token=S3CR3T#f"],
            &["json?token=S3CR3T x"],
            &["json?token=S3CR3T/x"],
            &["ip", "json?token=S3CR3T"],
            &["local", "json?token=S3CR3T"],
            &["--timeout", "x?token=S3CR3T"],
            &["--timeout=x?token=S3CR3T"],
        ];
        for args in cases {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!(code, 2, "{args:?}: {err}");
            assert!(!format!("{out}{err}").contains(SECRET), "{args:?}: {err}");
            assert!(err.contains("?token=***"), "{args:?}: {err}");
            assert!(requested(&fetch).is_empty(), "{args:?}");
        }
        // clap cuts an unknown long flag at `=`, so only the name is echoed.
        let (code, out, err) = exec(&no_network(), &["--no-such-flag?token=S3CR3T"]);
        assert_eq!(code, 2, "{err}");
        assert!(!format!("{out}{err}").contains(SECRET), "{err}");
    }

    #[test]
    fn verify_and_extra_arguments_are_only_valid_for_public_and_all() {
        let cases: [&[&str]; 5] = [
            &["--verify"],
            &["ip", "--verify"],
            &["local", "--verify"],
            &["where", "--verify"],
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
    fn all_verify_with_answers_from_one_operator_is_reported_in_band_with_exit_0() {
        let cloudflare_only = [Some(PUBLIC_IP), None, None, Some(PUBLIC_IP), None, None];
        let (code, out, err) = run_on(
            &Machine::laptop().host(),
            &providers(cloudflare_only),
            &["all", "--verify"],
        );
        assert_eq!(code, 0, "{err}");
        assert!(
            out.contains("[public]\n  unavailable: the address could not be verified"),
            "{out}"
        );
        assert!(
            err.contains(
                "myip: public: the address could not be verified: fewer than two independent \
                 operators answered in any address family"
            ),
            "{err}"
        );

        let (code, out, _) = run_on(
            &Machine::laptop().host(),
            &providers(cloudflare_only),
            &["--json", "all", "--verify"],
        );
        assert_eq!(code, 0);
        assert!(out.contains("\"public\":{\"ip\":null,"), "{out}");
        assert!(
            out.contains(
                "\"families\":[{\"family\":\"ipv4\",\"status\":\"unverified\",\"ip\":\"1.2.3.4\",\
                 \"agree\":2,\"total\":2,\"operators\":1,\"providers\":["
            ),
            "{out}"
        );
        assert!(out.contains("\"operator\":\"cloudflare\""), "{out}");
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
    const HOSTILE: [&str; 9] = [
        "\u{1b}[2J",
        "\n",
        "\r",
        "\0",
        "\u{85}",
        "\u{202e}",
        "\u{2028}",
        "\u{2029}",
        "\u{feff}",
    ];

    const SERVICE_ACCOUNT_NAMESPACE: &str =
        "/var/run/secrets/kubernetes.io/serviceaccount/namespace";

    /// Every place `local`, `where` and `all` take free text from.
    const SOURCES: [&str; 25] = [
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
        "shared address interface names",
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
            "interface name" | "shared address interface names" => FakeEnv::new(),
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
        // The one place interface names reach stderr: the note about an egress address that
        // two interfaces carry.
        if source == "shared address interface names" {
            machine.interfaces = FakeInterfaces::with(vec![
                iface(&format!("en{payload}"), 2, &[("192.168.1.23", 24)]),
                iface(&format!("vip{payload}"), 3, &[("192.168.1.23", 32)]),
            ]);
        }
        machine
    }

    #[test]
    fn no_free_text_reaches_a_terminal_unescaped_in_any_mode() {
        let commands: [&[&str]; 4] = [&["local"], &["where"], &["all"], &["all", "--verify"]];
        let mut on_stderr = std::collections::BTreeSet::new();
        for source in SOURCES {
            for bad in HOSTILE {
                let payload = format!("a{bad}INJECT");
                for format in [None, Some("json"), Some("json-pretty"), Some("yaml")] {
                    for command in commands {
                        let machine = machine_with(source, &payload);
                        let mut args = vec!["-v"];
                        if let Some(format) = format {
                            args.extend(["--format", format]);
                        }
                        args.extend_from_slice(command);
                        let (_, out, err) =
                            run_on(&machine.host(), &everyone_says(PUBLIC_IP), &args);
                        let label = format!("{source} / {bad:?} / {args:?}");
                        if err.contains("INJECT") {
                            on_stderr.insert((source, bad));
                        }
                        for forbidden in [
                            '\u{1b}', '\r', '\0', '\u{85}', '\u{202e}', '\u{2028}', '\u{2029}',
                            '\u{feff}', '\u{fffe}', '\u{ffff}',
                        ] {
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
                        if format == Some("json") {
                            assert_eq!(out.matches('\n').count(), 1, "{label}: {out:?}");
                        }
                    }
                }
            }
        }
        // The stderr checks above only mean something if hostile text did reach stderr.
        for bad in HOSTILE {
            assert!(
                on_stderr.contains(&("shared address interface names", bad)),
                "no run put {bad:?} on stderr, so the stderr assertions proved nothing"
            );
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
        let (_, _, err) = run_on(&machine.host(), &everyone_says(PUBLIC_IP), &["-v", "all"]);
        assert!(err.contains("myip: read /proc/net/route"), "{err:?}");
        assert!(!err.contains('\u{1b}'), "{err:?}");
    }

    const WORDED: &str = "a\u{1b}[2J\n\u{202e}b";
    const WORDED_ESCAPED: &str = "a\\x1b[2J\\n\\u{202e}b";

    fn assert_stderr_is_only_escaped_lines(err: &str, label: &str) {
        assert!(!err.is_empty(), "{label}: nothing on stderr");
        for forbidden in ['\u{1b}', '\r', '\0', '\u{85}', '\u{202e}'] {
            assert!(!err.contains(forbidden), "{label}: {err:?}");
        }
        for line in err.lines() {
            assert!(line.starts_with("myip: "), "{label}: {line:?}");
        }
    }

    #[test]
    fn a_hostile_interface_error_reaches_stderr_only_as_visible_escapes() {
        let mut machine = Machine::laptop();
        machine.interfaces = FakeInterfaces::failing_with(std::io::ErrorKind::Other, WORDED);
        for args in [&["local"][..], &["--json", "local"]] {
            let (code, out, err) = run_on(&machine.host(), &no_network(), args);
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(
                err,
                format!("myip: cannot list network interfaces: {WORDED_ESCAPED}\n"),
                "{args:?}"
            );
            assert!(!out.contains('\u{1b}'), "{args:?}: {out:?}");
        }
        for args in [&["all"][..], &["--json", "all"]] {
            let (code, out, err) = run_on(&machine.host(), &everyone_says(PUBLIC_IP), args);
            assert_eq!(code, 0, "{args:?}");
            assert_eq!(
                err,
                format!("myip: local: cannot list network interfaces: {WORDED_ESCAPED}\n"),
                "{args:?}"
            );
            assert!(!out.contains('\u{1b}'), "{args:?}: {out:?}");
        }
    }

    #[test]
    fn a_hostile_provider_error_reaches_stderr_only_as_visible_escapes() {
        let routes = PROVIDERS
            .iter()
            .map(|provider| {
                (
                    provider.url,
                    Err(FetchError::Transport(format!(
                        "{WORDED} ({})",
                        provider.name
                    ))),
                )
            })
            .collect();
        let fetch = RouteFetch::new(routes);
        let (code, _, err) = run_on(&Machine::laptop().host(), &fetch, &["-v", "all"]);
        assert_eq!(code, 0);
        assert_stderr_is_only_escaped_lines(&err, "-v all");
        for provider in PROVIDERS.iter() {
            let prefix = format!("myip: {}: a\\x1b[2J", provider.name);
            let line = err
                .lines()
                .find(|line| line.starts_with(&prefix))
                .unwrap_or_else(|| panic!("{}: {err:?}", provider.name));
            assert!(line.contains("\\u{202e}b"), "{line:?}");
        }
        let (code, _, err) = run_on(&Machine::laptop().host(), &fetch, &["all"]);
        assert_eq!(code, 0);
        assert_stderr_is_only_escaped_lines(&err, "all");
    }

    #[test]
    fn a_hostile_ipinfo_error_reaches_stderr_only_as_visible_escapes() {
        let transport = FakeFetch::new(vec![Err(FetchError::Transport(WORDED.to_owned()))]);
        let (code, out, err) = exec(&transport, &["ip"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert_stderr_is_only_escaped_lines(&err, "transport error");
        assert_eq!(err, format!("myip: {WORDED_ESCAPED}\n"));

        let body = FakeFetch::new(vec![ok(403, WORDED)]);
        let (code, out, err) = exec(&body, &["ip"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert_stderr_is_only_escaped_lines(&err, "error body");
        assert!(err.contains("HTTP 403: a\\x1b[2J"), "{err:?}");
    }

    #[test]
    fn a_verbose_note_that_names_interfaces_shows_their_control_characters_visibly() {
        let machine = machine_with("shared address interface names", "\u{1b}[2J\n");
        let (code, _, err) = run_on(&machine.host(), &everyone_says(PUBLIC_IP), &["-v", "local"]);
        assert_eq!(code, 0);
        assert_stderr_is_only_escaped_lines(&err, "-v local");
        assert!(
            err.contains("carried by several interfaces (en\\x1b[2J\\n, vip\\x1b[2J\\n)"),
            "{err:?}"
        );
    }

    /// An error text that tries to end its own line and start the report's headings and results.
    const FORGED: &str = "boom\n[highlights]\nfoo\r\n[where]\nbar";
    const FORGED_ESCAPED: &str = "boom\\n[highlights]\\nfoo\\r\\n[where]\\nbar";

    /// The report's own layout: `headings` appear once each, and no line is one the forged
    /// text would have started.
    fn assert_layout_is_the_programs_own(out: &str, headings: &[&str], label: &str) {
        for heading in headings {
            let count = out.lines().filter(|line| line == heading).count();
            assert_eq!(count, 1, "{label}: {heading}\n{out}");
        }
        for line in out.lines() {
            assert!(
                !line.starts_with("foo") && !line.starts_with("bar"),
                "{label}: forged line {line:?}\n{out}"
            );
        }
        assert!(!out.contains('\r'), "{label}: {out:?}");
    }

    fn assert_one_json_line(out: &str, label: &str) {
        assert_eq!(out.matches('\n').count(), 1, "{label}: {out:?}");
        assert!(out.ends_with('\n'), "{label}: {out:?}");
    }

    #[test]
    fn a_hostile_interface_error_cannot_forge_the_layout_on_stdout() {
        let mut machine = Machine::laptop();
        machine.interfaces = FakeInterfaces::failing_with(std::io::ErrorKind::Other, FORGED);

        let (code, out, _) = run_on(&machine.host(), &no_network(), &["local"]);
        assert_eq!(code, 1);
        assert!(
            !out.contains("[highlights]") && !out.contains("[where]"),
            "{out}"
        );
        assert_layout_is_the_programs_own(&out, &[], "local");

        let headings = ["[public]", "[local]", "[where]", "[highlights]"];
        for args in [&["all"][..], &["all", "--verify"]] {
            let (code, out, _) = run_on(&machine.host(), &everyone_says(PUBLIC_IP), args);
            assert_eq!(code, 0, "{args:?}");
            assert_layout_is_the_programs_own(&out, &headings, &format!("{args:?}"));
            assert!(
                out.contains(&format!(
                    "  unavailable: cannot list network interfaces: {FORGED_ESCAPED}\n"
                )),
                "{args:?}: {out}"
            );
        }
    }

    #[test]
    fn a_hostile_interface_error_keeps_the_json_on_one_line() {
        let mut machine = Machine::laptop();
        machine.interfaces = FakeInterfaces::failing_with(std::io::ErrorKind::Other, FORGED);

        let (code, out, _) = run_on(&machine.host(), &no_network(), &["--json", "local"]);
        assert_eq!(code, 1);
        assert_one_json_line(&out, "local");
        assert!(out.contains(FORGED_ESCAPED), "{out}");

        let (code, out, _) = run_on(
            &machine.host(),
            &everyone_says(PUBLIC_IP),
            &["--json", "all"],
        );
        assert_eq!(code, 0);
        assert_one_json_line(&out, "all");
        assert!(out.contains(FORGED_ESCAPED), "{out}");
    }

    #[test]
    fn a_hostile_provider_error_cannot_forge_the_layout_on_stdout() {
        let answers = [
            Some(PUBLIC_IP),
            Some(OTHER_IP),
            Some(PUBLIC_IP),
            None,
            None,
            None,
        ];
        let fetch = providers_failing_with(answers, FORGED);
        let headings = ["[public]", "[local]", "[where]", "[highlights]"];
        let (code, out, _) = run_on(&Machine::laptop().host(), &fetch, &["all", "--verify"]);
        assert_eq!(code, 3);
        assert_layout_is_the_programs_own(&out, &headings, "all --verify");
        assert!(out.contains("no answer:"), "{out}");

        let (code, out, _) = run_on(
            &Machine::laptop().host(),
            &fetch,
            &["--json", "all", "--verify"],
        );
        assert_eq!(code, 3);
        assert_one_json_line(&out, "all --verify");
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

    // ---- IPINFO_TOKEN (ADR 0004) ----

    const TOKEN_VALUE: &str = "Q7abc.DEF_123";

    /// A machine whose environment has `IPINFO_TOKEN` set to `value`.
    fn machine_with_token(value: &str) -> Machine {
        Machine {
            env: FakeEnv::new().var("IPINFO_TOKEN", value),
            ..Machine::quiet()
        }
    }

    fn token_run(value: &str, fetch: &dyn Fetch, args: &[&str]) -> (u8, String, String) {
        run_on(&machine_with_token(value).host(), fetch, args)
    }

    /// The bearer each recorded request was made with.
    fn bearers(fetch: &FakeFetch) -> Vec<Option<String>> {
        fetch
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|(_, options)| options.bearer.as_ref().map(|b| b.expose().to_owned()))
            .collect()
    }

    fn asked_for_the_token(spy: &RecordingEnv<'_>) -> bool {
        spy.accesses()
            .iter()
            .any(|a| matches!(a, Access::Var(name) if name == "IPINFO_TOKEN"))
    }

    #[test]
    fn the_variable_is_sent_as_a_bearer_with_the_one_request_and_never_in_the_url() {
        for args in [
            &["json"][..],
            &["city"],
            &["8.8.8.8/org"],
            &["public", "json"],
            &["public", "ip?x=1"],
            &[],
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, out, err) = token_run(TOKEN_VALUE, &fetch, args);
            assert_eq!(
                (code, out.as_str(), err.as_str()),
                (0, "x\n", ""),
                "{args:?}"
            );
            assert_eq!(bearers(&fetch), [Some(TOKEN_VALUE.to_owned())], "{args:?}");
            let urls = requested(&fetch);
            assert_eq!(urls.len(), 1, "{args:?}: {urls:?}");
            assert!(
                !urls[0].to_ascii_lowercase().contains("token") && !urls[0].contains(TOKEN_VALUE),
                "{args:?}: {urls:?}"
            );
        }
    }

    #[test]
    fn without_the_variable_no_bearer_is_sent() {
        let fetch = FakeFetch::new(vec![ok(200, "x")]);
        assert_eq!(exec(&fetch, &["city"]).0, 0);
        assert_eq!(bearers(&fetch), [None]);
    }

    #[test]
    fn an_explicit_token_parameter_wins_and_the_variable_is_neither_read_nor_checked() {
        // Without this the checks below prove nothing: the same variable is used otherwise.
        let fetch = FakeFetch::new(vec![ok(200, "x")]);
        token_run(TOKEN_VALUE, &fetch, &["json"]);
        assert_eq!(bearers(&fetch), [Some(TOKEN_VALUE.to_owned())]);

        for value in [TOKEN_VALUE, "a b", "bad\u{1}", ""] {
            for field in [
                "json?token=U",
                "json?TOKEN=U",
                "json?ToKeN=U",
                "json?%74oken=U",
                "json?%54OKEN=U",
                "json?token=",
                "json?token",
                "json?a=1&token=U",
                "json?a=1&token",
                "?token=U",
            ] {
                let fake = FakeEnv::new().var("IPINFO_TOKEN", value);
                let spy = RecordingEnv::new(&fake);
                let machine = Machine::quiet();
                let host = Host {
                    env: &spy,
                    interfaces: &machine.interfaces,
                    probe: &machine.probe,
                };
                let fetch = FakeFetch::new(vec![ok(200, "x")]);
                let (code, out, err) = run_on(&host, &fetch, &["-v", field]);
                assert_eq!(
                    (code, out.as_str()),
                    (0, "x\n"),
                    "{value:?} {field:?}: {err}"
                );
                assert_eq!(
                    requested(&fetch),
                    [format!("https://ipinfo.io/{field}")],
                    "{value:?} {field:?}"
                );
                assert_eq!(bearers(&fetch), [None], "{value:?} {field:?}");
                assert!(!asked_for_the_token(&spy), "{value:?} {field:?}");
                assert!(!err.contains("Authorization"), "{value:?} {field:?}: {err}");
            }
        }
    }

    #[test]
    fn a_parameter_that_is_not_named_token_does_not_stop_the_variable() {
        for field in [
            "json?tokens=U",
            "json?x=token",
            "json?my_token=U",
            "json?token_=U",
            "json?%74oken%20=U",
            "json?a=1&mytoken=2",
            "token",
            "json",
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, _, err) = token_run(TOKEN_VALUE, &fetch, &[field]);
            assert_eq!(code, 0, "{field:?}: {err}");
            assert_eq!(
                requested(&fetch),
                [format!("https://ipinfo.io/{field}")],
                "{field:?}"
            );
            assert_eq!(bearers(&fetch), [Some(TOKEN_VALUE.to_owned())], "{field:?}");
        }
    }

    #[test]
    fn only_the_passthrough_asks_for_the_variable() {
        for value in [TOKEN_VALUE, "a b", ""] {
            let fake = FakeEnv::new().var("IPINFO_TOKEN", value);
            let machine = Machine::laptop();

            // Without this the commands below prove nothing.
            let spy = RecordingEnv::new(&fake);
            let host = Host {
                env: &spy,
                interfaces: &machine.interfaces,
                probe: &machine.probe,
            };
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            run_on(&host, &fetch, &["city"]);
            assert!(
                asked_for_the_token(&spy),
                "the passthrough reads it: {value:?}"
            );

            let commands: [&[&str]; 8] = [
                &["public"],
                &["public", "--verify"],
                &["local"],
                &["where"],
                &["all"],
                &["all", "--verify"],
                &["--json", "-v", "public"],
                &["-v", "--format", "yaml", "all"],
            ];
            for command in commands {
                let spy = RecordingEnv::new(&fake);
                let host = Host {
                    env: &spy,
                    interfaces: &machine.interfaces,
                    probe: &machine.probe,
                };
                let fetch = everyone_says(PUBLIC_IP);
                let (code, out, err) = run_on(&host, &fetch, command);
                assert_eq!(code, 0, "{value:?} {command:?}: {err}");
                assert!(!asked_for_the_token(&spy), "{value:?} {command:?}");
                let sent = fetch.requests.lock().unwrap();
                assert!(
                    sent.iter().all(|(_, options)| options.bearer.is_none()),
                    "{value:?} {command:?}"
                );
                if command[0] != "local" && command[0] != "where" {
                    assert!(!sent.is_empty(), "{command:?}");
                }
                if value == TOKEN_VALUE {
                    assert!(!format!("{out}{err}").contains(value), "{command:?}");
                }
            }
        }
    }

    #[test]
    fn verbose_prints_one_fixed_line_about_the_token_and_never_the_value() {
        for args in [&["-v", "city"][..], &["-v", "public", "city"]] {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, out, err) = token_run(TOKEN_VALUE, &fetch, args);
            assert_eq!((code, out.as_str()), (0, "x\n"), "{args:?}");
            assert_eq!(
                err,
                "myip: GET https://ipinfo.io/city\n\
                 myip: Authorization: Bearer *** (from IPINFO_TOKEN)\n\
                 myip: HTTP 200, 1 bytes\n",
                "{args:?}"
            );
        }
    }

    #[test]
    fn verbose_says_nothing_about_a_token_that_is_not_sent() {
        // Without this the checks below prove nothing.
        let fetch = FakeFetch::new(vec![ok(200, "x")]);
        let (_, _, err) = token_run(TOKEN_VALUE, &fetch, &["-v", "city"]);
        assert!(err.contains("Authorization: Bearer ***"), "{err}");

        for (value, args) in [
            ("", &["-v", "city"][..]),
            (" \t\r\n", &["-v", "city"]),
            (TOKEN_VALUE, &["-v", "city?token=U"]),
            (TOKEN_VALUE, &["-v", "city?TOKEN=U"]),
            (TOKEN_VALUE, &["-v", "public", "city?%74oken=U"]),
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, _, err) = token_run(value, &fetch, args);
            assert_eq!(code, 0, "{args:?}: {err}");
            assert!(
                !err.contains("Authorization") && !err.contains("IPINFO_TOKEN"),
                "{args:?}: {err}"
            );
            assert!(!err.contains(TOKEN_VALUE), "{args:?}: {err}");
        }
        for args in [&["-v", "where"][..], &["-v", "local"]] {
            let (_, _, err) = run_on(
                &Machine {
                    env: FakeEnv::new().var("IPINFO_TOKEN", TOKEN_VALUE),
                    ..Machine::laptop()
                }
                .host(),
                &no_network(),
                args,
            );
            assert!(
                !err.contains("Authorization") && !err.contains(TOKEN_VALUE),
                "{err}"
            );
        }
    }

    #[test]
    fn an_error_body_that_echoes_the_variable_does_not_print_it() {
        let echoing = format!(
            r#"{{"status":403,"error":{{"title":"Unknown token","message":"{TOKEN_VALUE} is not known"}}}}"#
        );
        for args in [&["json"][..], &["-v", "json"], &["public", "city"]] {
            let fetch = FakeFetch::new(vec![ok(403, &echoing)]);
            let (code, out, err) = token_run(TOKEN_VALUE, &fetch, args);
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert_eq!(bearers(&fetch), [Some(TOKEN_VALUE.to_owned())], "{args:?}");
            assert!(
                err.contains(r#""message":"*** is not known""#),
                "{args:?}: {err}"
            );
            assert!(
                !err.contains(TOKEN_VALUE) && !err.contains("Q7"),
                "{args:?}: {err}"
            );
        }
    }

    #[test]
    fn a_value_in_an_error_body_that_the_length_limit_would_cut_does_not_leave_its_start() {
        let body = format!("{} {TOKEN_VALUE}", "x".repeat(196));
        let fetch = FakeFetch::new(vec![ok(500, &body)]);
        let (code, _, err) = token_run(TOKEN_VALUE, &fetch, &["json"]);
        assert_eq!(code, 1);
        assert_eq!(bearers(&fetch), [Some(TOKEN_VALUE.to_owned())]);
        assert!(!err.contains("Q7") && err.contains(" ***"), "{err}");
    }

    #[test]
    fn a_transport_failure_that_echoes_the_variable_does_not_print_it() {
        let text = format!("connection to {TOKEN_VALUE} refused");
        for error in [
            FetchError::Transport(text.clone()),
            FetchError::Rejected(text.clone()),
            FetchError::Resolve(text.clone()),
            FetchError::NoAddress(text.clone()),
        ] {
            let fetch = FakeFetch::new(vec![Err(error.clone())]);
            let (code, out, err) = token_run(TOKEN_VALUE, &fetch, &["-v", "json"]);
            assert_eq!((code, out.as_str()), (1, ""), "{error:?}");
            assert_eq!(bearers(&fetch), [Some(TOKEN_VALUE.to_owned())], "{error:?}");
            assert!(
                err.contains("connection to *** refused"),
                "{error:?}: {err}"
            );
            assert!(
                !err.contains(TOKEN_VALUE) && !err.contains("Q7"),
                "{error:?}: {err}"
            );
        }
    }

    #[test]
    fn the_options_a_request_carries_do_not_show_the_value_in_debug_output() {
        let fetch = FakeFetch::new(vec![ok(200, "x")]);
        token_run(TOKEN_VALUE, &fetch, &["json"]);
        let recorded = fetch.requests.lock().unwrap();
        let options = &recorded[0].1;
        assert!(format!("{options:?}").contains("bearer: Some(***)"));
        for shown in [
            format!("{options:?}"),
            format!("{options:#?}"),
            format!("{recorded:?}"),
            format!("{:?}", options.bearer),
            format!("{}", options.bearer.as_ref().unwrap()),
        ] {
            assert!(
                shown.contains("***") && !shown.contains(TOKEN_VALUE),
                "{shown}"
            );
        }
    }

    #[test]
    fn a_successful_answer_is_the_servers_and_is_passed_through_raw() {
        let body = format!(r#"{{"ip":"1.2.3.4","echo":"{TOKEN_VALUE}"}}"#);
        let fetch = FakeFetch::new(vec![ok(200, &body)]);
        let (code, out, err) = token_run(TOKEN_VALUE, &fetch, &["json"]);
        assert_eq!(bearers(&fetch), [Some(TOKEN_VALUE.to_owned())]);
        assert_eq!((code, out, err), (0, format!("{body}\n"), String::new()));
    }

    #[test]
    fn whitespace_around_the_value_is_trimmed_and_a_blank_one_is_unset() {
        for (value, want) in [
            (TOKEN_VALUE.to_owned(), Some(TOKEN_VALUE)),
            (format!("{TOKEN_VALUE}\n"), Some(TOKEN_VALUE)),
            (format!(" \t{TOKEN_VALUE}\r\n"), Some(TOKEN_VALUE)),
            (format!("\u{c}{TOKEN_VALUE}\u{c}"), Some(TOKEN_VALUE)),
            (String::new(), None),
            (" ".to_owned(), None),
            (" \t\r\n\u{c}".to_owned(), None),
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, out, err) = token_run(&value, &fetch, &["json"]);
            assert_eq!(
                (code, out.as_str(), err.as_str()),
                (0, "x\n", ""),
                "{value:?}"
            );
            assert_eq!(bearers(&fetch), [want.map(str::to_owned)], "{value:?}");
        }
    }

    #[test]
    fn every_allowed_character_and_the_longest_length_are_accepted() {
        let longest = "a".repeat(256);
        for value in [
            "ABCXYZabcxyz0123456789",
            ".",
            "_",
            "~",
            "+",
            "/",
            "=",
            "-",
            "a.b_c~d+e/f=g-h",
            "abc==",
            longest.as_str(),
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "x")]);
            let (code, _, err) = token_run(value, &fetch, &["json"]);
            assert_eq!(code, 0, "{value:?}: {err}");
            assert_eq!(bearers(&fetch), [Some(value.to_owned())], "{value:?}");
        }
    }

    #[test]
    fn an_invalid_value_is_a_usage_error_that_shows_nothing_about_it_and_asks_nothing() {
        let too_long_257 = format!("Qz9{}", "a".repeat(254));
        let too_long_300 = format!("Qz9{}", "a".repeat(297));
        let mut values: Vec<String> = [
            "a b",
            "a\tb",
            "a\u{1}b",
            "a\u{b}",
            "a\u{7f}b",
            "a\u{0}b",
            "é",
            "a\u{a0}b",
            "\u{feff}abc",
            "a\"b",
            "a,b",
            "a;b",
            "a\\b",
            "a:b",
            "a@b",
            "a%41b",
            "a?b",
            "a&b",
            "a#b",
            "a!b",
            "a$b",
            "a*b",
            "a'b",
            "a(b",
            "a{b",
            "a|b",
            "a^b",
            "a<b",
            "a\u{1f600}",
        ]
        .map(|bad| format!("Qz9{bad}Xw8"))
        .into();
        values.push(too_long_257);
        values.push(too_long_300);
        let mut messages = std::collections::BTreeSet::new();
        for value in &values {
            for args in [
                &["city"][..],
                &["-v", "city"],
                &["public", "city"],
                &[],
                &["-v", "json?a=1"],
            ] {
                // An answer is ready so that a request made by mistake fails the assertions below.
                let fetch = FakeFetch::new(vec![ok(200, "x")]);
                let (code, out, err) = token_run(value, &fetch, args);
                assert_eq!(code, 2, "{value:?} {args:?}");
                assert_eq!(out, "", "{value:?} {args:?}");
                assert!(requested(&fetch).is_empty(), "{value:?} {args:?}");
                assert!(
                    err.starts_with("error: ")
                        && err.contains("IPINFO_TOKEN")
                        && err.contains("256")
                        && err.lines().count() == 1,
                    "{value:?} {args:?}: {err}"
                );
                for shown in [
                    "Qz9",
                    "Xw8",
                    "257",
                    "300",
                    "\u{1f600}",
                    "\u{a0}",
                    "\u{feff}",
                ] {
                    assert!(!err.contains(shown), "{value:?} {args:?}: {err}");
                }
                messages.insert(err);
            }
        }
        assert_eq!(
            messages.len(),
            1,
            "one message for every invalid value: {messages:?}"
        );
    }

    #[test]
    fn the_format_usage_error_comes_before_the_value_check() {
        let fetch = no_network();
        let (code, _, err) = token_run("a b", &fetch, &["--format", "yaml", "city"]);
        assert_eq!(code, 2);
        assert!(
            err.contains("--format") && !err.contains("IPINFO_TOKEN"),
            "{err}"
        );
    }

    // ---- --format (ADR 0002) ----

    type Fixture = fn() -> RouteFetch;

    fn agree() -> RouteFetch {
        everyone_says(PUBLIC_IP)
    }

    fn split() -> RouteFetch {
        providers([
            Some(PUBLIC_IP),
            Some(OTHER_IP),
            Some(PUBLIC_IP),
            None,
            None,
            None,
        ])
    }

    fn cloudflare_only() -> RouteFetch {
        providers([Some(PUBLIC_IP), None, None, Some(PUBLIC_IP), None, None])
    }

    /// Every outcome a document format has to carry: a headline, a verified answer, a
    /// disagreement (exit 3), answers from one operator only, a total failure (exit 1) and
    /// each dimension command.
    fn documents() -> [(&'static [&'static str], u8, Fixture); 12] {
        [
            (&["public"], 0, agree),
            (&["public", "--verify"], 0, agree),
            (&["public", "--verify"], 3, split),
            (&["public", "--verify"], 1, cloudflare_only),
            (&["public"], 1, nobody_answers),
            (&["local"], 0, agree),
            (&["where"], 0, agree),
            (&["all"], 0, agree),
            (&["all", "--verify"], 0, agree),
            (&["all", "--verify"], 3, split),
            (&["all", "--verify"], 0, cloudflare_only),
            (&["all"], 0, nobody_answers),
        ]
    }

    fn laptop_run(fetch: &RouteFetch, args: &[&str]) -> (u8, String, String) {
        run_on(&Machine::laptop().host(), fetch, args)
    }

    /// A measured latency, in any of its spellings, becomes `N`: `"latency_ms":123` and
    /// `"latency_ms": 123` in a JSON document, `latency_ms: 123` in YAML and ` (123 ms)` in an
    /// attempt list. It is the only thing that differs between two runs of one command.
    fn steady(text: &str) -> String {
        [
            ("\"latency_ms\":", ""),
            ("\"latency_ms\": ", ""),
            ("latency_ms: ", ""),
            (" (", " ms)"),
        ]
        .iter()
        .fold(text.to_owned(), |text, (before, after)| {
            replace_number(&text, before, after)
        })
    }

    /// A run's output with every measured latency hidden, so two runs can be compared.
    fn steady_run((code, out, err): (u8, String, String)) -> (u8, String, String) {
        (code, steady(&out), steady(&err))
    }

    /// Replaces with `N` each run of digits that follows `before` and is followed by `after`.
    fn replace_number(text: &str, before: &str, after: &str) -> String {
        let (mut out, mut rest) = (String::new(), text);
        while let Some(at) = rest.find(before) {
            let (head, tail) = rest.split_at(at + before.len());
            out.push_str(head);
            let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if digits > 0 && tail[digits..].starts_with(after) {
                out.push('N');
                rest = &tail[digits..];
            } else {
                rest = tail;
            }
        }
        out.push_str(rest);
        out
    }

    /// Delays its first request only, so that two runs of one command measure different
    /// latencies, as two runs on a loaded machine do.
    struct StallsOnce<'a> {
        inner: &'a dyn Fetch,
        stall: Duration,
        stalled: std::sync::atomic::AtomicBool,
    }

    impl Fetch for StallsOnce<'_> {
        fn get(&self, url: &str, opts: &FetchOptions) -> Result<Response, FetchError> {
            if !self.stalled.swap(true, std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(self.stall);
            }
            self.inner.get(url, opts)
        }
    }

    #[test]
    fn two_runs_that_measure_different_latencies_compare_equal() {
        for (fixture, args) in [
            (nobody_answers as Fixture, &["public"][..]),
            (agree, &["public", "--format", "json"]),
            (agree, &["public", "--format", "json-pretty"]),
            (agree, &["public", "--format", "yaml"]),
        ] {
            let started = std::time::Instant::now();
            let quick = laptop_run(&fixture(), args);
            let inner = fixture();
            let slow = StallsOnce {
                inner: &inner,
                // Longer than the whole quick run, so its first latency is the larger one.
                stall: started.elapsed() + Duration::from_millis(10),
                stalled: std::sync::atomic::AtomicBool::new(false),
            };
            let stalled = run_on(&Machine::laptop().host(), &slow, args);
            assert_ne!(stalled, quick, "the stall did not show in {args:?}");
            assert_eq!(steady_run(stalled), steady_run(quick), "{args:?}");
        }
    }

    #[test]
    fn steady_hides_a_measured_latency_and_nothing_else() {
        for (text, want) in [
            (" (12 ms)", " (N ms)"),
            (
                "name: down (0 ms)\nname2: HTTP 500 (1234 ms)\n",
                "name: down (N ms)\nname2: HTTP 500 (N ms)\n",
            ),
            (r#"{"latency_ms":12,"x":1}"#, r#"{"latency_ms":N,"x":1}"#),
            (
                r#"{"latency_ms":3,"a":" (4 ms)"}"#,
                r#"{"latency_ms":N,"a":" (N ms)"}"#,
            ),
            (" (12 s)", " (12 s)"),
            (" (x ms)", " (x ms)"),
            (" (-3 ms)", " (-3 ms)"),
            (" (1 2 ms)", " (1 2 ms)"),
            (" (12 ms", " (12 ms"),
            ("(12 ms)", "(12 ms)"),
            ("12 ms", "12 ms"),
            (r#"{"latency_ms":null}"#, r#"{"latency_ms":null}"#),
            (
                "{\n  \"latency_ms\": 12,\n  \"x\": 1\n}",
                "{\n  \"latency_ms\": N,\n  \"x\": 1\n}",
            ),
            ("latency_ms: 12\nx: 1\n", "latency_ms: N\nx: 1\n"),
            ("  latency_ms: 0\n", "  latency_ms: N\n"),
            ("latency_ms: null\n", "latency_ms: null\n"),
            ("latency_ms 12\n", "latency_ms 12\n"),
            ("took (12 ms) of 7 s (7 s)", "took (N ms) of 7 s (7 s)"),
        ] {
            assert_eq!(steady(text), want, "{text}");
        }
    }

    /// What a real YAML parser reads from `text` (JSON is YAML), with every measured latency
    /// zeroed so two runs compare.
    fn read(text: &str) -> yaml_rust2::Yaml {
        use yaml_rust2::{Yaml, YamlLoader};

        fn zeroed(yaml: Yaml) -> Yaml {
            match yaml {
                Yaml::Array(items) => Yaml::Array(items.into_iter().map(zeroed).collect()),
                Yaml::Hash(map) => Yaml::Hash(
                    map.into_iter()
                        .map(|(key, value)| match (&key, value) {
                            (Yaml::String(name), Yaml::Integer(_)) if name == "latency_ms" => {
                                (key, Yaml::Integer(0))
                            }
                            (_, value) => {
                                let value = zeroed(value);
                                (key, value)
                            }
                        })
                        .collect(),
                ),
                other => other,
            }
        }

        let mut documents =
            YamlLoader::load_from_str(text).unwrap_or_else(|error| panic!("{error}\n---\n{text}"));
        assert_eq!(documents.len(), 1, "{text}");
        zeroed(documents.remove(0))
    }

    #[test]
    fn format_json_is_the_json_flag_byte_for_byte() {
        for (args, code, fixture) in documents() {
            let (want_code, want, want_err) =
                laptop_run(&fixture(), &[&["--json"][..], args].concat());
            assert_eq!(want_code, code, "{args:?}");
            for spelled in [
                [&["--format", "json"][..], args].concat(),
                [&["--format=json"][..], args].concat(),
                [args, &["--format", "json"]].concat(),
            ] {
                let (got_code, got, got_err) = laptop_run(&fixture(), &spelled);
                assert_eq!(got_code, want_code, "{spelled:?}");
                assert_eq!(steady(&got), steady(&want), "{spelled:?}");
                assert_eq!(steady(&got_err), steady(&want_err), "{spelled:?}");
            }
        }
    }

    #[test]
    fn every_document_format_carries_the_same_document() {
        for (args, code, fixture) in documents() {
            let run =
                |format: &str| laptop_run(&fixture(), &[&["--format", format][..], args].concat());
            let (json_code, json, json_err) = run("json");
            let (pretty_code, pretty, pretty_err) = run("json-pretty");
            let (yaml_code, yaml, yaml_err) = run("yaml");
            for (got, err) in [(pretty_code, &pretty_err), (yaml_code, &yaml_err)] {
                assert_eq!((got, steady(err)), (code, steady(&json_err)), "{args:?}");
            }
            assert_eq!(json_code, code, "{args:?}");

            let tree = read(&json);
            assert_eq!(read(&pretty), tree, "{args:?}\n{pretty}");
            assert_eq!(read(&yaml), tree, "{args:?}\n{yaml}");
            assert_eq!(tree["schema_version"].as_i64(), Some(1), "{args:?}");

            assert!(yaml.starts_with("schema_version: 1\n"), "{yaml}");
            assert!(
                pretty.starts_with("{\n  \"schema_version\": 1,\n"),
                "{pretty}"
            );
            for (text, name) in [(&json, "json"), (&pretty, "json-pretty"), (&yaml, "yaml")] {
                assert!(
                    text.ends_with('\n') && !text.ends_with("\n\n"),
                    "{name}: {text:?}"
                );
            }
            assert_eq!(json.matches('\n').count(), 1, "{json}");
        }
    }

    #[test]
    fn every_document_format_names_the_operators() {
        let slugs = [
            "cloudflare",
            "ipify",
            "amazon",
            "cloudflare",
            "ident.me",
            "ipinfo",
        ];
        for format in ["json", "json-pretty", "yaml"] {
            let (code, out, _) = laptop_run(&agree(), &["--format", format, "public", "--verify"]);
            assert_eq!(code, 0, "{format}");
            let tree = read(&out);
            let family = &tree["families"][0];
            assert_eq!(family["status"].as_str(), Some("verified"), "{format}");
            assert_eq!(family["total"].as_i64(), Some(6), "{format}");
            assert_eq!(family["operators"].as_i64(), Some(5), "{format}");
            let named: Vec<_> = family["providers"]
                .as_vec()
                .unwrap()
                .iter()
                .map(|provider| provider["operator"].as_str().unwrap())
                .collect();
            assert_eq!(named, slugs, "{format}");

            let (code, out, _) =
                laptop_run(&cloudflare_only(), &["--format", format, "all", "--verify"]);
            assert_eq!(code, 0, "{format}");
            let tree = read(&out);
            let family = &tree["public"]["families"][0];
            assert_eq!(family["status"].as_str(), Some("unverified"), "{format}");
            assert_eq!(family["agree"].as_i64(), Some(2), "{format}");
            assert_eq!(family["total"].as_i64(), Some(2), "{format}");
            assert_eq!(family["operators"].as_i64(), Some(1), "{format}");
            assert_eq!(
                family["providers"][1]["operator"].as_str(),
                Some("cloudflare"),
                "{format}"
            );
        }
        let (_, yaml, _) = laptop_run(
            &cloudflare_only(),
            &["--format", "yaml", "public", "--verify"],
        );
        assert!(yaml.contains("operators: 1\n"), "{yaml}");
        assert!(yaml.contains("operator: \"cloudflare\"\n"), "{yaml}");
    }

    #[test]
    fn a_disagreement_and_a_total_failure_are_still_documents_in_every_format() {
        for format in ["json", "json-pretty", "yaml"] {
            let (code, out, err) =
                laptop_run(&split(), &["--format", format, "public", "--verify"]);
            assert_eq!(code, 3, "{format}");
            assert_eq!(err, "myip: providers disagree: ipv4 (2 of 3 agree)\n");
            let tree = read(&out);
            assert_eq!(tree["ip"], yaml_rust2::Yaml::Null, "{format}");
            assert_eq!(
                tree["families"][0]["status"].as_str(),
                Some("disagree"),
                "{format}"
            );
            assert_eq!(tree["families"][0]["agree"].as_i64(), Some(2), "{format}");

            let (code, out, err) = laptop_run(&nobody_answers(), &["public", "--format", format]);
            assert_eq!(code, 1, "{format}");
            assert_eq!(err, "myip: no provider returned a valid public address\n");
            let tree = read(&out);
            assert_eq!(tree["ip"], yaml_rust2::Yaml::Null, "{format}");
            assert_eq!(
                tree["attempts"].as_vec().map(Vec::len),
                Some(PROVIDERS.len())
            );
        }
    }

    #[test]
    fn without_a_document_the_attempt_list_stays_on_stderr_and_with_one_it_does_not() {
        let (code, out, plain) = laptop_run(&nobody_answers(), &["public"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(plain.lines().count() > 1, "{plain}");
        let (_, _, text) = laptop_run(&nobody_answers(), &["public", "--format", "text"]);
        assert_eq!(steady(&text), steady(&plain));
        for format in ["json", "json-pretty", "yaml"] {
            let (_, _, err) = laptop_run(&nobody_answers(), &["public", "--format", format]);
            assert_eq!(
                err, "myip: no provider returned a valid public address\n",
                "{format}"
            );
        }
    }

    #[test]
    fn format_text_is_the_default() {
        for (args, code, fixture) in documents() {
            let (want_code, want, want_err) = laptop_run(&fixture(), args);
            assert_eq!(want_code, code, "{args:?}");
            for spelled in [
                [&["--format", "text"][..], args].concat(),
                [&["--format=text"][..], args].concat(),
                [args, &["--format", "text"]].concat(),
            ] {
                let (got_code, got, got_err) = laptop_run(&fixture(), &spelled);
                assert_eq!(
                    steady_run((got_code, got, got_err)),
                    steady_run((want_code, want.clone(), want_err.clone())),
                    "{spelled:?}"
                );
            }
        }
    }

    #[test]
    fn format_and_json_never_combine_even_when_they_agree() {
        for combined in [
            &["--json", "--format", "json", "where"][..],
            &["--format", "json", "--json", "where"],
            &["--json", "--format", "yaml", "where"],
            &["--format", "text", "--json", "where"],
            &["where", "--json", "--format=json-pretty"],
            &["--json", "--format", "json"],
        ] {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, combined);
            assert_eq!(code, 2, "{combined:?}");
            assert_eq!(out, "", "{combined:?}");
            assert!(err.contains("--json") && err.contains("--format"), "{err}");
            assert!(requested(&fetch).is_empty(), "{combined:?}");
        }
    }

    #[test]
    fn format_is_refused_for_the_ipinfo_passthrough_before_any_request() {
        for args in [
            &["--format", "json", "city"][..],
            &["--format", "text", "city"],
            &["--format", "yaml", "public", "city"],
            &["public", "--format", "json-pretty", "ip"],
            &["--format", "json"],
            &["--format=yaml"],
            &["--format", "text"],
            &["--format", "json", "8.8.8.8/org"],
        ] {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "", "{args:?}");
            assert!(
                err.starts_with("error: ")
                    && err.contains("--format")
                    && err.ends_with("For more information, try '--help'.\n"),
                "{args:?}: {err}"
            );
            assert!(
                requested(&fetch).is_empty(),
                "{args:?} must not query ipinfo.io"
            );
        }
    }

    #[test]
    fn the_passthrough_still_tolerates_json() {
        let fetch = FakeFetch::new(vec![ok(200, "Seattle\n")]);
        let (code, out, err) = exec(&fetch, &["--json", "city"]);
        assert_eq!((code, out.as_str(), err.as_str()), (0, "Seattle\n", ""));
    }

    #[test]
    fn an_unknown_missing_or_repeated_format_is_a_usage_error() {
        for args in [
            &["--format", "toml", "where"][..],
            &["--format=TOML", "where"],
            &["--format=Yaml", "where"],
            &["--format", "", "where"],
            &["--format", "where"],
            &["where", "--format"],
            &["--format", "json", "--format", "yaml", "where"],
            &["--format", "json", "--format", "json", "where"],
            &["where", "--format", "json", "--format", "json"],
        ] {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "", "{args:?}");
            assert!(err.contains("--format"), "{args:?}: {err}");
            assert!(requested(&fetch).is_empty(), "{args:?}");
        }
        let (_, _, err) = exec(&no_network(), &["--format", "toml", "where"]);
        for value in ["text", "json", "json-pretty", "yaml"] {
            assert!(err.contains(value), "{value} missing in {err}");
        }
    }

    #[test]
    fn a_format_after_the_command_overrides_one_before_it_like_every_global_option() {
        let compact = exec(
            &no_network(),
            &["--format", "yaml", "where", "--format", "json"],
        );
        let yaml = exec(
            &no_network(),
            &["--format", "json", "where", "--format", "yaml"],
        );
        assert_eq!(compact.0, 0);
        assert!(
            compact.1.starts_with("{\"schema_version\":1,"),
            "{}",
            compact.1
        );
        assert_eq!(yaml.0, 0);
        assert!(yaml.1.starts_with("schema_version: 1\n"), "{}", yaml.1);
    }

    #[test]
    fn help_lists_the_format_flag_and_its_values() {
        for args in [&["--help"][..], &["-h"], &["where", "--help"]] {
            let (code, out, _) = exec(&no_network(), args);
            assert_eq!(code, 0, "{args:?}");
            assert!(out.contains("--format <FORMAT>"), "{args:?}\n{out}");
            for value in ["text", "json-pretty", "yaml"] {
                assert!(out.contains(value), "{value} missing in\n{out}");
            }
        }
    }

    #[test]
    fn hostile_text_reads_back_the_same_in_every_document_format() {
        let commands: [&[&str]; 4] = [&["local"], &["where"], &["all"], &["all", "--verify"]];
        let mut reached = std::collections::BTreeSet::new();
        for source in SOURCES {
            for bad in HOSTILE {
                let payload = format!("a{bad}INJECT");
                for command in commands {
                    let run = |format: &str| {
                        let machine = machine_with(source, &payload);
                        let args = [&["--format", format][..], command].concat();
                        run_on(&machine.host(), &everyone_says(PUBLIC_IP), &args).1
                    };
                    let (json, pretty, yaml) = (run("json"), run("json-pretty"), run("yaml"));
                    let tree = read(&json);
                    assert_eq!(read(&pretty), tree, "{source} / {bad:?} / {command:?}");
                    assert_eq!(read(&yaml), tree, "{source} / {bad:?} / {command:?}");
                    if yaml.contains("INJECT") {
                        reached.insert(bad);
                    }
                }
            }
        }
        for bad in HOSTILE {
            assert!(
                reached.contains(bad),
                "no document carried {bad:?}, so nothing was proved"
            );
        }
    }

    // ---- --color (ADR 0002) ----

    const BOLD: &str = "\u{1b}[1m";
    const NAME: &str = "\u{1b}[1;36m";
    const DIM: &str = "\u{1b}[2m";
    const GREEN: &str = "\u{1b}[32m";
    const YELLOW: &str = "\u{1b}[33m";
    const RED: &str = "\u{1b}[31m";
    const RESET: &str = "\u{1b}[0m";

    /// One coloured span, spelled out here so the expectations do not use the code under test.
    fn span(code: &str, text: &str) -> String {
        format!("{code}{text}{RESET}")
    }

    /// A terminal that supports colour, with nothing in the environment.
    fn tty() -> Terminal {
        Terminal::from_lookup(true, false, |_| None)
    }

    fn terminal_with(is_tty: bool, vars: &'static [(&'static str, &'static str)]) -> Terminal {
        Terminal::from_lookup(is_tty, false, |name| {
            vars.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    fn coloured(machine: &Machine, fetch: &dyn Fetch, args: &[&str]) -> (u8, String, String) {
        run_on(
            &machine.host(),
            fetch,
            &[&["--color", "always"][..], args].concat(),
        )
    }

    type Build = fn() -> Machine;

    /// Every machine against every text outcome: a headline, an agreeing and a disagreeing
    /// `--verify`, a total failure, and each dimension command.
    fn text_cases() -> Vec<(Build, &'static [&'static str], Fixture)> {
        let outcomes: [(&[&str], Fixture); 10] = [
            (&["local"], agree),
            (&["where"], agree),
            (&["all"], agree),
            (&["all"], nobody_answers),
            (&["all", "--verify"], agree),
            (&["all", "--verify"], split),
            (&["public"], agree),
            (&["public"], nobody_answers),
            (&["public", "--verify"], agree),
            (&["public", "--verify"], split),
        ];
        let machines: [Build; 4] = [
            Machine::quiet,
            Machine::laptop,
            Machine::pod,
            Machine::without_interface_access,
        ];
        let mut cases = Vec::new();
        for machine in machines {
            for (args, fixture) in outcomes {
                cases.push((machine, args, fixture));
            }
        }
        cases
    }

    fn flagged<'a>(flags: &[&'a str], args: &[&'a str]) -> Vec<&'a str> {
        [flags, args].concat()
    }

    #[test]
    fn without_colour_the_text_is_unchanged_whatever_the_terminal_says() {
        for (machine, args, fixture) in text_cases() {
            let want = steady_run(run_on(&machine().host(), &fixture(), args));
            assert!(!want.1.contains('\u{1b}'), "{args:?}: {:?}", want.1);
            assert!(!want.2.contains('\u{1b}'), "{args:?}: {:?}", want.2);
            let forms = [
                (Terminal::default(), flagged(&["--color", "auto"], args)),
                (tty(), flagged(&["--color", "never"], args)),
                (tty(), flagged(&["--color=never"], args)),
                (terminal_with(true, &[("NO_COLOR", "1")]), args.to_vec()),
                (terminal_with(true, &[("CLICOLOR", "0")]), args.to_vec()),
                (terminal_with(true, &[("TERM", "dumb")]), args.to_vec()),
                (
                    terminal_with(false, &[("CLICOLOR_FORCE", "1")]),
                    flagged(&["--color", "never"], args),
                ),
            ];
            for (terminal, spelled) in forms {
                let got = steady_run(run_in(
                    &machine().host(),
                    &fixture(),
                    terminal.clone(),
                    &spelled,
                ));
                assert_eq!(got, want, "{spelled:?} on {terminal:?}");
            }
        }
    }

    #[test]
    fn colour_is_the_plain_text_with_codes_added_and_nothing_else() {
        let mut coloured_runs = 0;
        for (machine, args, fixture) in text_cases() {
            let (plain_code, plain, plain_err) = run_on(
                &machine().host(),
                &fixture(),
                &flagged(&["--color", "never"], args),
            );
            let (code, out, err) = run_on(
                &machine().host(),
                &fixture(),
                &flagged(&["--color", "always"], args),
            );
            let label = format!("{args:?} (exit {plain_code})");
            assert_eq!(
                (code, steady(&err)),
                (plain_code, steady(&plain_err)),
                "{label}"
            );
            assert_eq!(steady(&strip(&out)), steady(&plain), "{label}");
            assert!(
                !strip(&out).contains('\u{1b}'),
                "{label}: a code that is not ours in {out:?}"
            );
            assert_balanced(&out);
            let has_styled_text = matches!(args[0], "local" | "where" | "all")
                || (args == ["public", "--verify"] && plain_code == 3);
            assert_eq!(
                out.contains('\u{1b}'),
                has_styled_text && !plain.is_empty(),
                "{label}: {out:?}"
            );
            coloured_runs += usize::from(out.contains('\u{1b}'));
        }
        assert!(
            coloured_runs > 20,
            "only {coloured_runs} runs were coloured"
        );
    }

    #[test]
    fn auto_follows_the_terminal_and_the_environment_and_the_flag_wins() {
        let cases: [(Terminal, &[&str], bool); 14] = [
            (Terminal::default(), &["where"], false),
            (tty(), &["where"], true),
            (tty(), &["--color", "auto", "where"], true),
            (terminal_with(true, &[("NO_COLOR", "1")]), &["where"], false),
            (
                terminal_with(true, &[("NO_COLOR", "1")]),
                &["--color", "always", "where"],
                true,
            ),
            (
                terminal_with(false, &[("CLICOLOR_FORCE", "1")]),
                &["where"],
                true,
            ),
            (
                terminal_with(false, &[("CLICOLOR_FORCE", "1")]),
                &["--color", "never", "where"],
                false,
            ),
            (
                terminal_with(true, &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")]),
                &["where"],
                false,
            ),
            (terminal_with(true, &[("TERM", "dumb")]), &["where"], false),
            (tty(), &["--format", "json", "where"], false),
            (tty(), &["--json", "where"], false),
            (tty(), &["--format", "yaml", "where"], false),
            (
                tty(),
                &["--color", "always", "--format", "json-pretty", "where"],
                false,
            ),
            (tty(), &["--color", "always", "--json", "where"], false),
        ];
        for (terminal, args, colour) in cases {
            let (code, out, err) = run_in(
                &Machine::pod().host(),
                &no_network(),
                terminal.clone(),
                args,
            );
            assert_eq!(code, 0, "{args:?}: {err}");
            assert_eq!(
                out.contains('\u{1b}'),
                colour,
                "{args:?} on {terminal:?}: {out:?}"
            );
            assert!(!err.contains('\u{1b}'), "{args:?}: {err:?}");
        }
    }

    #[test]
    fn local_styles_names_provenance_guesses_and_failures() {
        let (code, out, err) = coloured(&Machine::laptop(), &no_network(), &["local"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_balanced(&out);
        let heading = format!("{}{}\n", span(BOLD, "interfaces"), span(DIM, " (observed)"));
        assert!(out.starts_with(&heading), "{out:?}");
        assert!(
            out.contains(&format!(
                "\n  {} {}\n",
                span(NAME, "lo"),
                span(YELLOW, "(likely virtual)")
            )),
            "{out:?}"
        );
        assert!(
            out.contains(&format!("\n  {}\n", span(NAME, "en0"))),
            "{out:?}"
        );
        assert!(
            out.contains(&format!(
                "\n  {} {}\n",
                span(NAME, "utun3"),
                span(YELLOW, "(likely virtual)")
            )),
            "{out:?}"
        );
        assert!(
            out.contains(&format!(
                "\n{}{DIM} (observed: a UDP connect() towards ",
                span(BOLD, "egress")
            )),
            "{out:?}"
        );
        assert!(
            out.contains(&format!("which sends nothing){RESET}\n")),
            "{out:?}"
        );
        let guess = "\"likely virtual\" is a guess from the interface name, not a fact.";
        assert!(out.contains(&format!("{}\n", span(DIM, guess))), "{out:?}");
        assert!(out.contains("  primary egress (IPv4)\n"), "{out:?}");
        assert!(!out.contains(GREEN) && !out.contains(RED), "{out:?}");

        let (_, out, _) = coloured(&Machine::quiet(), &no_network(), &["local"]);
        assert!(out.contains("\n  no interfaces found\n"), "{out:?}");
        let unknown = format!(
            "  IPv4  {}{}\n",
            span(YELLOW, "unknown"),
            span(DIM, " (the egress probe found no address; -v shows why)")
        );
        assert!(out.contains(&unknown), "{out:?}");

        let (code, out, _) = coloured(
            &Machine::without_interface_access(),
            &everyone_says(PUBLIC_IP),
            &["all"],
        );
        assert_eq!(code, 0);
        let failed = format!("  {}: ", span(RED, "unavailable"));
        assert_eq!(out.matches(&failed).count(), 1, "{out:?}");
    }

    #[test]
    fn where_styles_field_names_provenance_and_what_is_unknown() {
        let (code, out, err) = coloured(&Machine::laptop(), &no_network(), &["where"]);
        assert_eq!((code, err.as_str()), (0, ""));
        let declared = "\"declared\" values were handed to this process (environment, files an orchestrator wrote); \"observed\" ones were measured here.";
        assert_eq!(
            out,
            format!(
                "{}    host (no container or Kubernetes signals found)\n{}\n",
                span(NAME, "environment"),
                span(DIM, declared)
            )
        );

        let (code, out, _) = coloured(&Machine::pod(), &no_network(), &["where"]);
        assert_eq!(code, 0);
        assert_balanced(&out);
        let lines = [
            format!("{}    kubernetes (a pod)\n", span(NAME, "environment")),
            format!(
                "{}       web-0{}\n",
                span(NAME, "pod name"),
                span(DIM, "  (declared: env:POD_NAME)")
            ),
            format!(
                "{}         10.244.1.7{}\n",
                span(NAME, "pod ip"),
                span(DIM, "  (declared: env:POD_IP)")
            ),
            format!(
                "{}        {}{}\n",
                span(NAME, "pod ips"),
                span(YELLOW, "unknown"),
                span(DIM, " (expose POD_IPS with the Downward API)")
            ),
            format!(
                "{}   {}\n",
                span(NAME, "service port"),
                span(YELLOW, "unknown")
            ),
            format!("{}\n", span(DIM, declared)),
        ];
        for line in lines {
            assert!(out.contains(&line), "{line:?} missing in {out:?}");
        }
        let evidence = out
            .split_once(&format!("{}\n", span(BOLD, "evidence")))
            .unwrap_or_else(|| panic!("no evidence block in {out:?}"))
            .1;
        let listed: Vec<&str> = evidence
            .lines()
            .take_while(|l| l.starts_with("  "))
            .collect();
        assert!(!listed.is_empty(), "{out:?}");
        for line in listed {
            assert!(
                line.starts_with(&format!("  {DIM}- ")) && line.ends_with(RESET),
                "{line:?}"
            );
        }
    }

    /// The text of every span that `code` opens, in order.
    fn spans_of<'a>(out: &'a str, code: &str) -> Vec<&'a str> {
        out.split(code)
            .skip(1)
            .map(|rest| rest.split_once(RESET).map_or(rest, |(text, _)| text))
            .collect()
    }

    #[test]
    fn only_interface_and_field_names_are_bold_cyan_and_block_names_stay_bold() {
        let pod_fields = [
            "environment",
            "namespace",
            "pod name",
            "pod ip",
            "pod ips",
            "node name",
            "host ip",
            "host network",
            "service host",
            "service port",
            "cluster dns",
            "cluster domain",
        ];
        let pod_all_names: Vec<&str> = ["lo", "en0", "utun3"]
            .into_iter()
            .chain(pod_fields)
            .collect();
        /// (machine, arguments, network, the `name` spans, the `bold` spans)
        type Case = (
            Build,
            &'static [&'static str],
            Fixture,
            Vec<&'static str>,
            Vec<&'static str>,
        );
        let cases: [Case; 4] = [
            (
                Machine::laptop,
                &["local"],
                agree,
                vec!["lo", "en0", "utun3"],
                vec!["interfaces", "egress"],
            ),
            (
                Machine::laptop,
                &["where"],
                agree,
                vec!["environment"],
                vec![],
            ),
            (
                Machine::pod,
                &["all"],
                agree,
                pod_all_names,
                vec![
                    "[public]",
                    "[local]",
                    "interfaces",
                    "egress",
                    "[where]",
                    "evidence",
                    "[highlights]",
                    "kubernetes",
                ],
            ),
            (
                Machine::quiet,
                &["public", "--verify"],
                split,
                vec![],
                vec!["ipv4"],
            ),
        ];
        for (machine, args, fixture, names, bold) in cases {
            let (_, out, _) = coloured(&machine(), &fixture(), args);
            assert_eq!(spans_of(&out, NAME), names, "{args:?}: {out:?}");
            assert_eq!(spans_of(&out, BOLD), bold, "{args:?}: {out:?}");
        }
    }

    #[test]
    fn a_hostile_name_is_escaped_before_it_is_styled() {
        let hostile = format!("{NAME}\u{1b}[31m{RESET}");
        let machine = machine_with("interface name", &hostile);
        let (code, out, err) = coloured(&machine, &no_network(), &["local"]);
        assert_eq!((code, err.as_str()), (0, ""));
        let shown = "en\\x1b[1;36m\\x1b[31m\\x1b[0m";
        assert!(
            out.contains(&format!("\n  {}", span(NAME, shown))),
            "{out:?}"
        );
        assert_balanced(&out);
        assert!(!strip(&out).contains('\u{1b}'), "{out:?}");
    }

    #[test]
    fn all_styles_its_blocks_the_headline_the_nat_verdict_and_the_pod_summary() {
        let (code, out, err) = coloured(&Machine::pod(), &everyone_says(PUBLIC_IP), &["all"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_balanced(&out);
        for heading in ["[public]", "[local]", "[where]", "[highlights]"] {
            assert!(
                out.contains(&format!("{}\n", span(BOLD, heading))),
                "{out:?}"
            );
        }
        let lines = [
            format!(
                "  1.2.3.4 {}\n",
                span(DIM, "(IPv4, observed by one.one.one.one)")
            ),
            format!(
                "  {}{}\n",
                span(
                    YELLOW,
                    "NAT: this machine sends from 192.168.1.23 (private) but the internet sees 1.2.3.4 (IPv4);"
                ),
                span(
                    DIM,
                    " something in between (NAT, a proxy or a VPN) rewrites the address"
                )
            ),
            format!(
                "  {}  pod 10.244.1.7, node 192.168.49.2, egress 192.168.1.23, public 1.2.3.4\n  {}\n",
                span(BOLD, "kubernetes"),
                span(
                    DIM,
                    "(pod and node addresses are declared by the Pod spec; egress and public are observed)"
                )
            ),
        ];
        for line in lines {
            assert!(out.contains(&line), "{line:?} missing in {out:?}");
        }

        let (_, out, _) = coloured(
            &Machine::laptop(),
            &everyone_says(PUBLIC_IP),
            &["all", "--verify"],
        );
        let verified = format!(
            "  1.2.3.4 {DIM}(IPv4, observed by one.one.one.one{RESET}{}{DIM}){RESET}\n",
            span(GREEN, ", verified: 6 of 6 providers agree")
        );
        assert!(out.contains(&verified), "{out:?}");

        let mut direct = Machine::laptop();
        direct.probe = FakeProbe::new().route(PROBE_V4, PUBLIC_IP);
        let (_, out, _) = coloured(&direct, &everyone_says(PUBLIC_IP), &["all"]);
        let none = format!(
            "  {}\n",
            span(
                GREEN,
                "no NAT: the egress address 1.2.3.4 is the address the internet sees (IPv4)"
            )
        );
        assert!(out.contains(&none), "{out:?}");
        let highlights = out
            .split_once(&format!("{}\n", span(BOLD, "[highlights]")))
            .unwrap_or_else(|| panic!("no highlights in {out:?}"))
            .1;
        assert_eq!(highlights, none, "no NAT leaves nothing else to say");

        let (_, out, _) = coloured(&Machine::pod(), &nobody_answers(), &["all"]);
        assert_balanced(&out);
        let lines = [
            format!("  {}: ", span(RED, "unavailable")),
            format!(
                "  {}{}\n",
                span(YELLOW, "NAT: unknown"),
                span(
                    DIM,
                    " (needs a public answer and a local egress address of the same IP family)"
                )
            ),
            format!("public {}\n", span(YELLOW, "unknown")),
        ];
        for line in lines {
            assert!(out.contains(&line), "{line:?} missing in {out:?}");
        }
    }

    #[test]
    fn a_disagreement_is_styled_in_the_report_and_inside_all() {
        let (code, out, _) = coloured(&Machine::quiet(), &split(), &["public", "--verify"]);
        assert_eq!(code, 3);
        assert_balanced(&out);
        let report = [
            format!(
                "{}: {}\n",
                span(BOLD, "ipv4"),
                span(RED, "DISAGREE, 2 of 3 agree")
            ),
            format!("5.6.7.8  {}\n", span(RED, "<- differs")),
            format!("{}\n", span(RED, "no answer:")),
        ];
        assert!(out.starts_with(&report[0]), "{out:?}");
        for line in &report[1..] {
            assert!(out.contains(line.as_str()), "{line:?} missing in {out:?}");
        }

        let (code, out, _) = coloured(&Machine::laptop(), &split(), &["all", "--verify"]);
        assert_eq!(code, 3);
        assert_balanced(&out);
        for line in &report {
            assert!(
                out.contains(&format!("  {line}")),
                "{line:?} missing, indented, in {out:?}"
            );
        }
    }

    #[test]
    fn colour_never_reaches_the_headline_the_passthrough_a_failure_or_a_document() {
        for (args, fixture) in [
            (&["public"][..], agree as Fixture),
            (&["public", "--verify"], agree),
            (&["public"], nobody_answers),
            (&["public", "--verify"], nobody_answers),
        ] {
            let (code, out, err) = coloured(&Machine::laptop(), &fixture(), args);
            let (want_code, want_out, want_err) = laptop_run(&fixture(), args);
            assert_eq!(
                steady_run((code, out.clone(), err.clone())),
                steady_run((want_code, want_out, want_err)),
                "{args:?}"
            );
            assert!(
                !out.contains('\u{1b}') && !err.contains('\u{1b}'),
                "{args:?}"
            );
        }
        let (_, out, _) = coloured(&Machine::laptop(), &agree(), &["public"]);
        assert_eq!(out, "1.2.3.4\n");

        for spelled in [
            &["--color", "always", "city"][..],
            &["city", "--color", "always"],
            &["--color=always", "public", "city"],
            &["--json", "--color", "always", "city"],
        ] {
            let fetch = FakeFetch::new(vec![ok(200, "Seattle\n")]);
            let (code, out, err) = exec(&fetch, spelled);
            assert_eq!(
                (code, out.as_str(), err.as_str()),
                (0, "Seattle\n", ""),
                "{spelled:?}"
            );
            assert_eq!(requested(&fetch).len(), 1, "{spelled:?}");
        }

        for (args, code, fixture) in documents() {
            for format in ["json", "json-pretty", "yaml"] {
                let plain = laptop_run(&fixture(), &flagged(&["--format", format], args));
                let styled = laptop_run(
                    &fixture(),
                    &flagged(&["--color", "always", "--format", format], args),
                );
                assert_eq!(plain.0, code, "{args:?}");
                assert_eq!(
                    (styled.0, steady(&styled.1), steady(&styled.2)),
                    (plain.0, steady(&plain.1), steady(&plain.2)),
                    "{format} {args:?}"
                );
                assert!(!styled.1.contains('\u{1b}'), "{format} {args:?}");
            }
        }
    }

    #[test]
    fn stderr_the_version_and_usage_errors_are_never_coloured() {
        for args in [
            &["-v", "all"][..],
            &["-v", "local"],
            &["-v", "where"],
            &["-v", "all", "--verify"],
        ] {
            let (code, out, err) = coloured(&Machine::pod(), &split(), args);
            let (want_code, _, want_err) = run_on(
                &Machine::pod().host(),
                &split(),
                &flagged(&["--color", "never"], args),
            );
            assert_eq!(code, want_code, "{args:?}");
            assert_eq!(steady(&err), steady(&want_err), "{args:?}");
            assert!(
                err.contains("myip: "),
                "{args:?} must log something: {err:?}"
            );
            assert!(!err.contains('\u{1b}'), "{args:?}: {err:?}");
            assert!(out.contains('\u{1b}'), "{args:?} must still colour stdout");
        }
        for args in [
            &["--color", "always", "--version"][..],
            &["--color", "always", "--format", "toml"],
        ] {
            let (_, out, err) = exec(&no_network(), args);
            assert!(
                !out.contains('\u{1b}') && !err.contains('\u{1b}'),
                "{args:?}"
            );
        }
    }

    #[test]
    fn colour_options_are_validated_and_the_last_spelling_wins() {
        for args in [
            &["--color", "rainbow", "where"][..],
            &["--color=ALWAYS", "where"],
            &["--color", "", "where"],
            &["--color", "where"],
            &["where", "--color"],
            &["--color", "always", "--color", "never", "where"],
            &["where", "--color", "always", "--color", "never"],
            &["-C", "always", "where"],
        ] {
            let fetch = no_network();
            let (code, out, err) = exec(&fetch, args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "", "{args:?}");
            assert!(requested(&fetch).is_empty(), "{args:?}");
            assert!(err.starts_with("error: "), "{args:?}: {err}");
        }
        let (_, _, err) = exec(&no_network(), &["--color", "rainbow", "where"]);
        for value in ["auto", "always", "never"] {
            assert!(err.contains(value), "{value} missing in {err}");
        }

        let before = exec(
            &no_network(),
            &["--color", "always", "where", "--color", "never"],
        );
        let after = exec(
            &no_network(),
            &["--color", "never", "where", "--color", "always"],
        );
        assert!(!before.1.contains('\u{1b}'), "{:?}", before.1);
        assert!(after.1.contains('\u{1b}'), "{:?}", after.1);
        let (code, out, _) = exec(&no_network(), &["where", "--color", "always"]);
        assert_eq!(code, 0);
        assert!(out.contains('\u{1b}'));
    }

    #[test]
    fn help_lists_the_colour_flag_and_its_values() {
        for args in [
            &["--help"][..],
            &["-h"],
            &["where", "--help"],
            &["all", "--help"],
        ] {
            let (code, out, _) = exec(&no_network(), args);
            assert_eq!(code, 0, "{args:?}");
            assert!(out.contains("--color <WHEN>"), "{args:?}\n{out}");
            for value in ["auto", "always", "never"] {
                assert!(out.contains(value), "{value} missing in\n{out}");
            }
        }
    }

    /// Every help text there is: the long and the short form of the command and of each
    /// subcommand.
    const HELPS: [&[&str]; 10] = [
        &["--help"],
        &["-h"],
        &["public", "--help"],
        &["public", "-h"],
        &["local", "--help"],
        &["local", "-h"],
        &["where", "--help"],
        &["where", "-h"],
        &["all", "--help"],
        &["all", "-h"],
    ];

    /// `text` without any SGR sequence (`ESC [ digits m`), whichever colour it sets, and
    /// nothing else.
    fn without_sgr(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find("\u{1b}[") {
            out.push_str(&rest[..at]);
            let tail = &rest[at + 2..];
            let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            match tail[digits..].strip_prefix('m') {
                Some(after) => rest = after,
                None => {
                    out.push_str("\u{1b}[");
                    rest = tail;
                }
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn help_is_coloured_like_cargo_on_a_terminal_and_strips_to_the_plain_help() {
        for args in HELPS {
            let (code, plain, err) = exec(&no_network(), args);
            assert_eq!((code, err.as_str()), (0, ""), "{args:?}");
            assert!(!plain.contains('\u{1b}'), "{args:?}");

            let (code, styled, err) = run_in(&Machine::quiet().host(), &no_network(), tty(), args);
            assert_eq!((code, err.as_str()), (0, ""), "{args:?}");
            // Bold green headings, bold cyan literals, plain cyan placeholders, nothing else.
            for want in [
                "\u{1b}[1m\u{1b}[32mUsage:\u{1b}[0m",
                "\u{1b}[1m\u{1b}[32mOptions:\u{1b}[0m",
                "\u{1b}[1m\u{1b}[36m-h\u{1b}[0m",
                "\u{1b}[36m<WHEN>\u{1b}[0m",
            ] {
                assert!(styled.contains(want), "{args:?} lacks {want:?}\n{styled}");
            }
            let stripped = without_sgr(&styled);
            assert_eq!(stripped, plain, "{args:?}");
            let codes: std::collections::BTreeSet<_> = styled
                .split('\u{1b}')
                .skip(1)
                .map(|tail| tail.split('m').next().unwrap_or_default())
                .collect();
            assert!(
                codes.iter().all(|c| ["[0", "[1", "[32", "[36"].contains(c)),
                "{args:?}: {codes:?}"
            );
        }
    }

    #[test]
    fn help_colour_follows_the_decision_of_adr_0002_and_the_flag_in_any_position() {
        let cases: [(Terminal, &[&str], bool); 17] = [
            (Terminal::default(), &["--help"], false),
            (tty(), &["--help"], true),
            (tty(), &["-h"], true),
            (tty(), &["where", "--help"], true),
            (tty(), &["--color", "auto", "--help"], true),
            (tty(), &["--color", "never", "--help"], false),
            (tty(), &["--help", "--color", "never"], false),
            (tty(), &["where", "--help", "--color=never"], false),
            (tty(), &["--color=never", "public", "--help"], false),
            (Terminal::default(), &["--color", "always", "--help"], true),
            (Terminal::default(), &["--help", "--color", "always"], true),
            (
                Terminal::default(),
                &["public", "--help", "--color=always"],
                true,
            ),
            (
                terminal_with(true, &[("NO_COLOR", "1")]),
                &["--help"],
                false,
            ),
            (
                terminal_with(true, &[("NO_COLOR", "1")]),
                &["--color", "always", "-h"],
                true,
            ),
            (
                terminal_with(false, &[("CLICOLOR_FORCE", "1")]),
                &["--help"],
                true,
            ),
            (
                terminal_with(true, &[("CLICOLOR", "0")]),
                &["--help"],
                false,
            ),
            (
                terminal_with(true, &[("TERM", "dumb")]),
                &["local", "--help"],
                false,
            ),
        ];
        for (terminal, args, want) in cases {
            let (code, out, err) = run_in(
                &Machine::quiet().host(),
                &no_network(),
                terminal.clone(),
                args,
            );
            assert_eq!((code, err.as_str()), (0, ""), "{terminal:?} {args:?}");
            assert_eq!(out.contains('\u{1b}'), want, "{terminal:?} {args:?}");
        }
    }

    #[test]
    fn the_version_and_usage_errors_stay_plain_even_on_a_terminal_that_asks_for_colour() {
        let (code, out, err) = run_in(
            &Machine::quiet().host(),
            &no_network(),
            tty(),
            &["--version", "--color", "always"],
        );
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(
            out.starts_with("myip ") && !out.contains('\u{1b}'),
            "{out:?}"
        );

        for args in [
            &["--bogus", "--color", "always"][..],
            &["where", "--bogus", "--color=always"],
            &["--color", "rainbow", "--help"],
            &["--color", "always", "--format", "toml"],
        ] {
            let (code, out, err) = run_in(&Machine::quiet().host(), &no_network(), tty(), args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "", "{args:?}");
            assert!(err.starts_with("error: "), "{args:?}: {err:?}");
            assert!(!err.contains('\u{1b}'), "{args:?}: {err:?}");
        }
    }

    #[test]
    fn hostile_text_stays_visible_and_unstyled_in_colour_too() {
        let commands: [&[&str]; 4] = [&["local"], &["where"], &["all"], &["all", "--verify"]];
        let mut coloured_runs = 0;
        for source in SOURCES {
            for bad in HOSTILE {
                let payload = format!("a{bad}INJECT");
                for command in commands {
                    let label = format!("{source} / {bad:?} / {command:?}");
                    let plain = run_on(
                        &machine_with(source, &payload).host(),
                        &everyone_says(PUBLIC_IP),
                        &flagged(&["--color", "never"], command),
                    );
                    let (code, out, err) = run_on(
                        &machine_with(source, &payload).host(),
                        &everyone_says(PUBLIC_IP),
                        &flagged(&["--color", "always"], command),
                    );
                    assert_eq!((code, &err), (plain.0, &plain.2), "{label}");
                    assert_eq!(strip(&out), plain.1, "{label}");
                    assert_balanced(&out);
                    for forbidden in [
                        '\u{1b}', '\r', '\0', '\u{85}', '\u{202e}', '\u{2028}', '\u{2029}',
                        '\u{feff}', '\u{fffe}', '\u{ffff}',
                    ] {
                        assert!(
                            !strip(&out).contains(forbidden),
                            "{label}: {forbidden:?} reached the terminal\n{out:?}"
                        );
                    }
                    coloured_runs += usize::from(out.contains('\u{1b}'));
                }
            }
        }
        assert!(
            coloured_runs > 800,
            "only {coloured_runs} runs were coloured"
        );
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

    // ---- no CA certificates: say what to do, once (#35) ----

    /// The transport error of a Debian container without `ca-certificates`, verbatim from
    /// `docker run --rm -v myip-target:/t debian:stable-slim /t/release/myip public` (glibc
    /// release build, ureq 3 with rustls-platform-verifier).
    const NO_CA: &str = "rustls: unexpected error: No CA certificates were loaded from the system";
    const NO_CA_HINT: &str = "hint: no CA certificates were found; install ca-certificates, \
        or set SSL_CERT_FILE or SSL_CERT_DIR to a bundle";

    fn each_provider(
        answer: impl Fn(usize) -> Result<crate::http::Response, FetchError>,
    ) -> RouteFetch {
        let routes = PROVIDERS
            .iter()
            .enumerate()
            .map(|(at, provider)| (provider.url, answer(at)))
            .collect();
        RouteFetch::new(routes)
    }

    fn no_ca_anywhere() -> RouteFetch {
        providers_failing_with([None; 6], NO_CA)
    }

    #[test]
    fn public_says_once_after_the_attempt_list_what_to_do_without_ca_certificates() {
        let (code, out, err) = laptop_run(&no_ca_anywhere(), &["public"]);
        assert_eq!((code, out.as_str()), (1, ""));
        let lines: Vec<&str> = err.lines().collect();
        assert_eq!(lines.len(), 1 + PROVIDERS.len() + 1, "{err}");
        assert_eq!(
            lines[0],
            "myip: no provider returned a valid public address"
        );
        for (line, provider) in lines[1..].iter().zip(&PROVIDERS) {
            assert!(
                line.starts_with(&format!("  {}: {NO_CA} (", provider.name)),
                "{line}"
            );
        }
        assert_eq!(lines[PROVIDERS.len() + 1], NO_CA_HINT);
        assert_eq!(err.matches("hint:").count(), 1, "{err}");
    }

    #[test]
    fn the_hint_follows_the_verdict_with_verbose_verify_and_a_family() {
        for args in [
            &["-v", "public"][..],
            &["public", "--verify"],
            &["-v", "public", "--verify"],
            &["-6", "public"],
        ] {
            let (code, out, err) = laptop_run(&no_ca_anywhere(), args);
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert_eq!(err.lines().last(), Some(NO_CA_HINT), "{args:?}\n{err}");
            assert_eq!(err.matches("hint:").count(), 1, "{args:?}\n{err}");
        }
    }

    #[test]
    fn the_hint_is_stderr_only_and_never_part_of_a_document() {
        for format in ["json", "json-pretty", "yaml"] {
            for extra in [&[][..], &["--verify"]] {
                let args = [&["public", "--format", format][..], extra].concat();
                let (code, out, err) = laptop_run(&no_ca_anywhere(), &args);
                assert_eq!(code, 1, "{args:?}");
                assert!(!out.contains("hint"), "{args:?}\n{out}");
                assert_eq!(
                    err,
                    format!("myip: no provider returned a valid public address\n{NO_CA_HINT}\n"),
                    "{args:?}"
                );
            }
        }
    }

    #[test]
    fn the_ipinfo_passthrough_says_once_what_to_do_without_ca_certificates() {
        for args in [&[][..], &["ip"], &["public", "city"], &["-v", "org"]] {
            let fetch = FakeFetch::new(vec![Err(FetchError::Transport(NO_CA.to_owned()))]);
            let (code, out, err) = exec(&fetch, args);
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert!(
                err.ends_with(&format!("myip: {NO_CA}\n{NO_CA_HINT}\n")),
                "{args:?}\n{err}"
            );
            assert_eq!(err.matches("hint:").count(), 1, "{args:?}\n{err}");
        }
    }

    #[test]
    fn other_failures_never_get_the_hint() {
        const NO_DNS: &str =
            "io: failed to lookup address information: Temporary failure in name resolution";
        let boards: Vec<(&str, RouteFetch)> = vec![
            (
                "timeout",
                each_provider(|_| Err(FetchError::Timeout("deadline".to_owned()))),
            ),
            (
                "http 500",
                each_provider(|_| reply(500, "text/plain", "boom")),
            ),
            ("dns", providers_failing_with([None; 6], NO_DNS)),
            ("refused", nobody_answers()),
            (
                "ca error and a timeout",
                each_provider(|at| match at {
                    3 => Err(FetchError::Timeout("deadline".to_owned())),
                    _ => Err(FetchError::Transport(NO_CA.to_owned())),
                }),
            ),
            (
                "ca error and an http 500",
                each_provider(|at| match at {
                    0 => reply(500, "text/plain", "boom"),
                    _ => Err(FetchError::Transport(NO_CA.to_owned())),
                }),
            ),
            // The text is only recognised in a transport error, never in what a server says.
            (
                "a server quoting the text",
                each_provider(|_| reply(200, "text/plain", NO_CA)),
            ),
        ];
        for (label, fetch) in boards {
            for args in [&["public"][..], &["-v", "public"], &["public", "--verify"]] {
                let (code, out, err) = laptop_run(&fetch, args);
                assert_eq!((code, out.as_str()), (1, ""), "{label} {args:?}");
                assert!(!err.contains("hint:"), "{label} {args:?}\n{err}");
            }
        }
        let legacy = [
            Err(FetchError::Timeout("deadline".to_owned())),
            Err(FetchError::Transport(NO_DNS.to_owned())),
            ok(500, NO_CA),
            ok(200, ""),
        ];
        for result in legacy {
            let (code, _, err) = exec(&FakeFetch::new(vec![result]), &[]);
            assert_eq!(code, 1);
            assert!(!err.contains("hint:"), "{err}");
        }
    }

    #[test]
    fn an_answer_means_no_hint() {
        let (code, out, err) = laptop_run(&everyone_says(PUBLIC_IP), &["public"]);
        assert_eq!((code, out.as_str(), err.as_str()), (0, "1.2.3.4\n", ""));
        let (code, _, err) = laptop_run(
            &each_provider(|at| match at {
                0 => Err(FetchError::Transport(NO_CA.to_owned())),
                _ => reply(200, "text/plain", "1.2.3.4\n"),
            }),
            &["public"],
        );
        assert_eq!(code, 0);
        assert!(!err.contains("hint:"), "{err}");
    }
}
