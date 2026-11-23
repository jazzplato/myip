//! The YAML rendering of a [`Json`] tree for `--format yaml` (ADR 0002).
//!
//! Block style, two-space indents, one document, no `---`. Every string is double-quoted
//! with the JSON escapes (plus `\uXXXX` for what YAML cannot carry raw), so no value can be
//! read as another type or as syntax, and no line break or control character is printed.
//! A bare word is only ever `null`, `true`, `false`, an integer, `[]` or `{}`; they mean the
//! same under YAML 1.1 and 1.2. Keys are bare only when they match `[a-z][a-z0-9_]*` and are
//! not a word YAML 1.1 reads as a boolean or null. A key too long for an implicit entry is
//! written in the explicit `? key` / `: value` form.

use crate::json::{Json, write_quoted};

/// `value` as one YAML document in block style, ending with a newline.
pub fn render(value: &Json) -> String {
    let mut out = String::new();
    match value {
        Json::Object(fields) if !fields.is_empty() => mapping(&mut out, fields, 0, false),
        Json::Array(items) if !items.is_empty() => sequence(&mut out, items, 0, false),
        scalar => {
            scalar_text(&mut out, scalar);
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
fn mapping(out: &mut String, fields: &[(String, Json)], indent: usize, mut inline_first: bool) {
    for (key, value) in fields {
        if !std::mem::take(&mut inline_first) {
            pad(out, indent);
        }
        let key = key_text(key);
        if key.chars().count() > MAX_IMPLICIT_KEY {
            out.push_str("? ");
            out.push_str(&key);
            out.push('\n');
            pad(out, indent);
        } else {
            out.push_str(&key);
        }
        out.push(':');
        match value {
            Json::Object(fields) if !fields.is_empty() => {
                out.push('\n');
                mapping(out, fields, indent + 2, false);
            }
            Json::Array(items) if !items.is_empty() => {
                out.push('\n');
                sequence(out, items, indent + 2, false);
            }
            scalar => {
                out.push(' ');
                scalar_text(out, scalar);
                out.push('\n');
            }
        }
    }
}

/// One `- item` entry per line at `indent`; see [`mapping`] for `inline_first`.
fn sequence(out: &mut String, items: &[Json], indent: usize, mut inline_first: bool) {
    for item in items {
        if !std::mem::take(&mut inline_first) {
            pad(out, indent);
        }
        out.push_str("- ");
        match item {
            Json::Object(fields) if !fields.is_empty() => mapping(out, fields, indent + 2, true),
            Json::Array(items) if !items.is_empty() => sequence(out, items, indent + 2, true),
            scalar => {
                scalar_text(out, scalar);
                out.push('\n');
            }
        }
    }
}

fn pad(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

/// A scalar or an empty collection, on one line and without the newline.
fn scalar_text(out: &mut String, value: &Json) {
    match value {
        Json::Null => out.push_str("null"),
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
}
