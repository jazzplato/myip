//! The fixed provider list (ADR 0001) and the strict reader for what a provider answers.

use std::fmt;
use std::net::IpAddr;

use super::verdict::IpFamily;
use crate::http::{Family, Response};
use crate::legacy::looks_like_html;
use crate::net::{Scope, classify};

/// A provider answer is one address, so anything larger is not an answer (ADR 0001).
pub const MAX_ANSWER_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parser {
    /// The whole body is the address.
    PlainText,
    /// Cloudflare's `key=value` lines; the address is the `ip=` line.
    CloudflareTrace,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Provider {
    /// The host, which is also how the provider is named in output.
    pub name: &'static str,
    pub url: &'static str,
    /// Whether the host has an IPv6 address; without one it cannot answer under `-6`.
    pub ipv6: bool,
    pub parser: Parser,
}

/// Normative and ordered: changing a URL, the list or its order needs a new ADR.
pub static PROVIDERS: [Provider; 6] = [
    Provider {
        name: "one.one.one.one",
        url: "https://one.one.one.one/cdn-cgi/trace",
        ipv6: true,
        parser: Parser::CloudflareTrace,
    },
    Provider {
        name: "api64.ipify.org",
        url: "https://api64.ipify.org",
        ipv6: true,
        parser: Parser::PlainText,
    },
    Provider {
        name: "checkip.amazonaws.com",
        url: "https://checkip.amazonaws.com",
        ipv6: false,
        parser: Parser::PlainText,
    },
    Provider {
        name: "icanhazip.com",
        url: "https://icanhazip.com",
        ipv6: true,
        parser: Parser::PlainText,
    },
    Provider {
        name: "ident.me",
        url: "https://ident.me",
        ipv6: true,
        parser: Parser::PlainText,
    },
    Provider {
        name: "ipinfo.io",
        url: "https://ipinfo.io/ip",
        ipv6: false,
        parser: Parser::PlainText,
    },
];

/// Why a provider's response is not an answer. The text never contains the response body:
/// it is untrusted and may carry terminal escape sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    Status(u16),
    TooLarge,
    Empty,
    Html,
    NotAnAddress,
    NoTraceIp,
    DuplicateTraceIp,
    /// An IPv4 address written as IPv6 (`::ffff:a.b.c.d`) is ambiguous about its family.
    MappedV4,
    WrongFamily {
        got: IpFamily,
        wanted: IpFamily,
    },
    NotGlobal(IpAddr, Scope),
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(f, "HTTP {status}"),
            Self::TooLarge => write!(f, "answer larger than {MAX_ANSWER_BYTES} bytes"),
            Self::Empty => f.write_str("empty answer"),
            Self::Html => f.write_str("answer is a web page, not an address"),
            Self::NotAnAddress => f.write_str("answer is not an IP address"),
            Self::NoTraceIp => f.write_str("answer has no ip= line"),
            Self::DuplicateTraceIp => f.write_str("answer has more than one ip= line"),
            Self::MappedV4 => f.write_str("answer is an IPv4-mapped IPv6 address"),
            Self::WrongFamily { got, wanted } => {
                write!(f, "answer is an {got} address but {wanted} was requested")
            }
            Self::NotGlobal(ip, scope) => {
                write!(
                    f,
                    "answer {ip} is not a public address ({})",
                    scope.as_str()
                )
            }
        }
    }
}

/// Accepts a response only if it is a successful, small, non-HTML answer holding exactly one
/// globally routable address of the requested family.
pub fn read_answer(parser: Parser, response: &Response, wanted: Family) -> Result<IpAddr, Reject> {
    if !(200..300).contains(&response.status) {
        return Err(Reject::Status(response.status));
    }
    if response.body.len() > MAX_ANSWER_BYTES {
        return Err(Reject::TooLarge);
    }
    if response.body.trim().is_empty() {
        return Err(Reject::Empty);
    }
    if looks_like_html(&response.body) {
        return Err(Reject::Html);
    }
    let ip = match parser {
        Parser::PlainText => response
            .body
            .trim()
            .parse()
            .map_err(|_| Reject::NotAnAddress)?,
        Parser::CloudflareTrace => trace_ip(&response.body)?,
    };
    if matches!(ip, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some()) {
        return Err(Reject::MappedV4);
    }
    let got = IpFamily::of(ip);
    match (wanted, got) {
        (Family::V4, IpFamily::V6) => Err(Reject::WrongFamily {
            got,
            wanted: IpFamily::V4,
        }),
        (Family::V6, IpFamily::V4) => Err(Reject::WrongFamily {
            got,
            wanted: IpFamily::V6,
        }),
        _ => match classify(ip) {
            Scope::Global => Ok(ip),
            scope => Err(Reject::NotGlobal(ip, scope)),
        },
    }
}

fn trace_ip(body: &str) -> Result<IpAddr, Reject> {
    let mut values = body.lines().filter_map(|line| line.strip_prefix("ip="));
    let value = values.next().ok_or(Reject::NoTraceIp)?;
    if values.next().is_some() {
        return Err(Reject::DuplicateTraceIp);
    }
    value.trim().parse().map_err(|_| Reject::NotAnAddress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::testing::reply;

    fn response(status: u16, body: &str) -> Response {
        reply(status, "text/plain", body).unwrap()
    }

    fn read(parser: Parser, body: &str, wanted: Family) -> Result<IpAddr, Reject> {
        read_answer(parser, &response(200, body), wanted)
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    const TRACE: &str = "fl=123f45\nh=one.one.one.one\nip=1.2.3.4\nts=1700000000.123\n\
visit_scheme=https\nuag=myip/0.0.1\ncolo=AMS\nsliver=none\nhttp=http/2\nloc=NL\n\
tls=TLSv1.3\nsni=plaintext\nwarp=off\ngateway=off\nrbi=off\nkex=X25519\n";

    #[test]
    fn the_provider_list_is_the_adr_list_in_order() {
        let table: Vec<_> = PROVIDERS.iter().map(|p| (p.name, p.url, p.ipv6)).collect();
        assert_eq!(
            table,
            [
                (
                    "one.one.one.one",
                    "https://one.one.one.one/cdn-cgi/trace",
                    true
                ),
                ("api64.ipify.org", "https://api64.ipify.org", true),
                (
                    "checkip.amazonaws.com",
                    "https://checkip.amazonaws.com",
                    false
                ),
                ("icanhazip.com", "https://icanhazip.com", true),
                ("ident.me", "https://ident.me", true),
                ("ipinfo.io", "https://ipinfo.io/ip", false),
            ]
        );
        assert_eq!(PROVIDERS[0].parser, Parser::CloudflareTrace);
        assert!(PROVIDERS[1..].iter().all(|p| p.parser == Parser::PlainText));
    }

    #[test]
    fn every_provider_is_https_and_named_by_its_host() {
        for p in &PROVIDERS {
            let host = p
                .url
                .strip_prefix("https://")
                .unwrap_or_else(|| panic!("{} is not https", p.url));
            assert_eq!(host.split('/').next(), Some(p.name), "{}", p.url);
        }
    }

    #[test]
    fn plain_answers_are_trimmed_and_parsed() {
        for (body, want) in [
            ("1.2.3.4", "1.2.3.4"),
            ("1.2.3.4\n", "1.2.3.4"),
            ("1.2.3.4\r\n", "1.2.3.4"),
            ("  \t1.2.3.4 \n\n", "1.2.3.4"),
            ("2606:4700:4700::1111\n", "2606:4700:4700::1111"),
            ("2A00:1450:4001:81B::200E", "2a00:1450:4001:81b::200e"),
            ("9.9.9.9", "9.9.9.9"),
            ("2002:102:304::1", "2002:102:304::1"),
        ] {
            assert_eq!(
                read(Parser::PlainText, body, Family::Any),
                Ok(ip(want)),
                "{body:?}"
            );
        }
    }

    #[test]
    fn plain_answers_that_are_not_exactly_one_address_are_rejected() {
        for body in [
            "1.2.3.4 5.6.7.8",
            "1.2.3.4\n5.6.7.8\n",
            "1.2.3.4:8080",
            "[2606:4700:4700::1111]",
            "[2606:4700:4700::1111]:443",
            "1.2.3.4/32",
            "01.2.3.4",
            "1.2.3",
            "1.2.3.4.5",
            "256.1.1.1",
            "0x1.2.3.4",
            "fe80::1%eth0",
            "your ip is 1.2.3.4",
            "ip=1.2.3.4",
            "1.2.3.4\u{0}",
            "\u{1b}[31m1.2.3.4",
            "null",
            "{\"ip\":\"1.2.3.4\"}",
        ] {
            assert_eq!(
                read(Parser::PlainText, body, Family::Any),
                Err(Reject::NotAnAddress),
                "{body:?}"
            );
        }
    }

    #[test]
    fn empty_and_blank_answers_are_rejected() {
        for body in ["", "\n", "  \r\n\t "] {
            assert_eq!(
                read(Parser::PlainText, body, Family::Any),
                Err(Reject::Empty),
                "{body:?}"
            );
            assert_eq!(
                read(Parser::CloudflareTrace, body, Family::Any),
                Err(Reject::Empty),
                "{body:?}"
            );
        }
    }

    #[test]
    fn web_pages_are_rejected_even_when_the_status_is_ok() {
        for body in [
            "<!DOCTYPE html><html><body>Sign in to the network</body></html>",
            "\n<html><head><title>Captive portal</title></head></html>",
            "\u{feff}<!-- portal --><HTML>",
            "<body>1.2.3.4</body>",
        ] {
            for parser in [Parser::PlainText, Parser::CloudflareTrace] {
                assert_eq!(
                    read(parser, body, Family::Any),
                    Err(Reject::Html),
                    "{body:?}"
                );
            }
        }
    }

    #[test]
    fn non_2xx_statuses_are_errors_whatever_the_body_says() {
        for status in [199, 300, 301, 403, 404, 429, 500, 503] {
            let got = read_answer(
                Parser::PlainText,
                &response(status, "1.2.3.4\n"),
                Family::Any,
            );
            assert_eq!(got, Err(Reject::Status(status)), "{status}");
        }
        assert_eq!(
            read_answer(Parser::PlainText, &response(202, "1.2.3.4"), Family::Any),
            Ok(ip("1.2.3.4"))
        );
    }

    #[test]
    fn answers_over_the_cap_are_errors_not_truncations() {
        let exactly = format!("1.2.3.4{}", " ".repeat(MAX_ANSWER_BYTES - 7));
        assert_eq!(exactly.len(), MAX_ANSWER_BYTES);
        assert_eq!(
            read(Parser::PlainText, &exactly, Family::Any),
            Ok(ip("1.2.3.4"))
        );
        let over = format!("1.2.3.4{}", " ".repeat(MAX_ANSWER_BYTES - 6));
        assert_eq!(over.len(), MAX_ANSWER_BYTES + 1);
        assert_eq!(
            read(Parser::PlainText, &over, Family::Any),
            Err(Reject::TooLarge)
        );
        let padded_trace = format!("{TRACE}{}", "x=y\n".repeat(MAX_ANSWER_BYTES));
        assert_eq!(
            read(Parser::CloudflareTrace, &padded_trace, Family::Any),
            Err(Reject::TooLarge)
        );
    }

    #[test]
    fn addresses_that_are_not_global_are_rejected_with_their_scope() {
        for (body, scope) in [
            ("10.0.0.1", Scope::Private),
            ("172.16.5.4", Scope::Private),
            ("192.168.1.1", Scope::Private),
            ("127.0.0.1", Scope::Loopback),
            ("::1", Scope::Loopback),
            ("169.254.1.1", Scope::LinkLocal),
            ("fe80::1", Scope::LinkLocal),
            ("100.64.0.1", Scope::Cgnat),
            ("fd00::1", Scope::UniqueLocal),
            ("fc00::1", Scope::UniqueLocal),
            ("224.0.0.1", Scope::Multicast),
            ("ff02::1", Scope::Multicast),
            ("0.0.0.0", Scope::Unspecified),
            ("::", Scope::Unspecified),
            ("192.0.2.1", Scope::Documentation),
            ("198.51.100.7", Scope::Documentation),
            ("203.0.113.9", Scope::Documentation),
            ("2001:db8::1", Scope::Documentation),
            ("255.255.255.255", Scope::Reserved),
        ] {
            assert_eq!(
                read(Parser::PlainText, body, Family::Any),
                Err(Reject::NotGlobal(ip(body), scope)),
                "{body}"
            );
        }
    }

    #[test]
    fn ipv4_mapped_ipv6_is_rejected_even_when_the_embedded_address_is_public() {
        for body in ["::ffff:1.2.3.4", "::ffff:102:304", "::FFFF:9.9.9.9"] {
            for wanted in [Family::Any, Family::V4, Family::V6] {
                assert_eq!(
                    read(Parser::PlainText, body, wanted),
                    Err(Reject::MappedV4),
                    "{body} {wanted:?}"
                );
            }
        }
    }

    #[test]
    fn the_answer_must_be_in_the_requested_family() {
        let (v4, v6) = ("1.2.3.4", "2606:4700:4700::1111");
        assert_eq!(read(Parser::PlainText, v4, Family::V4), Ok(ip(v4)));
        assert_eq!(read(Parser::PlainText, v6, Family::V6), Ok(ip(v6)));
        assert_eq!(read(Parser::PlainText, v4, Family::Any), Ok(ip(v4)));
        assert_eq!(read(Parser::PlainText, v6, Family::Any), Ok(ip(v6)));
        assert_eq!(
            read(Parser::PlainText, v6, Family::V4),
            Err(Reject::WrongFamily {
                got: IpFamily::V6,
                wanted: IpFamily::V4
            })
        );
        assert_eq!(
            read(Parser::PlainText, v4, Family::V6),
            Err(Reject::WrongFamily {
                got: IpFamily::V4,
                wanted: IpFamily::V6
            })
        );
    }

    #[test]
    fn the_cloudflare_trace_is_read_from_its_ip_line() {
        assert_eq!(
            read(Parser::CloudflareTrace, TRACE, Family::Any),
            Ok(ip("1.2.3.4"))
        );
        let crlf = TRACE.replace('\n', "\r\n");
        assert_eq!(
            read(Parser::CloudflareTrace, &crlf, Family::Any),
            Ok(ip("1.2.3.4"))
        );
        let v6 = TRACE.replace("ip=1.2.3.4", "ip=2606:4700:4700::1111");
        assert_eq!(
            read(Parser::CloudflareTrace, &v6, Family::V6),
            Ok(ip("2606:4700:4700::1111"))
        );
        assert_eq!(
            read(Parser::CloudflareTrace, "ip=5.6.7.8", Family::Any),
            Ok(ip("5.6.7.8"))
        );
    }

    #[test]
    fn a_trace_without_exactly_one_valid_ip_line_is_rejected() {
        let missing = TRACE.replace("ip=1.2.3.4\n", "");
        let duplicate = TRACE.replace("ip=1.2.3.4\n", "ip=1.2.3.4\nip=5.6.7.8\n");
        let same_twice = TRACE.replace("ip=1.2.3.4\n", "ip=1.2.3.4\nip=1.2.3.4\n");
        let junk = TRACE.replace("ip=1.2.3.4", "ip=not-an-ip");
        let blank = TRACE.replace("ip=1.2.3.4", "ip=");
        let indented = TRACE.replace("ip=1.2.3.4", " ip=1.2.3.4");
        let uppercase = TRACE.replace("ip=1.2.3.4", "IP=1.2.3.4");
        let embedded = TRACE
            .replace("ip=1.2.3.4\n", "")
            .replace("h=one.one.one.one", "h=ip=1.2.3.4");
        let hello = "hello".to_owned();
        for (name, body, want) in [
            ("missing", &missing, Reject::NoTraceIp),
            ("duplicate", &duplicate, Reject::DuplicateTraceIp),
            ("same twice", &same_twice, Reject::DuplicateTraceIp),
            ("junk", &junk, Reject::NotAnAddress),
            ("blank", &blank, Reject::NotAnAddress),
            ("indented", &indented, Reject::NoTraceIp),
            ("uppercase", &uppercase, Reject::NoTraceIp),
            ("embedded in another value", &embedded, Reject::NoTraceIp),
            ("not a trace", &hello, Reject::NoTraceIp),
        ] {
            assert_eq!(
                read(Parser::CloudflareTrace, body, Family::Any),
                Err(want),
                "{name}"
            );
        }
    }

    #[test]
    fn trace_addresses_get_the_same_checks_as_plain_ones() {
        let with = |ip: &str| TRACE.replace("ip=1.2.3.4", &format!("ip={ip}"));
        assert_eq!(
            read(Parser::CloudflareTrace, &with("10.1.1.1"), Family::Any),
            Err(Reject::NotGlobal(ip("10.1.1.1"), Scope::Private))
        );
        assert_eq!(
            read(
                Parser::CloudflareTrace,
                &with("::ffff:1.2.3.4"),
                Family::Any
            ),
            Err(Reject::MappedV4)
        );
        assert_eq!(
            read(
                Parser::CloudflareTrace,
                &with("2606:4700:4700::1111"),
                Family::V4
            ),
            Err(Reject::WrongFamily {
                got: IpFamily::V6,
                wanted: IpFamily::V4
            })
        );
    }

    #[test]
    fn rejection_text_never_repeats_the_body() {
        let hostile = "\u{1b}]0;pwned\u{7}\u{1b}[2J secret-token";
        for parser in [Parser::PlainText, Parser::CloudflareTrace] {
            let reject = read(parser, hostile, Family::Any).unwrap_err();
            let text = reject.to_string();
            assert!(!text.contains('\u{1b}'), "{text:?}");
            assert!(!text.contains("secret-token"), "{text:?}");
        }
    }

    #[test]
    fn rejection_messages_are_readable() {
        for (reject, text) in [
            (Reject::Status(429), "HTTP 429"),
            (Reject::TooLarge, "answer larger than 4096 bytes"),
            (Reject::Empty, "empty answer"),
            (Reject::Html, "answer is a web page, not an address"),
            (Reject::NotAnAddress, "answer is not an IP address"),
            (Reject::NoTraceIp, "answer has no ip= line"),
            (
                Reject::DuplicateTraceIp,
                "answer has more than one ip= line",
            ),
            (Reject::MappedV4, "answer is an IPv4-mapped IPv6 address"),
            (
                Reject::WrongFamily {
                    got: IpFamily::V6,
                    wanted: IpFamily::V4,
                },
                "answer is an IPv6 address but IPv4 was requested",
            ),
            (
                Reject::NotGlobal(ip("10.0.0.1"), Scope::Private),
                "answer 10.0.0.1 is not a public address (private)",
            ),
        ] {
            assert_eq!(reject.to_string(), text);
        }
    }
}
