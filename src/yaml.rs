//! The YAML rendering of a [`Json`] tree for `--format yaml` (ADR 0002).
//!
//! Block style, two-space indents, one document, no `---`. Every string is double-quoted
//! with the JSON escapes (plus `\uXXXX` for what YAML cannot carry raw), so no value can be
//! read as another type or as syntax, and no line break or control character is printed.
//! A bare word is only ever `null`, `true`, `false`, an integer, `[]` or `{}`; they mean the
//! same under YAML 1.1 and 1.2. Keys are bare only when they match `[a-z][a-z0-9_]*` and are
//! not a word YAML 1.1 reads as a boolean or null. A key too long for an implicit entry is
//! written in the explicit `? key` / `: value` form.
//!
//! On a terminal (ADR 0005) each key is one cyan span, quotes included and the colon outside,
//! and each `null` is dim; the layout does not change, so without the codes the document is
//! the plain one.

use crate::json::{Json, write_quoted};
use crate::style::Style;

/// `value` as one YAML document in block style, ending with a newline.
pub fn render(value: &Json) -> String {
    render_styled(value, Style::Plain)
}

/// [`render`] with each key cyan and each `null` dim when `style` is `Ansi`.
pub fn render_styled(value: &Json, style: Style) -> String {
    let mut out = String::new();
    match value {
        Json::Object(fields) if !fields.is_empty() => mapping(&mut out, fields, 0, false, style),
        Json::Array(items) if !items.is_empty() => sequence(&mut out, items, 0, false, style),
        scalar => {
            scalar_text(&mut out, scalar, style);
            out.push('\n');
        }
    }
    out
}

/// YAML reads a `key: value` entry only when the key is at most 1024 characters. A longer one
/// is written as `? key` and `: value`; this switches over well before that edge, so the
/// counting of quotes and of the colon cannot matter.
const MAX_IMPLICIT_KEY: usize = 1000;

/// One `key: value` entry per line at `indent`. With `inline_first` the caller has already
/// written the first line's indentation (a `- ` marker), so the first entry continues it.
fn mapping(
    out: &mut String,
    fields: &[(String, Json)],
    indent: usize,
    mut inline_first: bool,
    style: Style,
) {
    for (key, value) in fields {
        if !std::mem::take(&mut inline_first) {
            pad(out, indent);
        }
        let key = key_text(key);
        // The form follows the plain key, so the colour cannot change the layout.
        let explicit = key.chars().count() > MAX_IMPLICIT_KEY;
        if explicit {
            out.push_str("? ");
        }
        out.push_str(&style.key(&key));
        if explicit {
            out.push('\n');
            pad(out, indent);
        }
        out.push(':');
        match value {
            Json::Object(fields) if !fields.is_empty() => {
                out.push('\n');
                mapping(out, fields, indent + 2, false, style);
            }
            Json::Array(items) if !items.is_empty() => {
                out.push('\n');
                sequence(out, items, indent + 2, false, style);
            }
            scalar => {
                out.push(' ');
                scalar_text(out, scalar, style);
                out.push('\n');
            }
        }
    }
}

/// One `- item` entry per line at `indent`; see [`mapping`] for `inline_first`.
fn sequence(out: &mut String, items: &[Json], indent: usize, mut inline_first: bool, style: Style) {
    for item in items {
        if !std::mem::take(&mut inline_first) {
            pad(out, indent);
        }
        out.push_str("- ");
        match item {
            Json::Object(fields) if !fields.is_empty() => {
                mapping(out, fields, indent + 2, true, style);
            }
            Json::Array(items) if !items.is_empty() => {
                sequence(out, items, indent + 2, true, style);
            }
            scalar => {
                scalar_text(out, scalar, style);
                out.push('\n');
            }
        }
    }
}

fn pad(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

/// A scalar or an empty collection, on one line and without the newline.
fn scalar_text(out: &mut String, value: &Json, style: Style) {
    match value {
        Json::Null => out.push_str(&style.dim("null")),
        Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Json::Int(value) => out.push_str(&value.to_string()),
        Json::Str(value) => write_quoted(out, value, true),
        Json::Array(_) => out.push_str("[]"),
        Json::Object(_) => out.push_str("{}"),
    }
}

fn key_text(key: &str) -> String {
    if is_bare_key(key) {
        key.to_owned()
    } else {
        let mut quoted = String::new();
        write_quoted(&mut quoted, key, true);
        quoted
    }
}

fn is_bare_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !matches!(
            key,
            "y" | "n" | "yes" | "no" | "on" | "off" | "true" | "false" | "null"
        )
}

#[cfg(test)]
mod tests {
    use yaml_rust2::{Yaml, YamlLoader};

    use super::*;
    use crate::style::testing::{assert_balanced, assert_only_document_codes, strip};

    fn tree(yaml: &Yaml) -> Json {
        match yaml {
            Yaml::Null => Json::Null,
            Yaml::Boolean(value) => Json::Bool(*value),
            Yaml::Integer(value) => Json::Int(*value),
            Yaml::String(value) => Json::Str(value.clone()),
            Yaml::Array(items) => Json::Array(items.iter().map(tree).collect()),
            Yaml::Hash(map) => Json::Object(
                map.iter()
                    .map(|(key, value)| match key {
                        Yaml::String(key) => (key.clone(), tree(value)),
                        other => panic!("key {other:?} is not a string"),
                    })
                    .collect(),
            ),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// What a real YAML parser reads from `text`, which must be exactly one document.
    fn parsed(text: &str) -> Json {
        let documents =
            YamlLoader::load_from_str(text).unwrap_or_else(|error| panic!("{error}\n---\n{text}"));
        assert_eq!(documents.len(), 1, "{text}");
        tree(&documents[0])
    }

    fn doc() -> Json {
        Json::object([
            ("schema_version", 1u32.into()),
            ("name", "eth0".into()),
            ("virtual", true.into()),
            ("missing", Json::Null),
            ("empty_list", Json::Array(vec![])),
            ("empty_object", Json::object(Vec::<(&str, Json)>::new())),
            ("tags", Json::Array(vec!["a".into(), "b".into()])),
            (
                "nested",
                Json::object([
                    (
                        "list",
                        Json::Array(vec![
                            Json::object([("k", 2u32.into()), ("j", "x".into())]),
                            Json::Array(vec![]),
                            Json::object(Vec::<(&str, Json)>::new()),
                            Json::Array(vec![1u32.into(), Json::Array(vec![2u32.into()])]),
                            Json::object([("items", Json::Array(vec!["p".into(), "q".into()]))]),
                        ]),
                    ),
                    ("no", Json::Int(-3)),
                ]),
            ),
        ])
    }

    #[test]
    fn renders_block_style_with_two_space_indents() {
        let want = "\
schema_version: 1
name: \"eth0\"
virtual: true
missing: null
empty_list: []
empty_object: {}
tags:
  - \"a\"
  - \"b\"
nested:
  list:
    - k: 2
      j: \"x\"
    - []
    - {}
    - - 1
      - - 2
    - items:
        - \"p\"
        - \"q\"
  \"no\": -3
";
        assert_eq!(render(&doc()), want);
    }

    #[test]
    fn every_rendering_reads_back_as_the_tree_it_came_from() {
        let tree = doc();
        assert_eq!(parsed(&render(&tree)), tree);
        assert_eq!(parsed(&tree.render(true)), tree);
        assert_eq!(parsed(&tree.render(false)), tree);
    }

    #[test]
    fn scalars_and_empty_collections_are_documents_too() {
        let cases = [
            (Json::Null, "null\n"),
            (Json::Bool(true), "true\n"),
            (Json::Int(i64::MIN), "-9223372036854775808\n"),
            (Json::from("a: b"), "\"a: b\"\n"),
            (Json::Array(vec![]), "[]\n"),
            (Json::object(Vec::<(&str, Json)>::new()), "{}\n"),
        ];
        for (value, want) in cases {
            assert_eq!(render(&value), want);
            assert_eq!(parsed(want), value);
        }
    }

    /// Strings a YAML emitter must not get wrong: anything a parser would read as another
    /// type, as syntax, as a line break or as a directive.
    const AWKWARD: &[&str] = &[
        "",
        " ",
        " leading and trailing ",
        "no",
        "yes",
        "on",
        "off",
        "y",
        "n",
        "null",
        "~",
        "true",
        "1e3",
        "0x1f",
        "1_000",
        "007",
        ".inf",
        "-",
        "- item",
        "---",
        "...",
        "# not a comment",
        "key: value",
        "a: b # c",
        "*alias",
        "&anchor",
        "!tag",
        "|",
        ">",
        "%YAML 1.2",
        "[flow]",
        "{flow}",
        ": ",
        "\"quoted\"",
        "it's",
        "back\\slash \\n",
        "tab\there",
        "line\nbreak\r\nand\rmore",
        "h\u{e9}llo \u{4e16}\u{754c} \u{1f980}",
        "</script>",
        "\u{1b}[2J\u{7}\0\u{7f}",
        "\u{85}\u{2028}\u{2029}\u{feff}\u{fffe}\u{ffff}\u{202e}\u{200b}",
    ];

    #[test]
    fn awkward_strings_survive_as_values_and_as_keys() {
        for &raw in AWKWARD {
            let tree = Json::object([
                ("value", raw.into()),
                (raw, "key".into()),
                ("list", Json::Array(vec![raw.into()])),
            ]);
            let out = render(&tree);
            assert_eq!(parsed(&out), tree, "{raw:?}\n{out}");
        }
    }

    #[test]
    fn a_long_string_stays_on_one_line_and_survives() {
        let raw = format!("{}\u{e9}{}", "word ".repeat(4000), "\u{1f980}".repeat(500));
        let tree = Json::object([("long", raw.into())]);
        let out = render(&tree);
        assert_eq!(out.lines().count(), 1);
        assert_eq!(parsed(&out), tree);
    }

    #[test]
    fn a_key_too_long_for_an_implicit_key_is_still_read_back() {
        // YAML allows an implicit `key: value` entry only up to 1024 characters of key.
        let keys = [
            "a".repeat(1000),
            "a".repeat(1001),
            "a".repeat(1023),
            "a".repeat(1024),
            "a".repeat(1025),
            "a".repeat(5000),
            "\u{e9}".repeat(1500),
            "\n".repeat(600),
            "\"".repeat(600),
            "word ".repeat(400),
        ];
        let values = [
            Json::Null,
            "text".into(),
            7u32.into(),
            Json::Array(vec![]),
            Json::object(Vec::<(&str, Json)>::new()),
            Json::Array(vec!["x".into(), Json::Array(vec![1u32.into()])]),
            Json::object([("inner", "y".into()), ("n", Json::Null)]),
        ];
        for key in &keys {
            for value in &values {
                let entry = Json::object([(key.clone(), value.clone())]);
                let both = Json::object([
                    ("first", 1u32.into()),
                    (key.as_str(), value.clone()),
                    ("last", 2u32.into()),
                ]);
                for tree in [
                    entry.clone(),
                    both.clone(),
                    Json::Array(vec![entry.clone(), both.clone(), Json::Null]),
                    Json::object([("outer", Json::Array(vec![entry, both]))]),
                ] {
                    let out = render(&tree);
                    assert_eq!(parsed(&out), tree, "key of {} chars in\n{out}", key.len());
                    assert!(out.ends_with('\n') && !out.contains(" \n"), "{out:?}");
                }
            }
        }
    }

    #[test]
    fn nothing_a_terminal_or_a_reader_acts_on_is_printed_raw() {
        let tree = Json::Array(AWKWARD.iter().map(|raw| Json::from(*raw)).collect());
        let out = render(&tree);
        assert_eq!(out.lines().count(), AWKWARD.len());
        for c in out.chars().filter(|c| *c != '\n') {
            assert!(
                !(c.is_control()
                    || crate::sanitize::is_unsafe(c)
                    || matches!(c, '\u{fffe}' | '\u{ffff}')),
                "{c:?} was printed raw in\n{out}"
            );
        }
    }

    #[test]
    fn keys_are_bare_only_when_every_yaml_version_reads_them_as_text() {
        let keys = [
            ("schema_version", "schema_version"),
            ("a1_b2", "a1_b2"),
            ("y", "\"y\""),
            ("n", "\"n\""),
            ("yes", "\"yes\""),
            ("no", "\"no\""),
            ("on", "\"on\""),
            ("off", "\"off\""),
            ("true", "\"true\""),
            ("false", "\"false\""),
            ("null", "\"null\""),
            ("Yes", "\"Yes\""),
            ("1st", "\"1st\""),
            ("_a", "\"_a\""),
            ("a-b", "\"a-b\""),
            ("a b", "\"a b\""),
            ("a:b", "\"a:b\""),
            ("", "\"\""),
            ("\u{e9}", "\"\u{e9}\""),
            ("a\nb", "\"a\\nb\""),
        ];
        for (key, written) in keys {
            let out = render(&Json::object([(key, Json::Null)]));
            assert_eq!(out, format!("{written}: null\n"), "{key:?}");
        }
    }

    /// Splits `out` into its scalars and checks each is one a YAML 1.1 and a YAML 1.2
    /// parser read as the same thing: quoted text, `null`, `true`, `false` or an integer.
    fn unambiguous(out: &str) {
        for line in out.lines() {
            let mut rest = line.trim_start().trim_start_matches("- ");
            while let Some(after) = rest.strip_prefix("- ") {
                rest = after;
            }
            if let Some(quoted) = rest.strip_prefix('"') {
                let mut escaped = false;
                let end = quoted
                    .char_indices()
                    .find(|(_, c)| match (escaped, c) {
                        (true, _) => {
                            escaped = false;
                            false
                        }
                        (false, '\\') => {
                            escaped = true;
                            false
                        }
                        (false, '"') => true,
                        _ => false,
                    })
                    .map(|(at, _)| at)
                    .unwrap_or_else(|| panic!("unterminated string in {line:?}"));
                rest = &quoted[end + 1..];
                if rest.is_empty() {
                    continue;
                }
                rest = rest
                    .strip_prefix(':')
                    .unwrap_or_else(|| panic!("junk after a string in {line:?}"));
            } else if let Some((key, after)) = rest.split_once(':') {
                assert!(
                    key.starts_with(|c: char| c.is_ascii_lowercase())
                        && key
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                        && !["y", "n", "yes", "no", "on", "off", "true", "false", "null"]
                            .contains(&key),
                    "bare key {key:?} in {line:?}"
                );
                rest = after;
            }
            let value = rest.trim_start();
            let digits = value.strip_prefix('-').unwrap_or(value);
            let ok = value.is_empty()
                || matches!(value, "null" | "true" | "false" | "[]" | "{}")
                || value.starts_with('"')
                || (!digits.is_empty()
                    && digits.chars().all(|c| c.is_ascii_digit())
                    && (digits == "0" || !digits.starts_with('0')));
            assert!(ok, "ambiguous scalar {value:?} in {line:?}");
        }
    }

    #[test]
    fn every_bare_word_means_the_same_in_yaml_1_1_and_1_2() {
        let mut fields = vec![
            ("null".to_owned(), Json::Null),
            ("yes".to_owned(), Json::Bool(true)),
            ("zero".to_owned(), Json::Int(0)),
            ("negative".to_owned(), Json::Int(-12)),
            ("max".to_owned(), Json::Int(i64::MAX)),
        ];
        fields.extend(AWKWARD.iter().map(|raw| ("s".to_owned(), Json::from(*raw))));
        let out = render(&doc());
        unambiguous(&out);
        let out = render(&Json::Object(fields));
        unambiguous(&out);
    }

    #[test]
    fn a_document_is_one_block_without_markers_tabs_or_trailing_spaces() {
        let out = render(&doc());
        assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
        assert!(!out.starts_with("---") && !out.contains("\n..."));
        assert!(!out.contains('\t'));
        assert!(out.lines().all(|line| line == line.trim_end()));
    }

    const CYAN: &str = "\u{1b}[36m";
    const DIM: &str = "\u{1b}[2m";
    const RESET: &str = "\u{1b}[0m";

    /// A key token as the styled document writes it, spelled out here so the expectations do
    /// not use the code under test.
    fn key(token: &str) -> String {
        format!("{CYAN}{token}{RESET}")
    }

    fn null() -> String {
        format!("{DIM}null{RESET}")
    }

    #[test]
    fn a_styled_document_wraps_every_key_and_every_null_and_nothing_else() {
        let want = [
            format!("{}: 1", key("schema_version")),
            format!("{}: \"eth0\"", key("name")),
            format!("{}: true", key("virtual")),
            format!("{}: {}", key("missing"), null()),
            format!("{}: []", key("empty_list")),
            format!("{}: {{}}", key("empty_object")),
            format!("{}:", key("tags")),
            "  - \"a\"".to_owned(),
            "  - \"b\"".to_owned(),
            format!("{}:", key("nested")),
            format!("  {}:", key("list")),
            format!("    - {}: 2", key("k")),
            format!("      {}: \"x\"", key("j")),
            "    - []".to_owned(),
            "    - {}".to_owned(),
            "    - - 1".to_owned(),
            "      - - 2".to_owned(),
            format!("    - {}:", key("items")),
            "        - \"p\"".to_owned(),
            "        - \"q\"".to_owned(),
            format!("  {}: -3", key("\"no\"")),
        ]
        .join("\n")
            + "\n";
        assert_eq!(render_styled(&doc(), Style::Ansi), want);
        assert_eq!(render_styled(&doc(), Style::Plain), render(&doc()));
    }

    #[test]
    fn a_quoted_key_is_one_span_with_its_quotes_and_the_colon_stays_outside() {
        let tree = Json::object([
            ("no", Json::Null),
            ("a b", "null".into()),
            ("", 1u32.into()),
            ("Yes", Json::Array(vec![Json::Null, "null".into()])),
        ]);
        let want = format!(
            "{}: {}\n{}: \"null\"\n{}: 1\n{}:\n  - {}\n  - \"null\"\n",
            key("\"no\""),
            null(),
            key("\"a b\""),
            key("\"\""),
            key("\"Yes\""),
            null(),
        );
        assert_eq!(render_styled(&tree, Style::Ansi), want);
    }

    #[test]
    fn a_long_key_in_the_explicit_form_is_one_span_and_the_colon_line_is_plain() {
        let quoted = "\u{e9}".repeat(1500);
        let bare = "a".repeat(1001);
        for (raw, token) in [
            (quoted.clone(), format!("\"{quoted}\"")),
            (bare.clone(), bare.clone()),
        ] {
            assert_eq!(
                render(&Json::object([(raw.as_str(), Json::Null)])),
                format!("? {token}\n: null\n")
            );
            let tree = Json::object([
                ("first", Json::Null),
                (raw.as_str(), Json::Null),
                ("last", "v".into()),
            ]);
            let want = format!(
                "{}: {}\n? {}\n: {}\n{}: \"v\"\n",
                key("first"),
                null(),
                key(&token),
                null(),
                key("last"),
            );
            assert_eq!(render_styled(&tree, Style::Ansi), want);
            let nested = Json::Array(vec![Json::object([(raw.as_str(), "v".into())])]);
            assert_eq!(
                render_styled(&nested, Style::Ansi),
                format!("- ? {}\n  : \"v\"\n", key(&token))
            );
        }
    }

    #[test]
    fn a_scalar_document_styles_only_a_null() {
        assert_eq!(
            render_styled(&Json::Null, Style::Ansi),
            format!("{}\n", null())
        );
        for plain in [Json::from("null"), Json::Int(0), Json::Array(vec![])] {
            assert_eq!(render_styled(&plain, Style::Ansi), render(&plain));
        }
    }

    #[test]
    fn a_hostile_key_or_value_adds_no_escape_byte_to_a_styled_document() {
        let hostile = "a\u{1b}[31m\n\u{1b}[2J\u{1b}]0;x\u{7}b";
        let tree = Json::object([(hostile, hostile.into()), ("n", Json::Null)]);
        let out = render_styled(&tree, Style::Ansi);
        assert!(out.contains("a\\u001b[31m\\n\\u001b[2J"), "{out:?}");
        assert_only_document_codes(&out);
        assert_balanced(&out);
        // Two keys and one null, each with an opening code and a reset.
        assert_eq!(out.matches('\u{1b}').count(), 6, "{out:?}");
        assert_eq!(strip(&out), render(&tree));
    }

    /// A small deterministic generator, so a failure repeats and no dependency is needed.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            usize::try_from(self.0 % n as u64).expect("fits")
        }
    }

    fn generated_key(rng: &mut Rng, taken: &[(String, Json)]) -> String {
        loop {
            let key = match rng.below(6) {
                0 => AWKWARD[rng.below(AWKWARD.len())].to_owned(),
                1 => "a".repeat(990 + rng.below(30)),
                2 => "\u{e9}\"".repeat(495 + rng.below(30)),
                3 => format!("k{}", rng.below(50)),
                4 => ["y", "n", "no", "null", "Yes", "a b", "x:y"][rng.below(7)].to_owned(),
                _ => format!("{}_{}", ["id", "name", "ip"][rng.below(3)], rng.below(4)),
            };
            if taken.iter().all(|(name, _)| *name != key) {
                return key;
            }
        }
    }

    fn generated_tree(rng: &mut Rng, depth: usize) -> Json {
        let kinds = if depth == 0 { 5 } else { 7 };
        match rng.below(kinds) {
            0 => Json::Null,
            1 => Json::Bool(rng.below(2) == 0),
            2 => Json::Int(i64::try_from(rng.below(2000)).expect("fits") - 1000),
            3 => Json::from(AWKWARD[rng.below(AWKWARD.len())]),
            4 => [
                Json::Array(vec![]),
                Json::object(Vec::<(&str, Json)>::new()),
            ][rng.below(2)]
            .clone(),
            5 => Json::Array(
                (0..rng.below(4))
                    .map(|_| generated_tree(rng, depth - 1))
                    .collect(),
            ),
            _ => {
                let mut fields = Vec::new();
                for _ in 0..1 + rng.below(4) {
                    let key = generated_key(rng, &fields);
                    fields.push((key, generated_tree(rng, depth - 1)));
                }
                Json::Object(fields)
            }
        }
    }

    fn count(tree: &Json) -> (usize, usize) {
        match tree {
            Json::Null => (0, 1),
            Json::Array(items) => items.iter().map(count).fold((0, 0), add),
            Json::Object(fields) => fields
                .iter()
                .map(|(_, value)| add((1, 0), count(value)))
                .fold((0, 0), add),
            _ => (0, 0),
        }
    }

    fn add(a: (usize, usize), b: (usize, usize)) -> (usize, usize) {
        (a.0 + b.0, a.1 + b.1)
    }

    #[test]
    fn stripping_the_codes_of_a_styled_document_gives_the_plain_one_for_any_tree() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut trees = vec![doc()];
        trees.extend((0..400).map(|_| generated_tree(&mut rng, 4)));
        for tree in trees {
            let plain = render(&tree);
            let styled = render_styled(&tree, Style::Ansi);
            let (keys, nulls) = count(&tree);
            assert_eq!(strip(&styled), plain, "{tree:?}");
            assert_eq!(parsed(&strip(&styled)), tree);
            assert_only_document_codes(&styled);
            assert_balanced(&styled);
            assert_eq!(styled.matches(CYAN).count(), keys, "{styled}");
            assert_eq!(styled.matches(DIM).count(), nulls, "{styled}");
            assert_eq!(styled.matches(RESET).count(), keys + nulls, "{styled}");
            assert_eq!(render_styled(&tree, Style::Plain), plain);
        }
    }
}
