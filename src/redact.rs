//! Keeps credentials out of anything `myip` prints to a terminal or a log.

/// Replaces the userinfo of every `scheme://user:password@host` in `text` with `***`.
///
/// The authority is taken to extend to the next whitespace or quote, so a password
/// that itself contains `/` or `?` is still covered. This may over-redact an `@` in a
/// URL path, which is the safe direction.
pub fn redact_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '<' | '>'))
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        match authority.rfind('@') {
            Some(i) => {
                out.push_str("***");
                out.push_str(&authority[i..]);
            }
            None => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_is_replaced_and_the_rest_is_kept() {
        for (input, want) in [
            ("http://u:pw@h:3128", "http://***@h:3128"),
            ("http://u:pw@h:3128/x?y=1", "http://***@h:3128/x?y=1"),
            (
                "proxy 'http://u:pw@h:1' refused",
                "proxy 'http://***@h:1' refused",
            ),
            (
                "a http://u:p@h b https://x:y@i c",
                "a http://***@h b https://***@i c",
            ),
            ("http://user@h", "http://***@h"),
            ("http://u:p@ss@h", "http://***@h"),
            ("http://u:p/ss?w#d@h:1", "http://***@h:1"),
        ] {
            assert_eq!(redact_userinfo(input), want, "{input}");
        }
    }

    #[test]
    fn quotes_inside_the_password_do_not_end_the_authority() {
        for (input, want) in [
            (
                "proxy http://user:pa'ss@proxy.example:3128 refused",
                "proxy http://***@proxy.example:3128 refused",
            ),
            (
                "proxy http://user:pa\"ss@h:1 refused",
                "proxy http://***@h:1 refused",
            ),
            (
                "proxy http://user:pa`ss@h:1 refused",
                "proxy http://***@h:1 refused",
            ),
            (
                "proxy 'http://user:pa'ss@h:1' refused",
                "proxy 'http://***@h:1' refused",
            ),
        ] {
            let got = redact_userinfo(input);
            assert_eq!(got, want, "{input}");
            assert!(!got.contains("ss@"), "{got}");
        }
    }

    #[test]
    fn text_without_userinfo_is_unchanged() {
        for input in [
            "",
            "plain text",
            "https://ipinfo.io/ip",
            "http://127.0.0.1:3128",
            "request to ipinfo.io timed out after 10s",
            "scheme:// then nothing",
            "://",
        ] {
            assert_eq!(redact_userinfo(input), input, "{input}");
        }
    }
}
