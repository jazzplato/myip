//! `myip public`: what does the internet see? Asks the providers of ADR 0001 in order and
//! prints the first valid answer, or with `--verify` asks all of them at once and compares.

mod provider;
mod render;
mod verdict;

use std::net::IpAddr;
use std::thread;
use std::time::{Duration, Instant};

pub use provider::{PROVIDERS, Provider, Reject, read_answer};
pub use verdict::{Answer, Attempt, FamilyReport, IpFamily, Status, Verdict};

use crate::Context;
use crate::cli::Options;
use crate::http::{Family, Fetch, FetchError, FetchOptions};
use crate::legacy::detail;
use provider::MAX_ANSWER_BYTES;

const NO_ANSWER: &str = "no provider returned a valid public address";
const NOT_VERIFIED: &str = "the address could not be verified: fewer than two providers answered \
in any address family";

/// The single answer a run stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headline {
    pub ip: IpAddr,
    pub family: IpFamily,
    /// The provider that gave this answer (with `--verify`, the earliest in the verified family).
    pub provider: &'static Provider,
    pub latency: Option<Duration>,
    /// `(agree, total)` of the headline's family; only with `--verify`.
    pub verified: Option<(usize, usize)>,
}

/// Everything a `public` lookup found out, before anything is printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicReport {
    attempts: Vec<Attempt>,
    /// `Some` exactly when the lookup was a `--verify` one.
    verdict: Option<Verdict>,
    headline: Option<Headline>,
    error: Option<String>,
    code: u8,
}

impl PublicReport {
    /// `attempts` must be in provider order.
    pub fn from_attempts(attempts: Vec<Attempt>, verify: bool) -> Self {
        if verify {
            Self::compared(attempts)
        } else {
            Self::first_valid(attempts)
        }
    }

    fn first_valid(attempts: Vec<Attempt>) -> Self {
        let headline = attempts.iter().find_map(|attempt| {
            let ip = *attempt.outcome.as_ref().ok()?;
            Some(Headline {
                ip,
                family: IpFamily::of(ip),
                provider: attempt.provider,
                latency: attempt.latency,
                verified: None,
            })
        });
        let failed = headline.is_none();
        Self {
            attempts,
            verdict: None,
            error: failed.then(|| NO_ANSWER.to_owned()),
            code: u8::from(failed),
            headline,
        }
    }

    fn compared(attempts: Vec<Attempt>) -> Self {
        let verdict = Verdict::from_attempts(&attempts);
        let code = verdict.exit_code();
        let headline = verdict.headline().and_then(|family| {
            let first = family.answers.first()?;
            Some(Headline {
                ip: family.ip?,
                family: family.family,
                provider: first.provider,
                latency: first.latency,
                verified: Some((family.agree, family.total)),
            })
        });
        let error = match code {
            0 => None,
            3 => Some(render::disagreement_summary(&verdict)),
            _ if verdict.families.is_empty() => Some(NO_ANSWER.to_owned()),
            _ => Some(NOT_VERIFIED.to_owned()),
        };
        Self {
            attempts,
            verdict: Some(verdict),
            headline,
            error,
            code,
        }
    }

    /// Present exactly when the exit code is 0.
    pub fn headline(&self) -> Option<&Headline> {
        self.headline.as_ref()
    }

    /// Why there is no headline; `None` when there is one.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The exit code of `myip public`: 0, 1, or 3 (a `--verify` disagreement).
    pub fn code(&self) -> u8 {
        self.code
    }

    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    /// The per-family comparison; present only for a `--verify` lookup.
    pub fn verdict(&self) -> Option<&Verdict> {
        self.verdict.as_ref()
    }
}

/// Asks the providers and judges the answers. `seen` is told about every attempt, live and in
/// order for the sequential lookup, and in provider order once all are in with `--verify`.
pub fn lookup(
    fetch: &dyn Fetch,
    options: &Options,
    verify: bool,
    seen: &mut dyn FnMut(&Attempt),
) -> PublicReport {
    let query = Query {
        fetch,
        timeout_secs: options.timeout,
        options: FetchOptions {
            timeout: Duration::from_secs(options.timeout),
            max_bytes: MAX_ANSWER_BYTES,
            family: options.family(),
            proxy: options.proxy.clone(),
            accept: "text/plain",
        },
    };
    let attempts = if verify {
        let attempts = query.ask_all();
        attempts.iter().for_each(&mut *seen);
        attempts
    } else {
        query.ask_in_order(seen)
    };
    PublicReport::from_attempts(attempts, verify)
}

pub fn run(options: &Options, verify: bool, ctx: &mut Context<'_>) -> u8 {
    let verbose = options.verbose;
    let report = lookup(ctx.fetch, options, verify, &mut |attempt| {
        if verbose {
            let _ = writeln!(ctx.err, "myip: {}", render::attempt_line(attempt));
        }
    });
    emit(&report, options, ctx)
}

fn emit(report: &PublicReport, options: &Options, ctx: &mut Context<'_>) -> u8 {
    let code = report.code();
    let written = if options.json {
        writeln!(ctx.out, "{}", report.to_json().render(false))
    } else if let Some(headline) = report.headline() {
        writeln!(ctx.out, "{}", headline.ip)
    } else if code == 3 {
        write!(ctx.out, "{}", render::disagreement_report(report))
    } else {
        Ok(())
    };
    if let Some(error) = report.error() {
        let _ = writeln!(ctx.err, "myip: {error}");
        // With -v the attempts have been printed as they happened, and JSON carries them.
        if code == 1 && !options.json && !options.verbose {
            let _ = write!(ctx.err, "{}", render::attempt_list(report));
        }
    }
    if written.is_err() { 1 } else { code }
}

struct Query<'a> {
    fetch: &'a dyn Fetch,
    options: FetchOptions,
    timeout_secs: u64,
}

impl Query<'_> {
    fn ask_in_order(&self, seen: &mut dyn FnMut(&Attempt)) -> Vec<Attempt> {
        let mut attempts = Vec::new();
        for provider in &PROVIDERS {
            let attempt = self.ask(provider);
            seen(&attempt);
            let answered = attempt.outcome.is_ok();
            attempts.push(attempt);
            if answered {
                break;
            }
        }
        attempts
    }

    /// All providers at once, so the run takes about one timeout. The attempts come back in
    /// provider order whatever order the answers arrived in.
    fn ask_all(&self) -> Vec<Attempt> {
        thread::scope(|scope| {
            let handles: Vec<_> = PROVIDERS
                .iter()
                .map(|provider| scope.spawn(move || self.ask(provider)))
                .collect();
            handles
                .into_iter()
                .zip(&PROVIDERS)
                .map(|(handle, provider)| {
                    handle.join().unwrap_or_else(|_| Attempt {
                        provider,
                        outcome: Err("the lookup failed unexpectedly".to_owned()),
                        latency: None,
                    })
                })
                .collect()
        })
    }

    fn ask(&self, provider: &'static Provider) -> Attempt {
        if self.options.family == Family::V6 && !provider.ipv6 {
            return Attempt {
                provider,
                outcome: Err("has no IPv6 address, not asked".to_owned()),
                latency: None,
            };
        }
        let started = Instant::now();
        let response = self.fetch.get(provider.url, &self.options);
        let latency = started.elapsed();
        let outcome = match response {
            Ok(response) => read_answer(provider.parser, &response, self.options.family)
                .map_err(|reject| reject.to_string()),
            Err(error) => Err(self.describe(&error)),
        };
        Attempt {
            provider,
            outcome,
            latency: Some(latency),
        }
    }

    /// Transport messages can name the proxy URL, so they go through the redacting `detail`.
    fn describe(&self, error: &FetchError) -> String {
        match error {
            FetchError::Timeout(_) => format!("timed out after {}s", self.timeout_secs),
            FetchError::TooLarge(_) => format!("answer larger than {MAX_ANSWER_BYTES} bytes"),
            FetchError::Transport(message) => detail(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::Mutex;

    use clap::Parser;

    use super::*;
    use crate::cli::Cli;
    use crate::http::ProxyMode;
    use crate::http::testing::{RouteFetch, reply};
    use crate::json::Json;
    use crate::net::{Scope, classify};

    const A: &str = "1.2.3.4";
    const B: &str = "5.6.7.8";
    const V6: &str = "2606:4700:4700::1111";
    const V6B: &str = "2a00:1450:4001:81b::200e";

    /// What one provider says in a test.
    #[derive(Clone, Copy)]
    enum Slot {
        Ip(&'static str),
        Raw(&'static str),
        Status(u16),
        TooBig,
        Timeout,
        Fail(&'static str),
    }
    use Slot::*;

    /// One slot per provider, in ADR order. Provider 1 speaks the Cloudflare trace format.
    fn board(slots: [Slot; 6]) -> RouteFetch {
        let routes = PROVIDERS
            .iter()
            .zip(slots)
            .map(|(provider, slot)| {
                let answer = match slot {
                    Ip(ip) if provider.name == "one.one.one.one" => {
                        reply(200, "text/plain", &format!("fl=1\nip={ip}\nts=1\n"))
                    }
                    Ip(ip) => reply(200, "text/plain", &format!("{ip}\n")),
                    Raw(body) => reply(200, "text/plain", body),
                    Status(code) => reply(code, "text/plain", "slow down"),
                    TooBig => Err(FetchError::TooLarge(5000)),
                    Timeout => Err(FetchError::Timeout("deadline".to_owned())),
                    Fail(message) => Err(FetchError::Transport(message.to_owned())),
                };
                (provider.url, answer)
            })
            .collect();
        RouteFetch::new(routes)
    }

    fn go(fetch: &dyn Fetch, args: &[&str]) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = crate::run(
            &args,
            &mut Context {
                fetch,
                out: &mut out,
                err: &mut err,
            },
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn options(args: &[&str]) -> Options {
        Cli::try_parse_from(std::iter::once("myip").chain(args.iter().copied()))
            .unwrap()
            .options
    }

    fn urls(providers: &[usize]) -> Vec<String> {
        providers
            .iter()
            .map(|&i| PROVIDERS[i].url.to_owned())
            .collect()
    }

    fn sorted(mut list: Vec<String>) -> Vec<String> {
        list.sort();
        list
    }

    fn all(slot: Slot) -> [Slot; 6] {
        [slot; 6]
    }

    // ---- the first valid answer wins, the rest are fallbacks (AC1) ----

    #[test]
    fn the_first_valid_answer_wins_and_later_providers_are_not_asked() {
        let fetch = board([Ip(A), Ip(B), Ip(B), Ip(B), Ip(B), Ip(B)]);
        assert_eq!(
            go(&fetch, &["public"]),
            (0, format!("{A}\n"), String::new())
        );
        assert_eq!(fetch.requested_urls(), urls(&[0]));
    }

    #[test]
    fn an_ipv6_answer_is_a_valid_answer_without_a_family_flag() {
        let fetch = board([Ip(V6), Ip(A), Ip(A), Ip(A), Ip(A), Ip(A)]);
        assert_eq!(
            go(&fetch, &["public"]),
            (0, format!("{V6}\n"), String::new())
        );
    }

    #[test]
    fn a_failure_hands_over_to_the_next_provider_in_adr_order() {
        let fetch = board([Fail("down"), Ip(B), Ip(A), Ip(A), Ip(A), Ip(A)]);
        assert_eq!(
            go(&fetch, &["public"]),
            (0, format!("{B}\n"), String::new())
        );
        assert_eq!(fetch.requested_urls(), urls(&[0, 1]));
    }

    #[test]
    fn every_kind_of_bad_answer_is_an_error_and_falls_back() {
        let cases: [(Slot, &str); 12] = [
            (Timeout, "timed out after 10s"),
            (Status(429), "HTTP 429"),
            (Raw("<!DOCTYPE html><title>Sign in</title>"), "web page"),
            (Raw(""), "empty answer"),
            (Raw("hello"), "not an IP address"),
            (Raw("10.0.0.1"), "not a public address (private)"),
            (Raw("127.0.0.1"), "not a public address (loopback)"),
            (Raw("100.64.0.1"), "not a public address (cgnat)"),
            (Raw("fe80::1"), "not a public address (link-local)"),
            (Raw("::ffff:1.2.3.4"), "IPv4-mapped"),
            (TooBig, "larger than 4096 bytes"),
            (Fail("connection refused"), "connection refused"),
        ];
        for (bad, reason) in cases {
            let fetch = board([Fail("down"), bad, Ip(A), Ip(B), Ip(B), Ip(B)]);
            let (code, out, err) = go(&fetch, &["-v", "public"]);
            assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"), "{reason}: {err}");
            let line = err
                .lines()
                .find(|line| line.starts_with("myip: api64.ipify.org: "))
                .unwrap_or_else(|| panic!("{reason}: {err}"));
            assert!(line.contains(reason), "{reason}: {err}");
            assert_eq!(fetch.requested_urls(), urls(&[0, 1, 2]), "{reason}");
        }
    }

    #[test]
    fn the_trace_answer_must_carry_exactly_one_ip_line() {
        for body in ["fl=1\nts=1\n", "ip=1.2.3.4\nip=5.6.7.8\n", "1.2.3.4\n"] {
            let mut slots = all(Ip(B));
            slots[0] = Raw(body);
            let fetch = board(slots);
            let (code, out, err) = go(&fetch, &["-v", "public"]);
            assert_eq!((code, out.as_str()), (0, "5.6.7.8\n"), "{body:?}: {err}");
            assert!(
                err.contains("one.one.one.one: answer has"),
                "{body:?}: {err}"
            );
        }
    }

    #[test]
    fn all_providers_failing_exits_1_with_nothing_on_stdout_and_the_attempts_on_stderr() {
        let fetch = board([
            Timeout,
            Status(500),
            Raw("<html>portal</html>"),
            Ip("192.168.1.1"),
            Fail("no route"),
            Raw(""),
        ]);
        let (code, out, err) = go(&fetch, &["public"]);
        assert_eq!((code, out.as_str()), (1, ""));
        let lines: Vec<&str> = err.lines().collect();
        assert_eq!(
            lines[0],
            "myip: no provider returned a valid public address"
        );
        assert_eq!(lines.len(), 7, "{err}");
        for (line, provider) in lines[1..].iter().zip(&PROVIDERS) {
            let prefix = format!("  {}: ", provider.name);
            assert!(line.starts_with(&prefix), "{line}");
        }
        assert_eq!(fetch.requested_urls(), urls(&[0, 1, 2, 3, 4, 5]));
    }

    #[test]
    fn verbose_logs_each_attempt_as_it_happens_and_does_not_list_them_again() {
        let fetch = board([Timeout, Ip(A), Ip(B), Ip(B), Ip(B), Ip(B)]);
        let (code, out, err) = go(&fetch, &["-v", "public"]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"));
        let lines: Vec<&str> = err.lines().collect();
        assert_eq!(lines.len(), 2, "{err}");
        assert!(
            lines[0].starts_with("myip: one.one.one.one: timed out after 10s ("),
            "{err}"
        );
        assert!(
            lines[1].starts_with("myip: api64.ipify.org: 1.2.3.4 ("),
            "{err}"
        );

        let (code, _, err) = go(&board(all(Fail("down"))), &["-v", "public"]);
        assert_eq!(code, 1);
        let lines: Vec<&str> = err.lines().collect();
        assert_eq!(lines.len(), 7, "{err}");
        assert!(lines.iter().all(|line| line.starts_with("myip: ")), "{err}");
    }

    #[test]
    fn a_bad_answer_body_is_never_echoed() {
        let hostile = "\u{1b}[2J\u{1b}[31mpwned";
        let fetch = board(all(Raw(hostile)));
        let (code, out, err) = go(&fetch, &["-v", "public"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(!err.contains('\u{1b}') && !err.contains("pwned"), "{err}");
        let (_, out, err) = go(&fetch, &["--json", "public"]);
        assert!(!format!("{out}{err}").contains("pwned"));
    }

    // ---- transport options (AC2) ----

    #[test]
    fn every_request_carries_the_size_cap_the_accept_header_and_the_global_flags() {
        for verify in [false, true] {
            let fetch = board(all(Ip(A)));
            let mut args = vec!["--timeout", "3", "-4", "--proxy", "env", "public"];
            if verify {
                args.push("--verify");
            }
            let (code, _, err) = go(&fetch, &args);
            assert_eq!(code, 0, "{err}");
            let requests = fetch.requests.lock().unwrap();
            assert_eq!(requests.len(), if verify { 6 } else { 1 });
            for (url, opts) in requests.iter() {
                assert!(PROVIDERS.iter().any(|p| p.url == url), "{url}");
                assert_eq!(opts.max_bytes, 4096, "{url}");
                assert_eq!(opts.accept, "text/plain", "{url}");
                assert_eq!(opts.timeout, Duration::from_secs(3), "{url}");
                assert_eq!(opts.family, Family::V4, "{url}");
                assert_eq!(opts.proxy, ProxyMode::Env, "{url}");
            }
        }
    }

    #[test]
    fn an_explicit_proxy_and_the_default_modes_reach_the_transport() {
        let fetch = board(all(Ip(V6)));
        go(
            &fetch,
            &["-6", "--proxy", "http://127.0.0.1:3128", "public"],
        );
        let (_, opts) = fetch.requests.lock().unwrap()[0].clone();
        assert_eq!(opts.family, Family::V6);
        assert_eq!(
            opts.proxy,
            ProxyMode::Url("http://127.0.0.1:3128".to_owned())
        );

        let fetch = board(all(Ip(A)));
        go(&fetch, &["public"]);
        let (_, opts) = fetch.requests.lock().unwrap()[0].clone();
        assert_eq!(opts.family, Family::Any);
        assert_eq!(opts.proxy, ProxyMode::None);
        assert_eq!(opts.timeout, Duration::from_secs(10));
    }

    #[test]
    fn proxy_credentials_never_reach_any_output() {
        const SECRET: &str = "hunter2";
        let proxy = format!("http://{}:{SECRET}@proxy.example:3128", "user");
        let refusal: &'static str =
            Box::leak(format!("proxy {proxy} refused the connection").into_boxed_str());
        let runs: [(&[&str], bool); 4] = [
            (&["-v"], false),
            (&["--json"], false),
            (&[], true),
            (&["-v", "--json"], true),
        ];
        for (global, verify) in runs {
            let fetch = board(all(Fail(refusal)));
            let mut args = vec!["--proxy", proxy.as_str()];
            args.extend_from_slice(global);
            args.push("public");
            if verify {
                args.push("--verify");
            }
            let (code, out, err) = go(&fetch, &args);
            let all_output = format!("{out}{err}");
            assert_eq!(code, 1, "{args:?}");
            assert!(!all_output.contains(SECRET), "{args:?}: {all_output}");
            assert!(all_output.contains("refused the connection"), "{args:?}");
        }
    }

    // ---- --verify (AC3) ----

    #[test]
    fn verify_asks_every_provider_once_and_prints_only_the_headline_when_all_agree() {
        let fetch = board(all(Ip(A)));
        assert_eq!(
            go(&fetch, &["public", "--verify"]),
            (0, format!("{A}\n"), String::new())
        );
        assert_eq!(
            sorted(fetch.requested_urls()),
            sorted(urls(&[0, 1, 2, 3, 4, 5]))
        );
    }

    #[test]
    fn verify_is_concurrent_and_plain_fallback_is_not() {
        let patient = Duration::from_secs(10);
        let fetch = board(all(Ip(A))).requiring_concurrency(6, patient);
        let (code, out, err) = go(&fetch, &["public", "--verify"]);
        assert_eq!((code, out.as_str(), err.as_str()), (0, "1.2.3.4\n", ""));

        // The first request of a sequential run is alone, so the gate refuses it.
        let impatient = Duration::from_millis(30);
        let fetch = board(all(Ip(A))).requiring_concurrency(2, impatient);
        let (code, out, err) = go(&fetch, &["-v", "public"]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"));
        assert!(
            err.starts_with("myip: one.one.one.one: requests were not made concurrently"),
            "{err}"
        );
        assert_eq!(fetch.requested_urls(), urls(&[0, 1]));
    }

    #[test]
    fn providers_that_error_are_listed_but_are_not_a_disagreement() {
        let fetch = board([Timeout, Ip(A), Ip(A), Ip(A), Fail("down"), Status(500)]);
        assert_eq!(
            go(&fetch, &["public", "--verify"]),
            (0, format!("{A}\n"), String::new())
        );
    }

    #[test]
    fn a_split_exits_3_and_names_who_said_what_on_stdout() {
        let fetch = board([Ip(A), Ip(B), Ip(A), Timeout, Fail("down"), Status(503)]);
        let (code, out, err) = go(&fetch, &["public", "--verify"]);
        assert_eq!(code, 3);
        assert_eq!(
            out,
            "\
ipv4: DISAGREE, 2 of 3 agree
  one.one.one.one        1.2.3.4
  api64.ipify.org        5.6.7.8  <- differs
  checkip.amazonaws.com  1.2.3.4
no answer:
  icanhazip.com          timed out after 10s
  ident.me               down
  ipinfo.io              HTTP 503
"
        );
        assert_eq!(err, "myip: providers disagree: ipv4 (2 of 3 agree)\n");
    }

    #[test]
    fn a_split_without_a_clear_majority_marks_nobody() {
        let fetch = board([Ip(A), Ip(B), Timeout, Timeout, Timeout, Timeout]);
        let (code, out, _) = go(&fetch, &["public", "--verify"]);
        assert_eq!(code, 3);
        assert!(out.starts_with("ipv4: DISAGREE, 1 of 2 agree\n"), "{out}");
        assert!(!out.contains("<- differs"), "{out}");
    }

    #[test]
    fn a_lone_answer_in_the_other_family_does_not_fail_the_run() {
        let fetch = board([Ip(A), Ip(A), Ip(A), Ip(V6), Ip(A), Ip(A)]);
        assert_eq!(
            go(&fetch, &["public", "--verify"]),
            (0, format!("{A}\n"), String::new())
        );
    }

    #[test]
    fn a_disagreement_wins_over_a_verified_family() {
        let fetch = board([Ip(A), Ip(B), Ip(A), Ip(V6), Ip(V6), Ip(B)]);
        let (code, out, err) = go(&fetch, &["public", "--verify"]);
        assert_eq!(code, 3, "{out}{err}");
        assert!(out.contains("ipv4: DISAGREE, 2 of 4 agree"), "{out}");
        assert!(out.contains("ipv6: verified, 2 of 2 agree"), "{out}");
        assert_eq!(err, "myip: providers disagree: ipv4 (2 of 4 agree)\n");
    }

    #[test]
    fn a_disagreement_in_both_families_is_reported_for_both() {
        let fetch = board([Ip(A), Ip(B), Ip(V6), Ip(V6B), Timeout, Timeout]);
        let (code, _, err) = go(&fetch, &["public", "--verify"]);
        assert_eq!(code, 3);
        assert_eq!(
            err,
            "myip: providers disagree: ipv4 (1 of 2 agree), ipv6 (1 of 2 agree)\n"
        );
    }

    #[test]
    fn a_dual_stack_split_between_families_is_not_a_disagreement() {
        let fetch = board([Ip(A), Ip(A), Ip(V6), Ip(V6), Timeout, Timeout]);
        assert_eq!(
            go(&fetch, &["public", "--verify"]),
            (0, format!("{A}\n"), String::new())
        );
        let fetch = board([Ip(V6), Ip(V6), Ip(A), Ip(A), Timeout, Timeout]);
        assert_eq!(
            go(&fetch, &["public", "--verify"]),
            (0, format!("{V6}\n"), String::new()),
            "the family whose first answer came earliest is the headline"
        );
    }

    #[test]
    fn fewer_than_two_answers_in_every_family_is_exit_1() {
        let fetch = board([Ip(A), Timeout, Fail("down"), Status(500), Raw(""), Ip(V6)]);
        let (code, out, err) = go(&fetch, &["public", "--verify"]);
        assert_eq!((code, out.as_str()), (1, ""));
        let first = "myip: the address could not be verified: fewer than two providers \
                     answered in any address family\n";
        assert!(err.starts_with(first), "{err}");
        assert_eq!(err.lines().count(), 1 + 6, "{err}");

        let (code, out, err) = go(&board(all(Fail("down"))), &["public", "--verify"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(
            err.starts_with("myip: no provider returned a valid public address\n"),
            "{err}"
        );
    }

    #[test]
    fn verbose_verify_logs_in_provider_order_whatever_the_arrival_order() {
        let fetch = board(all(Ip(A)));
        let (code, _, err) = go(&fetch, &["-v", "public", "--verify"]);
        assert_eq!(code, 0);
        let names: Vec<&str> = err
            .lines()
            .map(|line| line.strip_prefix("myip: ").unwrap())
            .map(|line| line.split(':').next().unwrap())
            .collect();
        let want: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
        assert_eq!(names, want, "{err}");
    }

    /// Fails the way a buggy transport would: by panicking.
    struct PanicsOn {
        url: &'static str,
        rest: RouteFetch,
    }

    impl Fetch for PanicsOn {
        fn get(&self, url: &str, opts: &FetchOptions) -> Result<crate::http::Response, FetchError> {
            assert_ne!(url, self.url, "simulated bug in the transport");
            self.rest.get(url, opts)
        }
    }

    #[test]
    fn a_panicking_request_is_one_failed_attempt_not_a_crash() {
        let fetch = PanicsOn {
            url: PROVIDERS[2].url,
            rest: board(all(Ip(A))),
        };
        let (code, out, err) = go(&fetch, &["-v", "public", "--verify"]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"), "{err}");
        assert!(
            err.contains("myip: checkip.amazonaws.com: the lookup failed unexpectedly\n"),
            "{err}"
        );
    }

    // ---- address families (AC4) ----

    #[test]
    fn ipv6_mode_never_asks_a_provider_that_has_no_ipv6_address() {
        let fetch = board([Timeout, Fail("down"), Ip(V6), Ip(V6B), Ip(V6), Ip(V6)]);
        assert_eq!(
            go(&fetch, &["-6", "public"]),
            (0, format!("{V6B}\n"), String::new())
        );
        assert_eq!(fetch.requested_urls(), urls(&[0, 1, 3]));

        let fetch = board(all(Fail("down")));
        let (code, out, err) = go(&fetch, &["-6", "public"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert_eq!(fetch.requested_urls(), urls(&[0, 1, 3, 4]));
        assert!(
            err.contains("  checkip.amazonaws.com: has no IPv6 address, not asked\n"),
            "{err}"
        );
        assert!(
            err.contains("  ipinfo.io: has no IPv6 address, not asked\n"),
            "{err}"
        );
    }

    #[test]
    fn ipv6_verify_needs_two_of_the_four_providers_that_have_ipv6() {
        let fetch = board(all(Ip(V6)));
        assert_eq!(
            go(&fetch, &["-6", "public", "--verify"]),
            (0, format!("{V6}\n"), String::new())
        );
        assert_eq!(sorted(fetch.requested_urls()), sorted(urls(&[0, 1, 3, 4])));

        let fetch = board([
            Ip(V6),
            Fail("down"),
            Ip(V6),
            Fail("down"),
            Fail("down"),
            Ip(V6),
        ]);
        let (code, out, err) = go(&fetch, &["-6", "public", "--verify"]);
        assert_eq!((code, out.as_str()), (1, ""), "{err}");
        assert!(err.contains("could not be verified"), "{err}");
    }

    #[test]
    fn an_answer_of_the_wrong_family_is_an_error() {
        let fetch = board([Ip(V6), Ip(A), Ip(B), Ip(B), Ip(B), Ip(B)]);
        let (code, out, err) = go(&fetch, &["-4", "-v", "public"]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"), "{err}");
        assert!(
            err.contains("one.one.one.one: answer is an IPv6 address but IPv4 was requested"),
            "{err}"
        );

        let fetch = board([Ip(A), Ip(V6), Ip(B), Ip(B), Ip(B), Ip(B)]);
        let (code, out, err) = go(&fetch, &["-6", "-v", "public"]);
        assert_eq!(
            (code, out.as_str()),
            (0, format!("{V6}\n").as_str()),
            "{err}"
        );
        assert!(
            err.contains("one.one.one.one: answer is an IPv4 address but IPv6 was requested"),
            "{err}"
        );
    }

    #[test]
    fn ipv4_verify_compares_only_ipv4_answers() {
        let fetch = board([Ip(A), Ip(V6), Ip(A), Ip(V6), Ip(A), Ip(A)]);
        let (code, out, err) = go(&fetch, &["-4", "public", "--verify"]);
        assert_eq!((code, out.as_str()), (0, "1.2.3.4\n"), "{err}");
        let fetch = board([Ip(A), Ip(V6), Ip(V6), Ip(V6), Ip(V6), Ip(V6)]);
        let (code, _, err) = go(&fetch, &["-4", "public", "--verify"]);
        assert_eq!(code, 1, "{err}");
    }

    #[test]
    fn both_family_flags_together_are_a_usage_error_without_any_request() {
        let fetch = board(all(Ip(A)));
        let (code, out, err) = go(&fetch, &["-4", "-6", "public"]);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(err.contains("error:"), "{err}");
        assert!(fetch.requested_urls().is_empty());
    }

    // ---- the library entry point (for `myip all`) ----

    #[test]
    fn lookup_reports_the_headline_in_both_modes() {
        let fetch = board([Timeout, Ip(A), Ip(A), Ip(B), Ip(A), Ip(A)]);
        let report = lookup(&fetch, &options(&[]), false, &mut |_| {});
        let headline = report.headline().unwrap();
        assert_eq!(headline.ip, A.parse::<IpAddr>().unwrap());
        assert_eq!(headline.family, IpFamily::V4);
        assert_eq!(headline.provider.name, "api64.ipify.org");
        assert_eq!(headline.verified, None);
        assert!(headline.latency.is_some());
        assert_eq!((report.code(), report.error()), (0, None));
        assert_eq!(report.attempts().len(), 2);
        assert!(report.verdict().is_none());

        let fetch = board([Timeout, Ip(A), Ip(A), Ip(A), Ip(A), Timeout]);
        let report = lookup(&fetch, &options(&[]), true, &mut |_| {});
        let headline = report.headline().unwrap();
        assert_eq!(headline.provider.name, "api64.ipify.org");
        assert_eq!(headline.verified, Some((4, 4)));
        assert_eq!((report.code(), report.error()), (0, None));
        assert_eq!(report.attempts().len(), 6);
        assert_eq!(report.verdict().unwrap().families.len(), 1);

        let fetch = board([Timeout, Ip(A), Ip(A), Ip(A), Ip(A), Ip(B)]);
        let report = lookup(&fetch, &options(&[]), true, &mut |_| {});
        assert_eq!(report.headline(), None);
        assert_eq!(
            (report.code(), report.error()),
            (3, Some("providers disagree: ipv4 (4 of 5 agree)"))
        );
    }

    #[test]
    fn lookup_has_a_headline_exactly_when_the_code_is_0() {
        let scenarios = scenarios();
        assert!(scenarios.iter().any(|s| s.expect == 0));
        assert!(scenarios.iter().any(|s| s.expect == 1));
        assert!(scenarios.iter().any(|s| s.expect == 3));
        for scenario in scenarios {
            let report = scenario.lookup();
            assert_eq!(report.code(), scenario.expect, "{}", scenario.name);
            assert_eq!(
                report.headline().is_some(),
                report.code() == 0,
                "{}",
                scenario.name
            );
            assert_eq!(
                report.error().is_none(),
                report.headline().is_some(),
                "{}",
                scenario.name
            );
        }
    }

    #[test]
    fn lookup_tells_the_caller_about_each_attempt_in_provider_order() {
        let mut names = Vec::new();
        let fetch = board([Timeout, Fail("down"), Ip(A), Ip(B), Ip(B), Ip(B)]);
        lookup(&fetch, &options(&[]), false, &mut |attempt| {
            names.push(attempt.provider.name);
        });
        assert_eq!(
            names,
            [
                "one.one.one.one",
                "api64.ipify.org",
                "checkip.amazonaws.com"
            ]
        );

        let mut names = Vec::new();
        lookup(&board(all(Ip(A))), &options(&[]), true, &mut |attempt| {
            names.push(attempt.provider.name);
        });
        let want: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
        assert_eq!(names, want);
    }

    // ---- --json (AC6) ----

    /// A reproducible run: the slots, the arguments before `public`, and `--verify` or not.
    struct Scenario {
        name: &'static str,
        slots: [Slot; 6],
        flags: &'static [&'static str],
        verify: bool,
        expect: u8,
    }

    impl Scenario {
        fn args(&self) -> Vec<&'static str> {
            let mut args = self.flags.to_vec();
            args.push("public");
            if self.verify {
                args.push("--verify");
            }
            args
        }

        fn lookup(&self) -> PublicReport {
            lookup(
                &board(self.slots),
                &options(self.flags),
                self.verify,
                &mut |_| {},
            )
        }
    }

    fn scenarios() -> Vec<Scenario> {
        let s = |name, slots, flags, verify, expect| Scenario {
            name,
            slots,
            flags,
            verify,
            expect,
        };
        vec![
            s("first answer", all(Ip(A)), &[], false, 0),
            s(
                "after fallbacks",
                [Timeout, Status(429), Raw("<html>"), Ip(V6), Ip(A), Ip(A)],
                &[],
                false,
                0,
            ),
            s("all fail", all(Fail("down")), &[], false, 1),
            s("all fail with -6", all(Timeout), &["-6"], false, 1),
            s("verified", all(Ip(A)), &[], true, 0),
            s(
                "verified with failures",
                [Fail("down"), Ip(A), Ip(A), Timeout, Ip(A), Status(500)],
                &[],
                true,
                0,
            ),
            s(
                "disagree",
                [Ip(A), Ip(B), Ip(A), Timeout, Fail("down"), Status(503)],
                &[],
                true,
                3,
            ),
            s(
                "split, no majority",
                [Ip(A), Ip(B), Timeout, Timeout, Timeout, Timeout],
                &[],
                true,
                3,
            ),
            s(
                "lone ipv6",
                [Ip(A), Ip(A), Ip(A), Ip(V6), Ip(A), Ip(A)],
                &[],
                true,
                0,
            ),
            s(
                "dual stack",
                [Ip(V6), Ip(V6), Ip(A), Ip(A), Timeout, Timeout],
                &[],
                true,
                0,
            ),
            s(
                "both families split",
                [Ip(A), Ip(B), Ip(V6), Ip(V6B), Timeout, Timeout],
                &[],
                true,
                3,
            ),
            s(
                "one answer",
                [Ip(A), Timeout, Fail("down"), Status(500), Raw(""), Ip(V6)],
                &[],
                true,
                1,
            ),
            s("nothing", all(Fail("down")), &[], true, 1),
            s("-6 verified", all(Ip(V6)), &["-6"], true, 0),
            s(
                "-6 not verifiable",
                [
                    Ip(V6),
                    Fail("down"),
                    Ip(V6),
                    Fail("down"),
                    Fail("down"),
                    Ip(V6),
                ],
                &["-6"],
                true,
                1,
            ),
            s(
                "-4 verified",
                [Ip(A), Ip(V6), Ip(A), Ip(V6), Ip(A), Ip(A)],
                &["-4"],
                true,
                0,
            ),
        ]
    }

    /// `"latency_ms":123` becomes `"latency_ms":N`, so output can be compared across runs.
    fn normalized(text: &str) -> String {
        let key = "\"latency_ms\":";
        let (mut out, mut rest) = (String::new(), text);
        while let Some(at) = rest.find(key) {
            let (head, tail) = rest.split_at(at + key.len());
            out.push_str(head);
            let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if digits > 0 {
                out.push('N');
            }
            rest = &tail[digits..];
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn json_golden_first_valid_after_a_fallback() {
        let fetch = board([Timeout, Ip(A), Ip(B), Ip(B), Ip(B), Ip(B)]);
        let (code, out, err) = go(&fetch, &["--timeout", "7", "--json", "public"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(
            normalized(&out),
            concat!(
                r#"{"schema_version":1,"ip":"1.2.3.4","family":"ipv4","provider":"api64.ipify.org","#,
                r#""latency_ms":N,"verified":null,"error":null,"attempts":["#,
                r#"{"provider":"one.one.one.one","url":"https://one.one.one.one/cdn-cgi/trace","#,
                r#""ip":null,"latency_ms":N,"error":"timed out after 7s"},"#,
                r#"{"provider":"api64.ipify.org","url":"https://api64.ipify.org","#,
                r#""ip":"1.2.3.4","latency_ms":N,"error":null}]}"#,
                "\n"
            )
        );
    }

    #[test]
    fn json_golden_verify_disagreement_goes_to_stdout_with_null_headline_fields() {
        let fetch = board([Ip(A), Ip(B), Ip(A), Timeout, Fail("down"), Status(503)]);
        let (code, out, err) = go(&fetch, &["--json", "public", "--verify"]);
        assert_eq!(code, 3);
        assert_eq!(err, "myip: providers disagree: ipv4 (2 of 3 agree)\n");
        assert_eq!(
            normalized(&out),
            concat!(
                r#"{"schema_version":1,"ip":null,"family":null,"provider":null,"latency_ms":null,"#,
                r#""verified":null,"error":"providers disagree: ipv4 (2 of 3 agree)","attempts":["#,
                r#"{"provider":"one.one.one.one","url":"https://one.one.one.one/cdn-cgi/trace","ip":"1.2.3.4","latency_ms":N,"error":null},"#,
                r#"{"provider":"api64.ipify.org","url":"https://api64.ipify.org","ip":"5.6.7.8","latency_ms":N,"error":null},"#,
                r#"{"provider":"checkip.amazonaws.com","url":"https://checkip.amazonaws.com","ip":"1.2.3.4","latency_ms":N,"error":null},"#,
                r#"{"provider":"icanhazip.com","url":"https://icanhazip.com","ip":null,"latency_ms":N,"error":"timed out after 10s"},"#,
                r#"{"provider":"ident.me","url":"https://ident.me","ip":null,"latency_ms":N,"error":"down"},"#,
                r#"{"provider":"ipinfo.io","url":"https://ipinfo.io/ip","ip":null,"latency_ms":N,"error":"HTTP 503"}],"#,
                r#""families":[{"family":"ipv4","status":"disagree","ip":null,"agree":2,"total":3,"providers":["#,
                r#"{"provider":"one.one.one.one","ip":"1.2.3.4","latency_ms":N},"#,
                r#"{"provider":"api64.ipify.org","ip":"5.6.7.8","latency_ms":N},"#,
                r#"{"provider":"checkip.amazonaws.com","ip":"1.2.3.4","latency_ms":N}]}]}"#,
                "\n"
            )
        );
    }

    #[test]
    fn json_golden_verified_dual_stack_keeps_the_headline_and_lists_both_families() {
        let fetch = board([Ip(V6), Ip(V6), Ip(A), Ip(A), Timeout, Timeout]);
        let (code, out, err) = go(&fetch, &["--json", "public", "--verify"]);
        assert_eq!((code, err.as_str()), (0, ""));
        let out = normalized(&out);
        assert!(
            out.starts_with(concat!(
                r#"{"schema_version":1,"ip":"2606:4700:4700::1111","family":"ipv6","#,
                r#""provider":"one.one.one.one","latency_ms":N,"verified":{"agree":2,"total":2},"error":null,"attempts":["#
            )),
            "{out}"
        );
        assert!(
            out.contains(concat!(
                r#""families":[{"family":"ipv4","status":"verified","ip":"1.2.3.4","agree":2,"total":2,"providers":["#,
                r#"{"provider":"checkip.amazonaws.com","ip":"1.2.3.4","latency_ms":N},"#,
                r#"{"provider":"icanhazip.com","ip":"1.2.3.4","latency_ms":N}]},"#,
                r#"{"family":"ipv6","status":"verified","ip":"2606:4700:4700::1111","agree":2,"total":2,"providers":["#,
                r#"{"provider":"one.one.one.one","ip":"2606:4700:4700::1111","latency_ms":N},"#,
                r#"{"provider":"api64.ipify.org","ip":"2606:4700:4700::1111","latency_ms":N}]}]}"#
            )),
            "{out}"
        );
    }

    #[test]
    fn json_golden_all_failing_still_prints_one_document() {
        let fetch = board(all(Fail("down")));
        let (code, out, err) = go(&fetch, &["--json", "public"]);
        assert_eq!(code, 1);
        assert_eq!(err, "myip: no provider returned a valid public address\n");
        let attempts: Vec<String> = PROVIDERS
            .iter()
            .map(|p| {
                format!(
                    r#"{{"provider":"{}","url":"{}","ip":null,"latency_ms":N,"error":"down"}}"#,
                    p.name, p.url
                )
            })
            .collect();
        assert_eq!(
            normalized(&out),
            format!(
                concat!(
                    r#"{{"schema_version":1,"ip":null,"family":null,"provider":null,"latency_ms":null,"#,
                    r#""verified":null,"error":"no provider returned a valid public address","attempts":[{}]}}"#,
                    "\n"
                ),
                attempts.join(",")
            )
        );
    }

    #[test]
    fn json_under_ipv6_reports_skipped_providers_without_a_latency() {
        let fetch = board(all(Fail("down")));
        let (_, out, _) = go(&fetch, &["--json", "-6", "public"]);
        assert!(
            out.contains(concat!(
                r#"{"provider":"checkip.amazonaws.com","url":"https://checkip.amazonaws.com","#,
                r#""ip":null,"latency_ms":null,"error":"has no IPv6 address, not asked"}"#
            )),
            "{out}"
        );
    }

    fn object(json: &Json) -> &[(String, Json)] {
        match json {
            Json::Object(fields) => fields,
            other => panic!("not an object: {other:?}"),
        }
    }

    fn keys(fields: &[(String, Json)]) -> Vec<&str> {
        fields.iter().map(|(key, _)| key.as_str()).collect()
    }

    fn get<'a>(fields: &'a [(String, Json)], key: &str) -> &'a Json {
        &fields.iter().find(|(name, _)| name == key).unwrap().1
    }

    fn text(json: &Json) -> &str {
        match json {
            Json::Str(text) => text,
            other => panic!("not a string: {other:?}"),
        }
    }

    fn int(json: &Json) -> i64 {
        match json {
            Json::Int(n) => *n,
            other => panic!("not an integer: {other:?}"),
        }
    }

    fn is_null(json: &Json) -> bool {
        matches!(json, Json::Null)
    }

    fn global_address(json: &Json) -> IpAddr {
        let ip: IpAddr = text(json).parse().unwrap();
        assert_eq!(classify(ip), Scope::Global, "{ip}");
        ip
    }

    fn array(json: &Json) -> &[Json] {
        match json {
            Json::Array(items) => items,
            other => panic!("not an array: {other:?}"),
        }
    }

    /// Checks the whole document against the schema of ADR 0001.
    fn validate(doc: &Json, verify: bool) {
        let top = object(doc);
        let mut want = vec![
            "schema_version",
            "ip",
            "family",
            "provider",
            "latency_ms",
            "verified",
            "error",
            "attempts",
        ];
        if verify {
            want.push("families");
        }
        assert_eq!(keys(top), want);
        assert_eq!(get(top, "schema_version"), &Json::Int(1));

        let headline = !is_null(get(top, "ip"));
        for key in ["family", "provider", "latency_ms"] {
            assert_eq!(is_null(get(top, key)), !headline, "{key}");
        }
        assert_eq!(matches!(get(top, "error"), Json::Str(_)), !headline);
        if headline {
            let ip = global_address(get(top, "ip"));
            assert_eq!(text(get(top, "family")), IpFamily::of(ip).as_str());
            assert!(
                PROVIDERS
                    .iter()
                    .any(|p| p.name == text(get(top, "provider")))
            );
            assert!(int(get(top, "latency_ms")) >= 0);
        }
        match (verify && headline, get(top, "verified")) {
            (false, verified) => assert!(is_null(verified)),
            (true, verified) => {
                let verified = object(verified);
                assert_eq!(keys(verified), ["agree", "total"]);
                let (agree, total) = (int(get(verified, "agree")), int(get(verified, "total")));
                assert!(agree >= 2 && agree == total, "{agree} of {total}");
            }
        }

        let attempts = array(get(top, "attempts"));
        let mut answered = 0;
        for attempt in attempts {
            let attempt = object(attempt);
            assert_eq!(
                keys(attempt),
                ["provider", "url", "ip", "latency_ms", "error"]
            );
            let (name, url) = (text(get(attempt, "provider")), text(get(attempt, "url")));
            assert!(PROVIDERS.iter().any(|p| (p.name, p.url) == (name, url)));
            assert_ne!(is_null(get(attempt, "ip")), is_null(get(attempt, "error")));
            if is_null(get(attempt, "error")) {
                global_address(get(attempt, "ip"));
                assert!(int(get(attempt, "latency_ms")) >= 0);
                answered += 1;
            } else {
                assert!(!text(get(attempt, "error")).is_empty());
                assert!(matches!(
                    get(attempt, "latency_ms"),
                    Json::Null | Json::Int(_)
                ));
            }
        }
        if verify || !headline {
            assert_eq!(attempts.len(), PROVIDERS.len());
        } else {
            assert!(attempts.len() <= PROVIDERS.len());
            assert_eq!(answered, 1, "the first valid answer is the last one asked");
        }

        if !verify {
            return;
        }
        let mut counted = 0;
        for family in array(get(top, "families")) {
            let family = object(family);
            assert_eq!(
                keys(family),
                ["family", "status", "ip", "agree", "total", "providers"]
            );
            assert!(["ipv4", "ipv6"].contains(&text(get(family, "family"))));
            let (agree, total) = (int(get(family, "agree")), int(get(family, "total")));
            match text(get(family, "status")) {
                "verified" => {
                    assert!(total >= 2 && agree == total);
                    global_address(get(family, "ip"));
                }
                "unverified" => {
                    assert_eq!((agree, total), (1, 1));
                    global_address(get(family, "ip"));
                }
                "disagree" => {
                    assert!(total >= 2 && agree < total);
                    assert!(is_null(get(family, "ip")));
                }
                other => panic!("unknown status {other}"),
            }
            let providers = array(get(family, "providers"));
            assert_eq!(providers.len() as i64, total);
            for answer in providers {
                let answer = object(answer);
                assert_eq!(keys(answer), ["provider", "ip", "latency_ms"]);
                let ip = global_address(get(answer, "ip"));
                assert_eq!(IpFamily::of(ip).as_str(), text(get(family, "family")));
            }
            counted += total;
        }
        assert_eq!(counted, answered);
    }

    #[test]
    fn every_json_document_satisfies_the_schema() {
        for scenario in scenarios() {
            validate(&scenario.lookup().to_json(), scenario.verify);
        }
    }

    #[test]
    fn json_output_is_one_compact_line_and_matches_the_report_in_every_scenario() {
        for scenario in scenarios() {
            let mut args = vec!["--json"];
            args.extend(scenario.args());
            let (code, out, _) = go(&board(scenario.slots), &args);
            assert_eq!(code, scenario.expect, "{}", scenario.name);
            assert!(
                out.ends_with('\n') && !out[..out.len() - 1].contains('\n'),
                "{}",
                scenario.name
            );
            let report = scenario.lookup().to_json().render(false);
            assert_eq!(
                normalized(&out),
                format!("{}\n", normalized(&report)),
                "{}",
                scenario.name
            );
        }
    }

    #[test]
    fn plain_output_follows_the_stdout_and_stderr_contract_in_every_scenario() {
        for scenario in scenarios() {
            let (code, out, err) = go(&board(scenario.slots), &scenario.args());
            let name = scenario.name;
            assert_eq!(code, scenario.expect, "{name}");
            match code {
                0 => assert_eq!((out.lines().count(), err.as_str()), (1, ""), "{name}"),
                1 => assert!(out.is_empty() && err.starts_with("myip: "), "{name}"),
                _ => {
                    assert!(out.contains("DISAGREE"), "{name}: {out}");
                    assert_eq!(err.lines().count(), 1, "{name}: {err}");
                }
            }
            assert!(out.ends_with('\n') || out.is_empty(), "{name}");
            if code == 0 {
                let ip: IpAddr = out.trim_end().parse().unwrap();
                assert_eq!(classify(ip), Scope::Global, "{name}");
            }
        }
    }

    // ---- output errors ----

    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_stdout_is_exit_1_not_a_panic() {
        for args in [&["public"][..], &["--json", "public"]] {
            let fetch = board(all(Ip(A)));
            let mut err = Vec::new();
            let code = crate::run(
                &args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(),
                &mut Context {
                    fetch: &fetch,
                    out: &mut Closed,
                    err: &mut err,
                },
            );
            assert_eq!(code, 1, "{args:?}");
        }
    }

    #[test]
    fn requests_are_recorded_for_every_attempt_even_across_threads() {
        let seen = Mutex::new(Vec::new());
        let fetch = board(all(Ip(A)));
        let report = lookup(&fetch, &options(&[]), true, &mut |attempt| {
            seen.lock().unwrap().push(attempt.provider.name);
        });
        assert_eq!(seen.lock().unwrap().len(), 6);
        assert_eq!(fetch.requests.lock().unwrap().len(), 6);
        assert_eq!(report.attempts().len(), 6);
    }
}
