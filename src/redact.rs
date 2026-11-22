//! Keeps credentials out of anything `myip` prints to a terminal or a log.

/// Replaces the userinfo of every `scheme://user:password@host` in `text` with `***`.
///
/// The authority is taken to extend to the next whitespace, so a password that itself
/// contains `/`, `?` or a quote is still covered. This may over-redact an `@` in a
/// URL path, which is the safe direction.
pub fn redact_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
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

/// Replaces the value of every query parameter of every `scheme://...?query` in `text`
/// with `***`; a parameter without `=` is replaced whole.
///
/// Like `redact_userinfo`, the URL is taken to extend to the next whitespace, so a
/// closing quote after it is swallowed: over-redacting is the safe direction. A
/// `#fragment` ends the query and is kept. Text outside a `scheme://` URL is untouched.
pub fn redact_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let (url, after) = tail.split_at(end);
        match url.split_once('?') {
            Some((path, query_and_fragment)) => {
                let (query, fragment) = match query_and_fragment.find('#') {
                    Some(i) => query_and_fragment.split_at(i),
                    None => (query_and_fragment, ""),
                };
                out.push_str(path);
                out.push('?');
                let mut pairs = query.split('&').map(redact_pair);
                if let Some(first) = pairs.next() {
                    out.push_str(&first);
                    for pair in pairs {
                        out.push('&');
                        out.push_str(&pair);
                    }
                }
                out.push_str(fragment);
            }
            None => out.push_str(url),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn redact_pair(pair: &str) -> String {
    match pair.split_once('=') {
        Some((name, _)) => format!("{name}=***"),
        None if pair.is_empty() => String::new(),
        None => "***".to_owned(),
    }
}

/// Replaces the query of every `?query` that one of `args` carries, wherever `text`
/// quotes it, with the same masking as `redact_query`.
///
/// clap echoes a rejected or unexpected argument verbatim and without a `scheme://`,
/// so `redact_query` cannot see it. Longer queries go first so that one that is a prefix
/// of another cannot leave the tail of the longer one behind.
pub fn redact_arg_queries(text: &str, args: &[String]) -> String {
    let mut queries: Vec<&str> = args
        .iter()
        .filter_map(|arg| arg.split_once('?'))
        .map(|(_, query)| query)
        .filter(|query| !query.is_empty())
        .collect();
    queries.sort_by_key(|query| std::cmp::Reverse(query.len()));
    let mut out = text.to_owned();
    for query in queries {
        let masked = query
            .split('&')
            .map(redact_pair)
            .collect::<Vec<_>>()
            .join("&");
        out = out.replace(&format!("?{query}"), &format!("?{masked}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_replaced_and_the_rest_is_kept() {
        for (input, want) in [
            (
                "https://ipinfo.io/json?token=abc123",
                "https://ipinfo.io/json?token=***",
            ),
            ("https://h/x?a=1&b=2", "https://h/x?a=***&b=***"),
            ("https://h/x?abc", "https://h/x?***"),
            ("https://h/x?a=&b", "https://h/x?a=***&***"),
            ("https://h/x?a=1#frag", "https://h/x?a=***#frag"),
            ("GET https://h/x?a=1 failed", "GET https://h/x?a=*** failed"),
            (
                "a https://h/?t=1 b http://i/p?u=2&v=3 c",
                "a https://h/?t=*** b http://i/p?u=***&v=*** c",
            ),
            ("https://u:p@h/x?a=1", "https://u:p@h/x?a=***"),
        ] {
            assert_eq!(redact_query(input), want, "{input}");
        }
    }

    #[test]
    fn text_without_a_url_query_is_unchanged_by_redact_query() {
        for input in [
            "",
            "plain text",
            "https://ipinfo.io/ip",
            "https://h/x?",
            "https://h/x#frag",
            "what? yes",
            "no url ?t=1",
            "scheme:// then ?t=1",
            "://",
        ] {
            assert_eq!(redact_query(input), input, "{input}");
        }
    }

    #[test]
    fn a_query_an_argument_carries_is_masked_wherever_the_text_quotes_it() {
        fn args(list: &[&str]) -> Vec<String> {
            list.iter().map(|a| (*a).to_owned()).collect()
        }
        for (text, list, want) in [
            (
                "error: invalid value 'json?token=abc' for '[FIELD]'",
                &["json?token=abc"][..],
                "error: invalid value 'json?token=***' for '[FIELD]'",
            ),
            (
                "unexpected argument 'x?a=1&b=2' found",
                &["ip", "x?a=1&b=2"],
                "unexpected argument 'x?a=***&b=***' found",
            ),
            ("bad 'x?abc'", &["x?abc"], "bad 'x?***'"),
            ("bad 'x?abc\"d'", &["x?abc\"d"], "bad 'x?***'"),
            ("bad 'x?t=1#f'", &["x?t=1#f"], "bad 'x?t=***'"),
            (
                "'--timeout=x?t=abc' and 'a?t=abc'",
                &["--timeout=x?t=abc", "a?t=abc"],
                "'--timeout=x?t=***' and 'a?t=***'",
            ),
            // One query a prefix of another: the longer one is masked first.
            (
                "'a?t=ab' 'a?t=abc'",
                &["a?t=ab", "a?t=abc"],
                "'a?t=***' 'a?t=***'",
            ),
            (
                "'a?t=abc' 'a?t=ab'",
                &["a?t=ab", "a?t=abc"],
                "'a?t=***' 'a?t=***'",
            ),
        ] {
            let got = redact_arg_queries(text, &args(list));
            assert_eq!(got, want, "{text}");
        }
    }

    #[test]
    fn text_is_unchanged_when_no_argument_carries_a_query() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        for (text, list) in [
            ("usage: myip [OPTIONS] [FIELD]", &["--help"][..]),
            ("a ? b", &["ip", "city"]),
            ("'ip?' is fine", &["ip?"]),
            ("'?' is fine", &["?"]),
            ("", &[]),
        ] {
            assert_eq!(redact_arg_queries(text, &args(list)), text, "{text}");
        }
    }

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
