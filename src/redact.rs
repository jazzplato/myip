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
/// `#fragment` ends the query and is kept, but it is scanned again because compact
/// JSON can put another URL behind it in the same word. Text outside a `scheme://` URL
/// is untouched.
pub fn redact_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let url = &tail[..end];
        let resume = match url.split_once('?') {
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
                end - fragment.len()
            }
            None => {
                out.push_str(url);
                end
            }
        };
        rest = &tail[resume..];
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

/// Replaces, wherever `text` has it, every query value that one of `args` carries with `***`.
///
/// A server may echo a value back in an error body (`token SECRET rejected`), with no `?`
/// or `scheme://` around it, so `redact_arg_queries` and `redact_query` cannot see it. The
/// values are the part after the first `=` of each `&`-separated pair of a `?query` (the
/// whole pair when it has no `=`), up to a `#`. An argument is trimmed first, as FIELD is
/// before it is sent. A percent-decoded form is masked as well, since the server sees that
/// one. Like the other redactions, no value is too short to be masked; an empty one is
/// skipped. Every place where a value occurs is found in `text` as it is, and places that
/// overlap or touch are masked once, so a value that holds another, or two that overlap,
/// leave no tail behind, and a mask is never searched for a value again.
pub fn redact_arg_values(text: &str, args: &[String]) -> String {
    let mut values: Vec<String> = Vec::new();
    for (_, query) in args.iter().filter_map(|arg| arg.trim().split_once('?')) {
        let query = query.split_once('#').map_or(query, |(query, _)| query);
        for pair in query.split('&') {
            let value = pair.split_once('=').map_or(pair, |(_, value)| value);
            values.extend(percent_decode(value));
            values.push(value.to_owned());
        }
    }
    values.retain(|value| !value.is_empty());
    values.sort();
    values.dedup();
    let mut spans: Vec<(usize, usize)> = values
        .iter()
        .flat_map(|value| text.match_indices(value.as_str()))
        .map(|(start, found)| (start, start + found.len()))
        .collect();
    spans.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut done = 0;
    let mut spans = spans.into_iter().peekable();
    while let Some((start, mut end)) = spans.next() {
        while let Some(&(next_start, next_end)) = spans.peek() {
            if next_start > end {
                break;
            }
            end = end.max(next_end);
            spans.next();
        }
        out.push_str(&text[done..start]);
        out.push_str("***");
        done = end;
    }
    out.push_str(&text[done..]);
    out
}

/// The `%XX`-decoded form of `value`, if it has one that is text.
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = match bytes.get(i..i + 3) {
            Some([b'%', high, low]) => {
                let digit = |b: &u8| char::from(*b).to_digit(16);
                digit(high).zip(digit(low)).map(|(h, l)| (h * 16 + l) as u8)
            }
            _ => None,
        };
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                i += 3;
            }
            None => {
                decoded.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(decoded).ok().filter(|text| text != value)
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
    fn a_fragment_does_not_hide_a_later_url_in_the_same_word() {
        for (input, want) in [
            (
                r#"{"help":"https://ipinfo.io/?page=1#auth","url":"https://ipinfo.io/json?token=S3CR3T"}"#,
                r#"{"help":"https://ipinfo.io/?page=***#auth","url":"https://ipinfo.io/json?token=***"#,
            ),
            (
                "https://a/?x=1#f1,https://b/?y=2#f2,https://c/?z=S3CR3T",
                "https://a/?x=***#f1,https://b/?y=***#f2,https://c/?z=***",
            ),
        ] {
            let got = redact_query(input);
            assert_eq!(got, want, "{input}");
            assert!(!got.contains("S3CR3T"), "{got}");
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
    fn a_query_value_from_the_arguments_is_masked_wherever_the_text_has_it() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        for (text, list, want) in [
            (
                r#"{"error":"token SECRET123 rejected"}"#,
                &["json?token=SECRET123"][..],
                r#"{"error":"token *** rejected"}"#,
            ),
            // Every pair, a bare token, and every occurrence.
            (
                "aaaa1111 bbbb2222 CCCC3333 aaaa1111",
                &["ip?a=aaaa1111&b=bbbb2222", "?CCCC3333"],
                "*** *** *** ***",
            ),
            // A value that contains another: the longer is masked first.
            (
                "x ABCD1234 ABCD y",
                &["ip?a=ABCD&b=ABCD1234"],
                "x *** *** y",
            ),
            // The fragment is not part of the query, and a server may echo the decoded value.
            (
                "bad a/b/c/d and WXYZ1234",
                &["ip?k=a%2Fb%2Fc%2Fd&k2=WXYZ1234#frag"],
                "bad *** and ***",
            ),
            // Any argument that carries a query counts, not only FIELD.
            (
                "proxy rejected PXTOKEN9",
                &["--proxy=http://h:1/?t=PXTOKEN9", "ip"],
                "proxy rejected ***",
            ),
            // No value is too short to hide: the existing query redaction has no minimum.
            (
                r#"{"token":"abc"}"#,
                &["json?token=abc"],
                r#"{"token":"***"}"#,
            ),
            ("pin 12 or 1", &["ip?a=12&b=1"], "pin *** or ***"),
            // FIELD is trimmed before it is sent, so the value is the trimmed one.
            (
                r#"{"error":"SECRET123"}"#,
                &["json?token=SECRET123\n"],
                r#"{"error":"***"}"#,
            ),
            (
                r#"{"error":"SECRET123"}"#,
                &[" json?token=SECRET123 "],
                r#"{"error":"***"}"#,
            ),
        ] {
            assert_eq!(redact_arg_values(text, &args(list)), want, "{text}");
        }
    }

    #[test]
    fn a_repeated_value_is_not_applied_once_per_repetition() {
        // `*` is masked as `***`, which holds `*` again: a value repeated and interleaved with
        // another of the same length must not make the text grow with every repetition.
        let repeated = format!("ip?{}", ["a=%2A&b=x"; 10].join("&"));
        assert_eq!(redact_arg_values("*", &[repeated]), "***");
    }

    #[test]
    fn two_values_that_overlap_in_the_text_are_one_mask() {
        let args = ["ip?a=abc&b=bcd".to_owned()];
        assert_eq!(redact_arg_values("x abcd y", &args), "x *** y");
    }

    #[test]
    fn arguments_without_a_query_value_leave_the_text_alone() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        for (text, list) in [
            ("token SECRET123 rejected", &["json"][..]),
            ("token SECRET123 rejected", &["-v", "public", "SECRET123"]),
            ("token SECRET123 rejected", &["json?", "ip?=", "?&"]),
            ("token SECRET123 rejected", &[]),
            ("", &["json?token=SECRET123"]),
        ] {
            assert_eq!(redact_arg_values(text, &args(list)), text, "{text}");
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
