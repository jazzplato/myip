//! Keeps credentials out of anything `myip` prints to a terminal or a log.
//!
//! Every redaction finds the byte ranges of `text` to hide and replaces them with `***` in
//! one go (`mask`), so that several redactions can be combined on the text as it was.

use std::ops::Range;

/// Replaces the userinfo of every `scheme://user:password@host` in `text` with `***`.
///
/// The authority is taken to extend to the next whitespace, so a password that itself
/// contains `/`, `?` or a quote is still covered. This may over-redact an `@` in a
/// URL path, which is the safe direction.
pub fn redact_userinfo(text: &str) -> String {
    mask(text, userinfo_spans(text))
}

fn userinfo_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("://") {
        let authority_start = from + at + 3;
        let tail = &text[authority_start..];
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        if let Some(i) = tail[..end].rfind('@') {
            spans.push(authority_start..authority_start + i);
        }
        from = authority_start + end;
    }
    spans
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
    mask(text, query_spans(text))
}

fn query_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("://") {
        let url_start = from + at + 3;
        let tail = &text[url_start..];
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let url = &tail[..end];
        let resume = match url.split_once('?') {
            Some((path, query_and_fragment)) => {
                let query_end = query_and_fragment
                    .find('#')
                    .unwrap_or(query_and_fragment.len());
                let mut pair_start = url_start + path.len() + 1;
                for pair in query_and_fragment[..query_end].split('&') {
                    // `name=value` hides the value, even an empty one; a bare `token` is hidden whole.
                    match pair.find('=') {
                        Some(i) => spans.push(pair_start + i + 1..pair_start + pair.len()),
                        None if pair.is_empty() => {}
                        None => spans.push(pair_start..pair_start + pair.len()),
                    }
                    pair_start += pair.len() + 1;
                }
                end - (query_and_fragment.len() - query_end)
            }
            None => end,
        };
        from = url_start + resume;
    }
    spans
}

/// Replaces the userinfo and the query values of every URL in `text`, as `redact_userinfo`
/// and then `redact_query` do.
pub fn redact_credentials(text: &str) -> String {
    mask(text, credential_spans(text))
}

/// The ranges that `redact_userinfo` and then `redact_query` hide, all in the coordinates
/// of `text`. The query pass of the second one looks at the text with the userinfo already
/// hidden, which has the same shape as the text with it overwritten by letters of the same
/// length, and that one has the coordinates of `text`.
fn credential_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = userinfo_spans(text);
    let mut hidden = text.to_owned();
    for span in &spans {
        hidden.replace_range(span.clone(), &"x".repeat(span.len()));
    }
    spans.extend(query_spans(&hidden));
    spans
}

/// `text` with every range of `spans` replaced by `***`. Ranges that overlap or touch are
/// one mask, so a mask never grows with the number of ranges that cover the same text.
fn mask(text: &str, mut spans: Vec<Range<usize>>) -> String {
    spans.sort_unstable_by_key(|span| (span.start, span.end));
    let mut out = String::with_capacity(text.len());
    let mut done = 0;
    let mut spans = spans.into_iter().peekable();
    while let Some(Range { start, mut end }) = spans.next() {
        while let Some(next) = spans.next_if(|next| next.start <= end) {
            end = end.max(next.end);
        }
        out.push_str(&text[done..start]);
        out.push_str("***");
        done = end;
    }
    out.push_str(&text[done..]);
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

/// Hides every credential in `text` and every query value that one of `args` carries.
///
/// A server may echo a value back in an error body (`token SECRET rejected`), with no `?`
/// or `scheme://` around it, so `redact_arg_queries` and `redact_query` cannot see it. The
/// values are the part after the first `=` of each `&`-separated pair of a `?query` (the
/// whole pair when it has no `=`), up to a `#`. An argument is trimmed first, as FIELD is
/// before it is sent. A percent-decoded form is masked as well, since the server sees that
/// one. Like the other redactions, no value is too short to be masked; an empty one is
/// skipped.
///
/// The credentials are those of `redact_credentials`. Both kinds of range are found in
/// `text` as it is and masked together, so a value that holds a `://` or a `?` is still
/// found after its credentials are hidden. Places that overlap or touch, including a value
/// that holds another, two that overlap, or one that overlaps itself, are masked once, so no
/// tail is left behind and a mask is never searched for a value again.
pub fn redact_arg_values(text: &str, args: &[String]) -> String {
    let mut spans = credential_spans(text);
    spans.extend(value_spans(text, args));
    mask(text, spans)
}

fn value_spans(text: &str, args: &[String]) -> Vec<Range<usize>> {
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
    // One flag per byte keeps the memory at the size of `text`, however many places the
    // values match: a range per match would grow with the number of values times the text.
    let mut covered = vec![false; text.len()];
    for value in &values {
        mark_occurrences(text.as_bytes(), value.as_bytes(), &mut covered);
    }
    let mut spans = Vec::new();
    let mut at = 0;
    while let Some(start) = covered[at..].iter().position(|&flag| flag) {
        let start = at + start;
        let end = covered[start..]
            .iter()
            .position(|&flag| !flag)
            .map_or(covered.len(), |len| start + len);
        spans.push(start..end);
        at = end;
    }
    spans
}

/// Flags every byte of `text` that lies in an occurrence of `value`, an occurrence that
/// overlaps the previous one included (`str::match_indices` would skip it, and leave the
/// tail of the value behind). Knuth-Morris-Pratt, so the time is linear in `text` however
/// repetitive both are. Both are UTF-8, so a match always starts on a character boundary.
fn mark_occurrences(text: &[u8], value: &[u8], covered: &mut [bool]) {
    // `fallback[i]` is the length of the longest proper prefix of `value[..=i]` that is
    // also a suffix of it.
    let mut fallback = vec![0; value.len()];
    let mut matched = 0;
    for (i, &byte) in value.iter().enumerate().skip(1) {
        while matched > 0 && byte != value[matched] {
            matched = fallback[matched - 1];
        }
        if byte == value[matched] {
            matched += 1;
        }
        fallback[i] = matched;
    }
    let (mut matched, mut marked) = (0, 0);
    for (i, &byte) in text.iter().enumerate() {
        while matched > 0 && byte != value[matched] {
            matched = fallback[matched - 1];
        }
        if byte == value[matched] {
            matched += 1;
        }
        if matched == value.len() {
            let end = i + 1;
            covered[(end - matched).max(marked)..end].fill(true);
            marked = end;
            matched = fallback[matched - 1];
        }
    }
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
    fn a_value_that_overlaps_itself_is_masked_as_one_run() {
        let args = ["ip?t=ABABABABA".to_owned()];
        assert_eq!(redact_arg_values("ABABABABABABABABA", &args), "***");
        assert_eq!(redact_arg_values("x ABABABABABA y", &args), "x *** y");
    }

    #[test]
    fn many_overlapping_values_in_a_long_text_do_not_store_a_range_per_match() {
        // 64 values `a`, `aa`, ... match at nearly every offset of a body of `a`s; the ranges
        // that are kept must be the one run they make, not one per match.
        let body = "a".repeat(64 * 1024);
        let query = (1..=64)
            .map(|n| format!("k{n}={}", "a".repeat(n)))
            .collect::<Vec<_>>()
            .join("&");
        let args = [format!("ip?{query}")];
        let spans = value_spans(&body, &args);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0], 0..body.len());
        assert_eq!(redact_arg_values(&body, &args), "***");
    }

    #[test]
    fn every_occurrence_is_found_as_a_plain_search_at_each_offset_finds_it() {
        let words = |max: usize| {
            (1..=max).flat_map(|len| {
                (0..1_usize << len).map(move |bits| {
                    (0..len)
                        .map(|i| if bits >> i & 1 == 0 { 'a' } else { 'b' })
                        .collect::<String>()
                })
            })
        };
        for text in words(9) {
            for value in words(4) {
                let mut want = vec![false; text.len()];
                for start in 0..text.len() {
                    if text[start..].starts_with(&value) {
                        want[start..start + value.len()].fill(true);
                    }
                }
                let mut got = vec![false; text.len()];
                mark_occurrences(text.as_bytes(), value.as_bytes(), &mut got);
                assert_eq!(got, want, "{value} in {text}");
            }
        }
        let args = ["ip?k=éé".to_owned()];
        assert_eq!(redact_arg_values("xéééy", &args), "x***y");
    }

    #[test]
    fn two_values_that_overlap_in_the_text_are_one_mask() {
        let args = ["ip?a=abc&b=bcd".to_owned()];
        assert_eq!(redact_arg_values("x abcd y", &args), "x *** y");
    }

    #[test]
    fn credentials_are_hidden_in_one_pass_as_the_two_redactions_hide_them_in_turn() {
        for text in [
            "http://u:p?w#d@h:1/x?t=1",
            "http://u:p@h/x?t=1&u=2#f http://a?b=c",
            "x://@h?k=v y://a:b@c?d",
            r#"{"a":"https://u:p@h/?t=1#https://v:q@i?k"}"#,
            "http://?=&&a=&b#c://x@y?z",
            "no url here, 5 @ 3?",
            "héllo://ü:ß@é?ñ=ö#ü",
        ] {
            assert_eq!(
                redact_credentials(text),
                redact_query(&redact_userinfo(text)),
                "{text}"
            );
        }
    }

    #[test]
    fn a_value_that_holds_a_url_is_masked_with_the_credentials_it_overlaps() {
        let args = ["json?token=SECRET123:%2F%2Fhost%3Fx%3Dy".to_owned()];
        assert_eq!(
            redact_arg_values("token SECRET123://host?x=y rejected", &args),
            "token *** rejected"
        );
        // A value that is only a colon is masked everywhere, and the credentials that its
        // `://` finds are still hidden.
        let args = ["ip?c=:".to_owned()];
        assert_eq!(
            redact_arg_values("http://u:pw@h/x?k=v", &args),
            "http***//***@h/x?k=***"
        );
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
