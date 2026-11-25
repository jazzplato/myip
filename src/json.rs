//! A tiny ordered JSON writer for the `--json` documents of the dimension subcommands.
//! Output only: nothing in `myip` parses JSON, so a full serializer would be dead weight.

use crate::sanitize;
use crate::style::Style;

/// A JSON value whose object keys keep insertion order, so `schema_version` stays first.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Builds an object from `(key, value)` pairs, keeping their order.
    pub fn object<K: Into<String>>(fields: impl IntoIterator<Item = (K, Json)>) -> Self {
        Self::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// `pretty` indents with two spaces and puts one element per line; otherwise the
    /// output is one line with no spaces. Neither form ends with a newline.
    pub fn render(&self, pretty: bool) -> String {
        self.render_styled(pretty, Style::Plain)
    }

    /// [`Json::render`] with each key cyan and each `null` dim when `style` is `Ansi`, for a
    /// pretty document on a terminal. Compact output is the machine line and is never styled.
    pub fn render_styled(&self, pretty: bool, style: Style) -> String {
        let style = if pretty { style } else { Style::Plain };
        let mut out = String::new();
        self.write(&mut out, pretty, 0, style);
        out
    }

    fn write(&self, out: &mut String, pretty: bool, depth: usize, style: Style) {
        match self {
            Self::Null => out.push_str(&style.dim("null")),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Int(value) => out.push_str(&value.to_string()),
            Self::Str(value) => write_quoted(out, value, false),
            Self::Array(items) => write_seq(out, pretty, depth, ('[', ']'), items, |out, item| {
                item.write(out, pretty, depth + 1, style);
            }),
            Self::Object(fields) => {
                write_seq(
                    out,
                    pretty,
                    depth,
                    ('{', '}'),
                    fields,
                    |out, (key, value)| {
                        let mut token = String::new();
                        write_quoted(&mut token, key, false);
                        out.push_str(&style.key(&token));
                        out.push(':');
                        if pretty {
                            out.push(' ');
                        }
                        value.write(out, pretty, depth + 1, style);
                    },
                );
            }
        }
    }
}

fn write_seq<T>(
    out: &mut String,
    pretty: bool,
    depth: usize,
    (open, close): (char, char),
    items: &[T],
    mut write_item: impl FnMut(&mut String, &T),
) {
    out.push(open);
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if pretty {
            newline(out, depth + 1);
        }
        write_item(out, item);
    }
    if pretty && !items.is_empty() {
        newline(out, depth);
    }
    out.push(close);
}

fn newline(out: &mut String, depth: usize) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// Writes `value` as a double-quoted string, escaping everything [`sanitize::is_unsafe`]
/// flags. With `yaml` it also escapes U+FFFE and U+FFFF: JSON may carry those raw, a YAML
/// stream may not. The escapes used are valid in both.
pub(crate) fn write_quoted(out: &mut String, value: &str, yaml: bool) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if sanitize::is_unsafe(c) || (yaml && matches!(c, '\u{fffe}' | '\u{ffff}')) => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<u32> for Json {
    fn from(value: u32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(value: Vec<T>) -> Self {
        Self::Array(value.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::testing::{assert_balanced, assert_only_document_codes, strip};

    fn doc() -> Json {
        Json::object([
            ("schema_version", 1u32.into()),
            ("name", "eth0".into()),
            ("virtual", true.into()),
            ("missing", Json::Null),
            ("empty_list", Vec::<String>::new().into()),
            ("empty_object", Json::object(Vec::<(&str, Json)>::new())),
            ("tags", vec!["a", "b"].into()),
            (
                "nested",
                Json::object([(
                    "list",
                    Json::Array(vec![Json::object([("k", 2u32.into())])]),
                )]),
            ),
        ])
    }

    #[test]
    fn renders_pretty_with_two_spaces_and_keeps_key_order() {
        let want = r#"{
  "schema_version": 1,
  "name": "eth0",
  "virtual": true,
  "missing": null,
  "empty_list": [],
  "empty_object": {},
  "tags": [
    "a",
    "b"
  ],
  "nested": {
    "list": [
      {
        "k": 2
      }
    ]
  }
}"#;
        assert_eq!(doc().render(true), want);
    }

    #[test]
    fn renders_compact_as_one_line_without_spaces_or_trailing_newline() {
        let want = r#"{"schema_version":1,"name":"eth0","virtual":true,"missing":null,"empty_list":[],"empty_object":{},"tags":["a","b"],"nested":{"list":[{"k":2}]}}"#;
        let out = doc().render(false);
        assert_eq!(out, want);
        assert!(!out.contains('\n') && !out.ends_with(' '));
    }

    #[test]
    fn scalars_render_bare_in_both_modes() {
        for pretty in [true, false] {
            assert_eq!(Json::Null.render(pretty), "null");
            assert_eq!(Json::Bool(false).render(pretty), "false");
            assert_eq!(Json::Bool(true).render(pretty), "true");
            assert_eq!(Json::Int(0).render(pretty), "0");
            assert_eq!(Json::Int(-7).render(pretty), "-7");
            assert_eq!(Json::Int(i64::MAX).render(pretty), "9223372036854775807");
            assert_eq!(Json::Int(i64::MIN).render(pretty), "-9223372036854775808");
        }
    }

    #[test]
    fn options_become_null_or_the_value() {
        assert_eq!(Json::from(None::<&str>), Json::Null);
        assert_eq!(Json::from(Some("x")), Json::Str("x".to_owned()));
        assert_eq!(Json::from(Some(7u32)), Json::Int(7));
    }

    #[test]
    fn strings_are_escaped_so_the_document_stays_one_valid_value() {
        let cases = [
            ("plain", r#""plain""#),
            ("say \"hi\"", r#""say \"hi\"""#),
            ("back\\slash", r#""back\\slash""#),
            ("line\nbreak\r\ttab", r#""line\nbreak\r\ttab""#),
            ("\u{8}\u{c}", r#""\b\f""#),
            ("\u{0}\u{1}\u{1f}", r#""\u0000\u0001\u001f""#),
            ("\u{7f}", r#""\u007f""#),
            ("\u{85}\u{9b}", r#""\u0085\u009b""#),
            ("\u{202e}\u{2028}\u{feff}", r#""\u202e\u2028\ufeff""#),
            (
                "h\u{e9}llo \u{4e16}\u{754c} \u{1f980}",
                "\"h\u{e9}llo \u{4e16}\u{754c} \u{1f980}\"",
            ),
            ("</script>", r#""</script>""#),
        ];
        for (raw, want) in cases {
            for pretty in [true, false] {
                assert_eq!(Json::from(raw).render(pretty), want, "{raw:?}");
            }
        }
    }

    #[test]
    fn object_keys_are_escaped_too() {
        let out = Json::object([("a\"b\n", Json::Null)]).render(false);
        assert_eq!(out, r#"{"a\"b\n":null}"#);
    }

    #[test]
    fn a_hostile_value_cannot_break_out_of_its_string() {
        let name = "eth0\",\"injected\":true,\"x\":\"";
        let object = Json::object([("name", name.into())]);
        assert_eq!(
            object.render(true),
            "{\n  \"name\": \"eth0\\\",\\\"injected\\\":true,\\\"x\\\":\\\"\"\n}"
        );
        assert_eq!(
            object.render(false),
            "{\"name\":\"eth0\\\",\\\"injected\\\":true,\\\"x\\\":\\\"\"}"
        );
    }

    #[test]
    fn an_array_of_objects_nests_in_both_modes() {
        let value = Json::Array(vec![Json::object([("a", 1u32.into())]), Json::Null]);
        assert_eq!(value.render(false), r#"[{"a":1},null]"#);
        assert_eq!(value.render(true), "[\n  {\n    \"a\": 1\n  },\n  null\n]");
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
        let long = "k".repeat(1500);
        let tree = Json::object([
            ("schema_version", 1u32.into()),
            ("name", "null".into()),
            ("missing", Json::Null),
            (
                "a \"key\"",
                Json::object([(
                    "list",
                    Json::Array(vec![
                        Json::object([("inner", Json::Null), ("text", "x".into())]),
                        Json::Null,
                    ]),
                )]),
            ),
            ("empty", Json::Array(vec![])),
            (long.as_str(), Json::Null),
        ]);
        let want = [
            "{".to_owned(),
            format!("  {}: 1,", key("\"schema_version\"")),
            format!("  {}: \"null\",", key("\"name\"")),
            format!("  {}: {},", key("\"missing\""), null()),
            format!("  {}: {{", key(r#""a \"key\"""#)),
            format!("    {}: [", key("\"list\"")),
            "      {".to_owned(),
            format!("        {}: {},", key("\"inner\""), null()),
            format!("        {}: \"x\"", key("\"text\"")),
            "      },".to_owned(),
            format!("      {}", null()),
            "    ]".to_owned(),
            "  },".to_owned(),
            format!("  {}: [],", key("\"empty\"")),
            format!("  {}: {}", key(&format!("\"{long}\"")), null()),
            "}".to_owned(),
        ]
        .join("\n");
        assert_eq!(tree.render_styled(true, Style::Ansi), want);
        assert_eq!(tree.render_styled(true, Style::Plain), tree.render(true));
    }

    #[test]
    fn a_scalar_document_styles_only_a_null() {
        assert_eq!(Json::Null.render_styled(true, Style::Ansi), null());
        for plain in [Json::from("null"), Json::Int(0), Json::Bool(true)] {
            assert_eq!(plain.render_styled(true, Style::Ansi), plain.render(true));
        }
    }

    #[test]
    fn compact_json_is_never_styled() {
        for style in [Style::Plain, Style::Ansi] {
            assert_eq!(doc().render_styled(false, style), doc().render(false));
        }
    }

    #[test]
    fn a_hostile_key_or_value_adds_no_escape_byte_to_a_styled_document() {
        let hostile = "a\u{1b}[31m\n\u{1b}[2J\u{1b}]0;x\u{7}b";
        let tree = Json::object([(hostile, hostile.into()), ("n", Json::Null)]);
        let out = tree.render_styled(true, Style::Ansi);
        assert!(
            out.contains("a\\u001b[31m\\n\\u001b[2J\\u001b]0;x\\u0007b"),
            "{out:?}"
        );
        assert_only_document_codes(&out);
        assert_balanced(&out);
        // Two keys and one null, each with an opening code and a reset.
        assert_eq!(out.matches('\u{1b}').count(), 6, "{out:?}");
        assert_eq!(strip(&out), tree.render(true));
    }
}
