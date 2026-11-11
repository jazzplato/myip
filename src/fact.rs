//! A fact is a value together with where it came from and how we know it.

use std::fmt;

use crate::json::Json;

/// `schema_version` of every `--json` document the dimension subcommands print.
pub const SCHEMA_VERSION: u32 = 1;
/// How `myip` came to know a value.
///
/// - `Declared`: handed to the process by configuration someone set (environment
///   variables such as the Kubernetes Downward API, `resolv.conf`, the service-account
///   namespace file). It says what the orchestrator claims, not what is true.
/// - `Observed`: measured by `myip` from the running system (interface addresses, the
///   routing table, the egress probe, cgroups, mounts, marker files).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    Declared,
    Observed,
}

impl Basis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Observed => "observed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Flag(bool),
    List(Vec<String>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Flag(true) => f.write_str("yes"),
            Self::Flag(false) => f.write_str("no"),
            Self::List(items) => f.write_str(&items.join(", ")),
        }
    }
}

impl Value {
    fn to_json(&self) -> Json {
        match self {
            Self::Text(text) => text.as_str().into(),
            Self::Flag(flag) => (*flag).into(),
            Self::List(items) => items.clone().into(),
        }
    }
}

/// `source` names the origin in a fixed vocabulary: `env:NAME`, `file:/path`,
/// `observed:what`. A fact derived from several inputs lists them separated by commas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub value: Value,
    pub source: String,
    pub basis: Basis,
}

impl Fact {
    pub fn declared(value: Value, source: impl Into<String>) -> Self {
        Self {
            value,
            source: source.into(),
            basis: Basis::Declared,
        }
    }

    pub fn observed(value: Value, source: impl Into<String>) -> Self {
        Self {
            value,
            source: source.into(),
            basis: Basis::Observed,
        }
    }

    pub fn text(&self) -> String {
        self.value.to_string()
    }

    pub fn to_json(&self) -> Json {
        Json::object([
            ("value", self.value.to_json()),
            ("source", self.source.as_str().into()),
            ("basis", self.basis.as_str().into()),
        ])
    }
}

impl From<&Option<Fact>> for Json {
    fn from(fact: &Option<Fact>) -> Self {
        fact.as_ref().map_or(Json::Null, Fact::to_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fact_renders_value_source_and_basis_in_that_order() {
        let fact = Fact::declared(Value::Text("10.1.2.3".to_owned()), "env:POD_IP");
        assert_eq!(
            fact.to_json().render(true),
            "{\n  \"value\": \"10.1.2.3\",\n  \"source\": \"env:POD_IP\",\n  \"basis\": \"declared\"\n}"
        );
    }

    #[test]
    fn flags_and_lists_keep_their_json_types() {
        let flag = Fact::declared(Value::Flag(false), "env:POD_IP,env:HOST_IP");
        assert!(flag.to_json().render(true).contains("\"value\": false"));
        let list = Fact::observed(
            Value::List(vec!["a".to_owned(), "b".to_owned()]),
            "file:/etc/resolv.conf",
        );
        let json = list.to_json().render(true);
        assert!(
            json.contains("\"value\": [\n    \"a\",\n    \"b\"\n  ]"),
            "{json}"
        );
        assert!(json.contains("\"basis\": \"observed\""), "{json}");
    }

    #[test]
    fn a_missing_fact_is_null() {
        assert_eq!(Json::from(&None::<Fact>), Json::Null);
    }

    #[test]
    fn values_print_for_people() {
        assert_eq!(Value::Flag(true).to_string(), "yes");
        assert_eq!(Value::Flag(false).to_string(), "no");
        assert_eq!(
            Value::List(vec!["a".to_owned(), "b".to_owned()]).to_string(),
            "a, b"
        );
        assert_eq!(Value::Text("x".to_owned()).to_string(), "x");
    }
}
