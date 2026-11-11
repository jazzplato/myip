//! The text and JSON a `public` lookup prints. Only the JSON is a stable contract
//! (`schema_version` 1, ADR 0001); the human text may change.

use std::time::Duration;

use super::provider::PROVIDERS;
use super::verdict::{Attempt, FamilyReport, Status, Verdict};
use super::{Headline, PublicReport};
use crate::json::Json;

/// `name: 1.2.3.4 (42 ms)` or `name: reason`.
pub fn attempt_line(attempt: &Attempt) -> String {
    let what = match &attempt.outcome {
        Ok(ip) => ip.to_string(),
        Err(reason) => reason.clone(),
    };
    match attempt.latency {
        Some(latency) => format!(
            "{}: {what} ({} ms)",
            attempt.provider.name,
            latency.as_millis()
        ),
        None => format!("{}: {what}", attempt.provider.name),
    }
}

/// Every attempt, one indented line each.
pub fn attempt_list(report: &PublicReport) -> String {
    report
        .attempts()
        .iter()
        .map(|attempt| format!("  {}\n", attempt_line(attempt)))
        .collect()
}

/// `providers disagree: ipv4 (2 of 3 agree)`, for the stderr note and the JSON `error`.
pub fn disagreement_summary(verdict: &Verdict) -> String {
    let families: Vec<String> = verdict
        .disagreements()
        .map(|f| format!("{} ({} of {} agree)", f.family.as_str(), f.agree, f.total))
        .collect();
    format!("providers disagree: {}", families.join(", "))
}

/// What `--verify` prints on stdout when it exits 3: who said what, per family, with the
/// answers that differ from a clear majority marked.
pub fn disagreement_report(report: &PublicReport) -> String {
    let width = PROVIDERS.iter().map(|p| p.name.len()).max().unwrap_or(0);
    let mut text = String::new();
    for family in report.verdict().map_or(&[][..], |v| &v.families[..]) {
        text.push_str(&format!(
            "{}: {}\n",
            family.family.as_str(),
            summary(family)
        ));
        let majority = family.majority();
        for answer in &family.answers {
            let mark = match majority {
                Some(m) if family.status == Status::Disagree && m != answer.ip => "  <- differs",
                _ => "",
            };
            text.push_str(&format!(
                "  {:<width$}  {}{mark}\n",
                answer.provider.name, answer.ip
            ));
        }
    }
    let failed: Vec<&Attempt> = report
        .attempts()
        .iter()
        .filter(|a| a.outcome.is_err())
        .collect();
    if !failed.is_empty() {
        text.push_str("no answer:\n");
        for attempt in failed {
            let reason = attempt.outcome.as_ref().err().map_or("", String::as_str);
            text.push_str(&format!("  {:<width$}  {reason}\n", attempt.provider.name));
        }
    }
    text
}

fn summary(family: &FamilyReport) -> String {
    match family.status {
        Status::Verified => format!("verified, {} of {} agree", family.agree, family.total),
        Status::Unverified => "unverified, 1 answer".to_owned(),
        Status::Disagree => format!("DISAGREE, {} of {} agree", family.agree, family.total),
    }
}

impl PublicReport {
    /// The `--json` document. `ip`, `family`, `provider`, `latency_ms` and `verified` are the
    /// headline and are null together with it; `error` is non-null exactly then. `families`
    /// is present only for a `--verify` lookup.
    pub fn to_json(&self) -> Json {
        let headline = self.headline();
        let mut fields = vec![
            ("schema_version", Json::Int(1)),
            ("ip", headline.map_or(Json::Null, |h| ip(h.ip))),
            (
                "family",
                headline.map_or(Json::Null, |h| h.family.as_str().into()),
            ),
            (
                "provider",
                headline.map_or(Json::Null, |h| h.provider.name.into()),
            ),
            (
                "latency_ms",
                headline.and_then(|h| h.latency).map_or(Json::Null, millis),
            ),
            ("verified", headline.map_or(Json::Null, verified)),
            ("error", self.error().map_or(Json::Null, Json::from)),
            (
                "attempts",
                Json::Array(self.attempts().iter().map(attempt).collect()),
            ),
        ];
        if let Some(verdict) = self.verdict() {
            fields.push((
                "families",
                Json::Array(verdict.families.iter().map(family).collect()),
            ));
        }
        Json::object(fields)
    }
}

fn ip(ip: std::net::IpAddr) -> Json {
    Json::Str(ip.to_string())
}

fn int(n: usize) -> Json {
    Json::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

fn millis(latency: Duration) -> Json {
    Json::Int(i64::try_from(latency.as_millis()).unwrap_or(i64::MAX))
}

fn verified(headline: &Headline) -> Json {
    headline.verified.map_or(Json::Null, |(agree, total)| {
        Json::object([("agree", int(agree)), ("total", int(total))])
    })
}

fn attempt(attempt: &Attempt) -> Json {
    let (address, error) = match &attempt.outcome {
        Ok(found) => (ip(*found), Json::Null),
        Err(reason) => (Json::Null, Json::from(reason.as_str())),
    };
    Json::object([
        ("provider", attempt.provider.name.into()),
        ("url", attempt.provider.url.into()),
        ("ip", address),
        ("latency_ms", attempt.latency.map_or(Json::Null, millis)),
        ("error", error),
    ])
}

fn family(family: &FamilyReport) -> Json {
    let providers = family.answers.iter().map(|answer| {
        Json::object([
            ("provider", answer.provider.name.into()),
            ("ip", ip(answer.ip)),
            ("latency_ms", answer.latency.map_or(Json::Null, millis)),
        ])
    });
    Json::object([
        ("family", family.family.as_str().into()),
        ("status", family.status.as_str().into()),
        ("ip", family.ip.map_or(Json::Null, ip)),
        ("agree", int(family.agree)),
        ("total", int(family.total)),
        ("providers", Json::Array(providers.collect())),
    ])
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    fn at(provider: usize, outcome: Result<&str, &str>, millis: Option<u64>) -> Attempt {
        Attempt {
            provider: &PROVIDERS[provider],
            outcome: outcome
                .map(|ip| ip.parse::<IpAddr>().unwrap())
                .map_err(str::to_owned),
            latency: millis.map(Duration::from_millis),
        }
    }

    #[test]
    fn an_attempt_line_names_the_provider_the_result_and_the_latency() {
        assert_eq!(
            attempt_line(&at(1, Ok("1.2.3.4"), Some(42))),
            "api64.ipify.org: 1.2.3.4 (42 ms)"
        );
        assert_eq!(
            attempt_line(&at(0, Err("HTTP 429"), Some(7))),
            "one.one.one.one: HTTP 429 (7 ms)"
        );
        assert_eq!(
            attempt_line(&at(2, Err("has no IPv6 address, not asked"), None)),
            "checkip.amazonaws.com: has no IPv6 address, not asked"
        );
    }

    #[test]
    fn the_attempt_list_is_one_indented_line_per_attempt() {
        let report = PublicReport::from_attempts(
            vec![at(0, Err("down"), Some(1)), at(1, Err("HTTP 500"), Some(2))],
            false,
        );
        assert_eq!(
            attempt_list(&report),
            "  one.one.one.one: down (1 ms)\n  api64.ipify.org: HTTP 500 (2 ms)\n"
        );
    }

    #[test]
    fn the_summary_names_every_disagreeing_family() {
        let report = PublicReport::from_attempts(
            vec![
                at(0, Ok("1.2.3.4"), Some(1)),
                at(1, Ok("5.6.7.8"), Some(1)),
                at(2, Ok("2606:4700:4700::1111"), Some(1)),
                at(3, Ok("2a00:1450:4001:81b::200e"), Some(1)),
                at(4, Ok("1.2.3.4"), Some(1)),
            ],
            true,
        );
        assert_eq!(
            disagreement_summary(report.verdict().unwrap()),
            "providers disagree: ipv4 (2 of 3 agree), ipv6 (1 of 2 agree)"
        );
    }

    #[test]
    fn the_report_marks_only_answers_that_differ_from_a_clear_majority() {
        let report = PublicReport::from_attempts(
            vec![
                at(0, Ok("1.2.3.4"), Some(1)),
                at(1, Ok("1.2.3.4"), Some(1)),
                at(2, Ok("5.6.7.8"), Some(1)),
                at(3, Err("timed out after 10s"), Some(10_000)),
                at(4, Ok("2606:4700:4700::1111"), Some(1)),
            ],
            true,
        );
        assert_eq!(
            disagreement_report(&report),
            "\
ipv4: DISAGREE, 2 of 3 agree
  one.one.one.one        1.2.3.4
  api64.ipify.org        1.2.3.4
  checkip.amazonaws.com  5.6.7.8  <- differs
ipv6: unverified, 1 answer
  ident.me               2606:4700:4700::1111
no answer:
  icanhazip.com          timed out after 10s
"
        );
    }

    #[test]
    fn a_verified_family_next_to_a_disagreeing_one_is_shown_without_marks() {
        let report = PublicReport::from_attempts(
            vec![
                at(0, Ok("1.2.3.4"), Some(1)),
                at(1, Ok("5.6.7.8"), Some(1)),
                at(2, Ok("2606:4700:4700::1111"), Some(1)),
                at(3, Ok("2606:4700:4700::1111"), Some(1)),
            ],
            true,
        );
        let text = disagreement_report(&report);
        assert!(text.contains("ipv4: DISAGREE, 1 of 2 agree\n"), "{text}");
        assert!(text.contains("ipv6: verified, 2 of 2 agree\n"), "{text}");
        assert!(
            !text.contains("<- differs") && !text.contains("no answer"),
            "{text}"
        );
    }

    #[test]
    fn json_headline_fields_are_set_together_or_null_together() {
        let report = PublicReport::from_attempts(vec![at(1, Ok("1.2.3.4"), Some(5))], false);
        assert_eq!(
            report.to_json().render(false),
            concat!(
                r#"{"schema_version":1,"ip":"1.2.3.4","family":"ipv4","provider":"api64.ipify.org","#,
                r#""latency_ms":5,"verified":null,"error":null,"attempts":["#,
                r#"{"provider":"api64.ipify.org","url":"https://api64.ipify.org","#,
                r#""ip":"1.2.3.4","latency_ms":5,"error":null}]}"#
            )
        );
        let report = PublicReport::from_attempts(Vec::new(), false);
        assert_eq!(
            report.to_json().render(false),
            concat!(
                r#"{"schema_version":1,"ip":null,"family":null,"provider":null,"latency_ms":null,"#,
                r#""verified":null,"error":"no provider returned a valid public address","attempts":[]}"#
            )
        );
    }

    #[test]
    fn json_escapes_what_an_error_message_may_contain() {
        let report = PublicReport::from_attempts(
            vec![at(0, Err("quote \" and \\ and\nnewline"), Some(1))],
            false,
        );
        let json = report.to_json().render(false);
        assert!(
            json.contains(r#""error":"quote \" and \\ and\nnewline""#),
            "{json}"
        );
        assert!(!json.contains('\n'));
    }
}
