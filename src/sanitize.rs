//! The one place that decides which characters may not reach a terminal as themselves.
//!
//! Interface names, resolver files, container cgroups and environment variables are text
//! that other parties control. A terminal acts on escape sequences (ESC, the C1 controls),
//! and a reader is fooled by characters that reorder or hide text, so such characters are
//! never printed raw: text output and diagnostics show them as `\x1b`-style escapes, and
//! the JSON writer encodes them as `\uXXXX`.

/// True for characters a terminal or a reader may act on instead of showing: the C0 and C1
/// controls (including ESC, NUL and DEL), and the Unicode characters that reorder, hide or
/// split text (bidirectional and zero-width controls, line and paragraph separators, the
/// byte order mark).
pub fn is_unsafe(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{61c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
        )
}

/// `text` with every unsafe character, newlines included, replaced by a visible escape
/// such as `\x1b`, `\n` or `\u{202e}`.
pub fn escape(text: &str) -> String {
    escape_with(text, false)
}

/// Like [`escape`], but keeps the newlines: for a block of text whose own line structure
/// is the layout.
pub fn escape_keeping_newlines(text: &str) -> String {
    escape_with(text, true)
}

fn escape_with(text: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' if keep_newlines => out.push('\n'),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if is_unsafe(c) => {
                let code = u32::from(c);
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else {
                    out.push_str(&format!("\\u{{{code:x}}}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Test support for the rule that a text layout owns its newlines: free text placed into
/// it must be escaped per field, so that it can neither start lines of its own nor carry
/// a control character.
#[cfg(test)]
pub(crate) mod testing {
    use super::is_unsafe;

    /// Free text built to forge the layout around it: headings, result lines, line breaks
    /// of every kind and a terminal escape.
    pub const FORGERY: &str = "boom\n[highlights]\nforged\r\n[where]\nfoo\u{1b}[2J\u{85}\u{202e}";

    /// Renders once with a harmless value and once with [`FORGERY`] in the same place. Both
    /// texts must have the same lines, and the hostile one nothing unsafe but its newlines.
    pub fn assert_forgery_proof(what: &str, render: impl Fn(&str) -> String) {
        let plain = render("boom");
        let hostile = render(FORGERY);
        assert_ne!(
            plain, hostile,
            "{what}: the free text never reached the output"
        );
        assert_eq!(
            hostile.lines().count(),
            plain.lines().count(),
            "{what}: free text started lines of its own:\n{hostile}"
        );
        assert!(
            hostile.chars().all(|c| c == '\n' || !is_unsafe(c)),
            "{what}: an unescaped control character:\n{hostile:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_and_text_tricks_are_unsafe() {
        let unsafe_chars = [
            '\u{0}', '\u{7}', '\u{8}', '\t', '\n', '\r', '\u{1b}', '\u{1f}', '\u{7f}', '\u{80}',
            '\u{85}', '\u{9b}', '\u{9f}', '\u{61c}', '\u{200b}', '\u{200e}', '\u{200f}',
            '\u{2028}', '\u{2029}', '\u{202a}', '\u{202e}', '\u{2060}', '\u{2066}', '\u{2069}',
            '\u{206a}', '\u{206f}', '\u{feff}',
        ];
        for c in unsafe_chars {
            assert!(is_unsafe(c), "{c:?} should be unsafe");
        }
    }

    #[test]
    fn ordinary_text_is_safe() {
        let safe = [
            ' ',
            'a',
            'Z',
            '0',
            '-',
            '.',
            '/',
            '\\',
            '"',
            '~',
            '\u{a0}',
            '\u{a1}',
            '\u{e9}',
            '\u{ff}',
            '\u{4e16}',
            '\u{1f980}',
            '\u{2010}',
            '\u{2027}',
            '\u{202f}',
            '\u{205f}',
            '\u{fffd}',
        ];
        for c in safe {
            assert!(!is_unsafe(c), "{c:?} should be safe");
        }
    }

    #[test]
    fn unsafe_characters_become_visible_escapes() {
        let cases = [
            ("a\u{1b}[2Jb", "a\\x1b[2Jb"),
            ("nul\0here", "nul\\x00here"),
            ("tab\there", "tab\\there"),
            ("cr\rhere", "cr\\rhere"),
            ("lf\nhere", "lf\\nhere"),
            ("del\u{7f}", "del\\x7f"),
            ("c1\u{85}\u{9b}", "c1\\x85\\x9b"),
            ("flip\u{202e}ed", "flip\\u{202e}ed"),
            ("zero\u{200b}width", "zero\\u{200b}width"),
        ];
        for (raw, want) in cases {
            assert_eq!(escape(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn the_newline_variant_keeps_only_newlines() {
        assert_eq!(
            escape_keeping_newlines("a\nb\r\nc\u{1b}\t\n"),
            "a\nb\\r\nc\\x1b\\t\n"
        );
    }

    #[test]
    fn ordinary_text_is_left_alone_including_non_ascii() {
        let text = "eth0 h\u{e9}llo \u{4e16}\u{754c} \u{1f980} back\\slash \"quoted\"";
        assert_eq!(escape(text), text);
        assert_eq!(escape_keeping_newlines(text), text);
        assert_eq!(escape(""), "");
    }

    #[test]
    fn an_escaped_string_contains_nothing_unsafe() {
        let nasty: String = ('\u{0}'..='\u{2fff}').chain(['\u{feff}']).collect();
        let escaped = escape(&nasty);
        assert!(!escaped.chars().any(is_unsafe));
    }
}
