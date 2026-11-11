//! A tiny ordered JSON writer for the `--json` documents of the dimension subcommands.
//! Output only: nothing in `myip` parses JSON, so a full serializer would be dead weight.

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
    pub fn render(&self, _pretty: bool) -> String {
        unimplemented!()
    }
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
            ("\u{7f}", "\"\u{7f}\""),
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
}
