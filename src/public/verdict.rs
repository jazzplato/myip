//! What a set of provider answers adds up to: which addresses were seen, whether they agree
//! within each address family, and the exit code that follows (ADR 0001, "Command semantics").

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use super::provider::Provider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpFamily {
    V4,
    V6,
}

impl IpFamily {
    pub fn of(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(_) => Self::V4,
            IpAddr::V6(_) => Self::V6,
        }
    }

    /// The stable spelling used in `--json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V4 => "ipv4",
            Self::V6 => "ipv6",
        }
    }
}

impl fmt::Display for IpFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::V4 => "IPv4",
            Self::V6 => "IPv6",
        })
    }
}

/// One provider's turn: an address, or the reason there is none. Errors never carry the
/// provider's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub provider: &'static Provider,
    pub outcome: Result<IpAddr, String>,
    /// `None` when no request was made (the provider cannot serve the requested family).
    pub latency: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// At least two answers, all equal.
    Verified,
    /// A single answer.
    Unverified,
    /// At least two answers that are not all equal.
    Disagree,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::Disagree => "disagree",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Position of the provider in the fixed provider order.
    pub rank: usize,
    pub provider: &'static Provider,
    pub ip: IpAddr,
    pub latency: Option<Duration>,
}

/// Every answer in one address family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyReport {
    pub family: IpFamily,
    pub status: Status,
    /// The agreed address when verified, the lone answer when unverified, `None` on disagreement.
    pub ip: Option<IpAddr>,
    /// Size of the largest group of equal answers.
    pub agree: usize,
    pub total: usize,
    /// In provider order, never empty.
    pub answers: Vec<Answer>,
}

fn count(answers: &[Answer], ip: IpAddr) -> usize {
    answers.iter().filter(|a| a.ip == ip).count()
}

impl FamilyReport {
    fn new(family: IpFamily, answers: Vec<Answer>) -> Option<Self> {
        let first = answers.first()?.ip;
        let total = answers.len();
        let agree = answers.iter().map(|a| count(&answers, a.ip)).max()?;
        let (status, ip) = if total == 1 {
            (Status::Unverified, Some(first))
        } else if agree == total {
            (Status::Verified, Some(first))
        } else {
            (Status::Disagree, None)
        };
        Some(Self {
            family,
            status,
            ip,
            agree,
            total,
            answers,
        })
    }

    /// The address most answers gave, when exactly one address has the most.
    pub fn majority(&self) -> Option<IpAddr> {
        let mut leaders = self
            .answers
            .iter()
            .map(|a| a.ip)
            .filter(|ip| count(&self.answers, *ip) == self.agree);
        let first = leaders.next()?;
        leaders.all(|ip| ip == first).then_some(first)
    }
}

/// The comparison of a `--verify` run. Only families somebody answered in are present,
/// IPv4 first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub families: Vec<FamilyReport>,
}

impl Verdict {
    /// `attempts` must be in provider order.
    pub fn from_attempts(attempts: &[Attempt]) -> Self {
        let mut v4 = Vec::new();
        let mut v6 = Vec::new();
        for (rank, attempt) in attempts.iter().enumerate() {
            if let Ok(ip) = attempt.outcome {
                let answer = Answer {
                    rank,
                    provider: attempt.provider,
                    ip,
                    latency: attempt.latency,
                };
                match IpFamily::of(ip) {
                    IpFamily::V4 => v4.push(answer),
                    IpFamily::V6 => v6.push(answer),
                }
            }
        }
        Self {
            families: [(IpFamily::V4, v4), (IpFamily::V6, v6)]
                .into_iter()
                .filter_map(|(family, answers)| FamilyReport::new(family, answers))
                .collect(),
        }
    }

    pub fn disagreements(&self) -> impl Iterator<Item = &FamilyReport> {
        self.families
            .iter()
            .filter(|f| f.status == Status::Disagree)
    }

    /// 3 if any family disagrees (wins over everything), else 0 if any is verified, else 1.
    pub fn exit_code(&self) -> u8 {
        if self.disagreements().next().is_some() {
            3
        } else if self.families.iter().any(|f| f.status == Status::Verified) {
            0
        } else {
            1
        }
    }

    /// Only for exit code 0: the verified family whose first answer came earliest in
    /// provider order.
    pub fn headline(&self) -> Option<&FamilyReport> {
        if self.exit_code() != 0 {
            return None;
        }
        self.families
            .iter()
            .filter(|f| f.status == Status::Verified)
            .min_by_key(|f| f.answers[0].rank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::public::provider::PROVIDERS;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    /// One slot per provider, in provider order: an address, or `None` for "errored".
    fn attempts(answers: [Option<&str>; 6]) -> Vec<Attempt> {
        PROVIDERS
            .iter()
            .zip(answers)
            .map(|(provider, answer)| Attempt {
                provider,
                outcome: answer
                    .map(ip)
                    .ok_or_else(|| "timed out after 10s".to_owned()),
                latency: Some(Duration::from_millis(40)),
            })
            .collect()
    }

    const A: &str = "1.2.3.4";
    const B: &str = "5.6.7.8";
    const C: &str = "9.9.9.10";
    const X: &str = "2606:4700:4700::1111";
    const Y: &str = "2a00:1450:4001:81b::200e";

    type Slots = [Option<&'static str>; 6];

    struct Expect {
        code: u8,
        families: &'static [(&'static str, Status, usize, usize)],
        headline: Option<&'static str>,
    }

    fn check(name: &str, answers: Slots, want: &Expect) {
        let verdict = Verdict::from_attempts(&attempts(answers));
        let got: Vec<_> = verdict
            .families
            .iter()
            .map(|f| (f.family.as_str(), f.status, f.agree, f.total))
            .collect();
        assert_eq!(got, want.families, "{name}: families");
        assert_eq!(verdict.exit_code(), want.code, "{name}: exit code");
        assert_eq!(
            verdict.headline().map(|f| f.answers[0].provider.name),
            want.headline,
            "{name}: headline provider"
        );
    }

    #[test]
    fn verification_follows_the_adr_per_family() {
        let n = None;
        let (a, b, c) = (Some(A), Some(B), Some(C));
        let (x, y) = (Some(X), Some(Y));
        let cases: Vec<(&str, Slots, Expect)> = vec![
            (
                "all six agree",
                [a, a, a, a, a, a],
                Expect {
                    code: 0,
                    families: &[("ipv4", Status::Verified, 6, 6)],
                    headline: Some("one.one.one.one"),
                },
            ),
            (
                "three agree, three errored: errors are not disagreement",
                [n, a, n, a, a, n],
                Expect {
                    code: 0,
                    families: &[("ipv4", Status::Verified, 3, 3)],
                    headline: Some("api64.ipify.org"),
                },
            ),
            (
                "two agree and nothing else",
                [n, n, n, n, a, a],
                Expect {
                    code: 0,
                    families: &[("ipv4", Status::Verified, 2, 2)],
                    headline: Some("ident.me"),
                },
            ),
            (
                "a 2-1 split disagrees",
                [a, a, b, n, n, n],
                Expect {
                    code: 3,
                    families: &[("ipv4", Status::Disagree, 2, 3)],
                    headline: None,
                },
            ),
            (
                "one dissenter among six disagrees",
                [a, a, a, a, b, a],
                Expect {
                    code: 3,
                    families: &[("ipv4", Status::Disagree, 5, 6)],
                    headline: None,
                },
            ),
            (
                "two answers that differ",
                [n, n, a, b, n, n],
                Expect {
                    code: 3,
                    families: &[("ipv4", Status::Disagree, 1, 2)],
                    headline: None,
                },
            ),
            (
                "three-way split",
                [a, b, c, n, n, n],
                Expect {
                    code: 3,
                    families: &[("ipv4", Status::Disagree, 1, 3)],
                    headline: None,
                },
            ),
            (
                "a lone answer is unverified, which does not pass the gate",
                [n, n, n, n, a, n],
                Expect {
                    code: 1,
                    families: &[("ipv4", Status::Unverified, 1, 1)],
                    headline: None,
                },
            ),
            (
                "nobody answered",
                [n, n, n, n, n, n],
                Expect {
                    code: 1,
                    families: &[],
                    headline: None,
                },
            ),
            (
                "five over IPv4 plus a lone IPv6 answer: the lone one does not fail the run",
                [a, a, a, x, a, a],
                Expect {
                    code: 0,
                    families: &[
                        ("ipv4", Status::Verified, 5, 5),
                        ("ipv6", Status::Unverified, 1, 1),
                    ],
                    headline: Some("one.one.one.one"),
                },
            ),
            (
                "an IPv4 split next to a lone IPv6 answer still exits 3",
                [a, a, b, x, n, n],
                Expect {
                    code: 3,
                    families: &[
                        ("ipv4", Status::Disagree, 2, 3),
                        ("ipv6", Status::Unverified, 1, 1),
                    ],
                    headline: None,
                },
            ),
            (
                "a split between families is not a disagreement",
                [a, x, n, n, n, n],
                Expect {
                    code: 1,
                    families: &[
                        ("ipv4", Status::Unverified, 1, 1),
                        ("ipv6", Status::Unverified, 1, 1),
                    ],
                    headline: None,
                },
            ),
            (
                "dual stack, each family agrees: the family answered first is the headline",
                [x, a, a, x, a, a],
                Expect {
                    code: 0,
                    families: &[
                        ("ipv4", Status::Verified, 4, 4),
                        ("ipv6", Status::Verified, 2, 2),
                    ],
                    headline: Some("one.one.one.one"),
                },
            ),
            (
                "dual stack, IPv4 answered first",
                [a, x, a, x, n, n],
                Expect {
                    code: 0,
                    families: &[
                        ("ipv4", Status::Verified, 2, 2),
                        ("ipv6", Status::Verified, 2, 2),
                    ],
                    headline: Some("one.one.one.one"),
                },
            ),
            (
                "dual stack, IPv6 answered first although IPv4 has more answers",
                [n, x, a, x, a, a],
                Expect {
                    code: 0,
                    families: &[
                        ("ipv4", Status::Verified, 3, 3),
                        ("ipv6", Status::Verified, 2, 2),
                    ],
                    headline: Some("api64.ipify.org"),
                },
            ),
            (
                "one family disagrees while the other is verified: exit 3 wins",
                [a, a, b, x, x, n],
                Expect {
                    code: 3,
                    families: &[
                        ("ipv4", Status::Disagree, 2, 3),
                        ("ipv6", Status::Verified, 2, 2),
                    ],
                    headline: None,
                },
            ),
            (
                "IPv6 answers that differ disagree",
                [x, y, n, n, n, n],
                Expect {
                    code: 3,
                    families: &[("ipv6", Status::Disagree, 1, 2)],
                    headline: None,
                },
            ),
        ];
        for (name, answers, want) in &cases {
            check(name, *answers, want);
        }
    }

    #[test]
    fn answers_are_equal_by_address_not_by_spelling() {
        let verdict = Verdict::from_attempts(&attempts([
            Some("2606:4700:4700:0:0:0:0:1111"),
            Some("2606:4700:4700::1111"),
            None,
            None,
            None,
            None,
        ]));
        assert_eq!(verdict.exit_code(), 0);
        assert_eq!(verdict.families[0].ip, Some(ip(X)));
    }

    #[test]
    fn family_reports_carry_the_agreed_address() {
        let verdict = Verdict::from_attempts(&attempts([
            Some(X),
            Some(A),
            Some(A),
            Some(Y),
            Some(Y),
            None,
        ]));
        let by_family = |f: IpFamily| verdict.families.iter().find(|r| r.family == f).unwrap();
        let v4 = by_family(IpFamily::V4);
        assert_eq!((v4.status, v4.ip), (Status::Verified, Some(ip(A))));
        let v6 = by_family(IpFamily::V6);
        assert_eq!((v6.status, v6.ip), (Status::Disagree, None));
        assert_eq!((v6.agree, v6.total), (2, 3));
        assert_eq!(v6.majority(), Some(ip(Y)));
    }

    #[test]
    fn the_lone_answer_is_reported_but_not_verified() {
        let verdict = Verdict::from_attempts(&attempts([None, None, None, None, Some(X), None]));
        let only = &verdict.families[0];
        assert_eq!((only.status, only.ip), (Status::Unverified, Some(ip(X))));
    }

    #[test]
    fn majority_needs_a_unique_largest_group() {
        let two_two =
            Verdict::from_attempts(&attempts([Some(A), Some(A), Some(B), Some(B), None, None]));
        assert_eq!(two_two.families[0].majority(), None);
        let two_one =
            Verdict::from_attempts(&attempts([Some(A), Some(B), Some(B), None, None, None]));
        assert_eq!(two_one.families[0].majority(), Some(ip(B)));
    }

    #[test]
    fn answers_keep_provider_order_and_rank() {
        let verdict =
            Verdict::from_attempts(&attempts([None, Some(A), None, Some(A), None, Some(A)]));
        let ranks: Vec<_> = verdict.families[0].answers.iter().map(|a| a.rank).collect();
        assert_eq!(ranks, [1, 3, 5]);
    }

    #[test]
    fn family_spelling_is_stable() {
        assert_eq!(IpFamily::V4.as_str(), "ipv4");
        assert_eq!(IpFamily::V6.as_str(), "ipv6");
        assert_eq!(IpFamily::of(ip(A)), IpFamily::V4);
        assert_eq!(IpFamily::of(ip(X)), IpFamily::V6);
        assert_eq!(Status::Verified.as_str(), "verified");
        assert_eq!(Status::Unverified.as_str(), "unverified");
        assert_eq!(Status::Disagree.as_str(), "disagree");
    }
}
