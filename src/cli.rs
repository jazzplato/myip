//! Command-line surface. The contract is recorded in ADR 0001.

use std::ffi::OsStr;

use clap::builder::TypedValueParser;
use clap::error::ErrorKind;
use clap::{Arg, Args, Command as ClapCommand, Parser, Subcommand};

use crate::http::{Family, ProxyMode};

const EXIT_CODES: &str = "\
Exit codes:
  0  success
  1  no answer could be obtained (network, provider or HTTP error)
  2  usage error
  3  --verify ran but providers disagreed";

/// Where am I on the network, and what will others see?
///
/// Without a subcommand, prints what ipinfo.io returns for FIELD (default: json).
#[derive(Parser, Debug)]
#[command(name = "myip", version, after_help = EXIT_CODES)]
#[command(disable_help_subcommand = true)]
pub struct Cli {
    #[command(flatten)]
    pub options: Options,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// ipinfo.io path to print, e.g. ip, city, 8.8.8.8/org
    #[arg(value_name = "FIELD", value_parser = parse_field)]
    pub field: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct Options {
    /// Machine-readable output; with FIELD the ipinfo.io body is passed through unchanged
    #[arg(long, global = true)]
    pub json: bool,

    /// Use IPv4 only
    #[arg(short = '4', global = true, conflicts_with = "ipv6")]
    pub ipv4: bool,

    /// Use IPv6 only
    #[arg(short = '6', global = true)]
    pub ipv6: bool,

    /// Give up on a request after this many seconds
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 10,
        value_parser = clap::value_parser!(u64).range(1..=600),
        global = true
    )]
    pub timeout: u64,

    /// Proxy for outgoing requests: none (default), env (HTTP_PROXY, HTTPS_PROXY, ALL_PROXY) or an http:// URL
    #[arg(
        long,
        value_name = "none|env|URL",
        default_value = "none",
        value_parser = ProxyParser,
        global = true
    )]
    pub proxy: ProxyMode,

    /// Log requests and responses to stderr
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

impl Options {
    pub fn family(&self) -> Family {
        match (self.ipv4, self.ipv6) {
            (true, _) => Family::V4,
            (_, true) => Family::V6,
            _ => Family::Any,
        }
    }
}

// Not implemented yet; the names are reserved so scripts cannot grow to depend on them as fields.
#[derive(Subcommand, Debug, Clone, Copy)]
pub enum Command {
    #[command(hide = true)]
    Public,
    #[command(hide = true)]
    Local,
    #[command(hide = true)]
    Where,
    #[command(hide = true)]
    All,
}

impl Command {
    pub fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Local => "local",
            Self::Where => "where",
            Self::All => "all",
        }
    }
}

/// A FIELD is appended to `https://ipinfo.io/`, so it must stay a relative path: only
/// characters that need no escaping, never a leading `/` or a `//` (which would turn
/// the host into whatever follows), and no `..`.
fn parse_field(raw: &str) -> Result<String, String> {
    let field = raw.trim();
    if field.is_empty() {
        return Err("must not be empty".to_owned());
    }
    if field.contains("..") {
        return Err("must not contain `..`".to_owned());
    }
    if field.starts_with('/') || field.contains("//") {
        return Err("must be a relative path without a leading `/` or `//`".to_owned());
    }
    if let Some(bad) = field
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-')))
    {
        return Err(format!(
            "unsupported character {bad:?}; allowed: letters, digits and . _ : / -"
        ));
    }
    Ok(field.to_owned())
}

fn proxy_mode(raw: &str) -> Option<ProxyMode> {
    match raw {
        "none" => Some(ProxyMode::None),
        "env" => Some(ProxyMode::Env),
        url if url.starts_with("http://") && ureq::Proxy::new(url).is_ok() => {
            Some(ProxyMode::Url(url.to_owned()))
        }
        _ => None,
    }
}

/// Validates `--proxy` without letting clap quote the rejected value, which may carry
/// credentials in any shape (with or without a scheme, with spaces or quotes).
#[derive(Clone)]
struct ProxyParser;

impl TypedValueParser for ProxyParser {
    type Value = ProxyMode;

    fn parse_ref(
        &self,
        cmd: &ClapCommand,
        _arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<ProxyMode, clap::Error> {
        value.to_str().and_then(proxy_mode).ok_or_else(|| {
            cmd.clone().error(
                ErrorKind::InvalidValue,
                "invalid value for '--proxy <none|env|URL>': expected `none`, `env` or an http://HOST:PORT URL",
            )
        })
    }
}
