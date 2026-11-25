//! Command-line surface. The contract is recorded in ADR 0001.

use std::ffi::OsStr;

use clap::builder::styling::{AnsiColor, Effects, Style};
use clap::builder::{Styles, TypedValueParser};
use clap::error::ErrorKind;
use clap::{Arg, Args, Command as ClapCommand, Parser, Subcommand, ValueEnum};

use crate::http::{Family, ProxyMode};
use crate::version::VERSION;

const EXIT_CODES: &str = "\
Exit codes:
  0  success
  1  no answer could be obtained (network, provider or HTTP error)
  2  usage error
  3  --verify ran but providers disagreed";

/// The palette of `cargo --help` (ADR 0005). Only the headings, `Usage:`, literals and
/// placeholders ever reach a person: errors are written without style, so the error, valid
/// and invalid styles are set to cargo's but are never shown.
const fn styles() -> Styles {
    const HEADER: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
    const LITERAL: Style = AnsiColor::Cyan.on_default().effects(Effects::BOLD);
    Styles::styled()
        .header(HEADER)
        .usage(HEADER)
        .literal(LITERAL)
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
        .valid(LITERAL)
        .invalid(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
}

/// Where am I on the network, and what will others see?
///
/// Without a subcommand, prints what ipinfo.io returns for FIELD (default: json).
#[derive(Parser, Debug)]
#[command(name = "myip", version = VERSION.as_str(), after_help = EXIT_CODES, styles = styles())]
#[command(disable_help_subcommand = true)]
pub struct Cli {
    #[command(flatten)]
    pub options: Options,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// ipinfo.io path to print, e.g. ip, city, 8.8.8.8/org, json?token=TOKEN
    #[arg(value_name = "FIELD", value_parser = parse_field)]
    pub field: Option<String>,
}

/// How `public`, `local`, `where` and `all` write their result. The contract is ADR 0002.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// For people; the layout is not a stable interface
    Text,
    /// One compact JSON line; its bytes are stable (schema_version 1)
    Json,
    /// The same document, indented; stable to parse, not byte for byte
    JsonPretty,
    /// The same document as YAML; stable to parse, not byte for byte
    Yaml,
}

impl Format {
    /// Whether this is a machine-readable document, whose stdout is the whole answer.
    pub fn is_document(self) -> bool {
        self != Self::Text
    }
}

/// When `--help` and the text of `local`, `where`, `all` and a `--verify` disagreement carry
/// colour. The contract is ADR 0002, amended by ADR 0005.
#[derive(ValueEnum, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorChoice {
    /// Only when stdout is a terminal that supports it (NO_COLOR, CLICOLOR and CLICOLOR_FORCE apply)
    #[default]
    Auto,
    /// Whatever stdout is
    Always,
    /// Never
    Never,
}

/// The `--color` choice that `--help` has to honour: clap stops at `--help` before a later
/// `--color` can be parsed, so the arguments are read here. The last `--color VALUE` or
/// `--color=VALUE` before a bare `--` whose value is `auto`, `always` or `never` wins; with
/// none, `auto`. A value clap would reject is skipped, as it never reaches the help.
pub fn prescan_color(args: &[String]) -> ColorChoice {
    let mut choice = ColorChoice::Auto;
    let mut args = args
        .iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--")
        .peekable();
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--color=") {
            Some(value) => Some(value),
            None if arg == "--color" => args.peek().copied(),
            None => None,
        };
        if let Some(parsed) = value.and_then(|value| ColorChoice::from_str(value, false).ok()) {
            choice = parsed;
        }
    }
    choice
}

#[derive(Args, Debug, Clone)]
pub struct Options {
    /// Same as --format json; with FIELD the ipinfo.io body is passed through unchanged
    #[arg(long, global = true)]
    pub json: bool,

    /// How public, local, where and all write their result (not with FIELD)
    #[arg(long, value_name = "FORMAT", global = true, conflicts_with = "json")]
    pub format: Option<Format>,

    /// Colour for --help and for the text of local, where, all and a provider disagreement (never for documents or stderr)
    #[arg(
        long,
        value_name = "WHEN",
        value_enum,
        default_value_t = ColorChoice::Auto,
        global = true
    )]
    pub color: ColorChoice,

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
    /// The format to write: `--json` is `--format json`, and the default is text.
    pub fn format(&self) -> Format {
        if self.json {
            Format::Json
        } else {
            self.format.unwrap_or(Format::Text)
        }
    }

    pub fn family(&self) -> Family {
        match (self.ipv4, self.ipv6) {
            (true, _) => Family::V4,
            (_, true) => Family::V6,
            _ => Family::Any,
        }
    }
}

// All four commands are implemented; `Public` carries its own flags.
#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Print your public IP address, as seen by independent providers
    ///
    /// Asks cloudflare (one.one.one.one), ipify (api64.ipify.org), checkip.amazonaws.com,
    /// icanhazip.com, ident.me and ipinfo.io in that order and prints the first valid
    /// answer. With --verify it asks all of them at once and compares the answers per
    /// address family. The providers see your public address; that is the point.
    ///
    /// With a FIELD (`myip public city`) it is the same as `myip city`: ipinfo.io answers
    /// alone and the answer is not verified.
    Public(PublicArgs),
    /// Interfaces with their scope, and the address used to reach the internet
    Local,
    /// Whether this process runs on a host, in a container or in a Kubernetes pod
    Where,
    /// Public address, local interfaces and location together, with NAT and pod highlights
    ///
    /// The public address comes from the same providers as `myip public`. A section that
    /// cannot be produced is reported in place and the exit code stays 0; only a
    /// --verify disagreement exits 3.
    All(AllArgs),
}

#[derive(Args, Debug, Clone, Copy)]
pub struct AllArgs {
    /// Ask every provider for the public address and require agreeing answers from at least two independent operators per family; exit 3 if they differ
    #[arg(long)]
    pub verify: bool,
}

#[derive(Args, Debug, Clone)]
pub struct PublicArgs {
    /// Ask every provider and require agreeing answers from at least two independent operators per family; exit 3 if they differ
    #[arg(long, conflicts_with = "field")]
    pub verify: bool,

    /// ipinfo.io path to print, e.g. city, region, country: the same as `myip FIELD`,
    /// answered by ipinfo.io alone and not verified
    #[arg(value_name = "FIELD", value_parser = parse_field)]
    pub field: Option<String>,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Public(_) => "public",
            Self::Local => "local",
            Self::Where => "where",
            Self::All(_) => "all",
        }
    }
}

/// A FIELD is appended to `https://ipinfo.io/`, so it must stay a relative path: only
/// characters that need no escaping, never more than one leading `/` (which is dropped,
/// as 0.0.1 sent it to the same effect) or a `//` (which would turn the host into
/// whatever follows), and no `..`. It may end in a `?query`, such as ipinfo's
/// `?token=...`: the query follows the first `?`, so it cannot change the host or the
/// path, and a second `?`, a `#` and every other unescaped character are rejected.
fn parse_field(raw: &str) -> Result<String, String> {
    let field = raw.trim();
    let (path, query) = match field.split_once('?') {
        Some((path, query)) => (path, query),
        None => (field, ""),
    };
    let path = path.strip_prefix('/').unwrap_or(path);
    if path.is_empty() && query.is_empty() {
        return Err("must not be empty".to_owned());
    }
    if path.contains("..") {
        return Err("must not contain `..`".to_owned());
    }
    if path.starts_with('/') || path.contains("//") {
        return Err("must be a relative path without a leading `//` or any `//`".to_owned());
    }
    if let Some(bad) = path
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-')))
    {
        return Err(format!(
            "unsupported character {bad:?}; allowed: letters, digits and . _ : / -"
        ));
    }
    if let Some(bad) = query.chars().find(|c| {
        !(c.is_ascii_alphanumeric()
            || matches!(c, '.' | '_' | '~' | '-' | '%' | '=' | '&' | '+' | ',' | ':'))
    }) {
        return Err(format!(
            "unsupported character {bad:?} in the query; allowed: letters, digits and . _ ~ - % = & + , :"
        ));
    }
    Ok(if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{query}")
    })
}

fn proxy_mode(raw: &str) -> Option<ProxyMode> {
    match raw {
        "none" => Some(ProxyMode::None),
        "env" => Some(ProxyMode::Env),
        url if is_http_proxy_url(url) && ureq::Proxy::new(url).is_ok() => {
            Some(ProxyMode::Url(url.to_owned()))
        }
        _ => None,
    }
}

/// `http://[userinfo@]host[:port][/]`. ureq parses the URL with `http::Uri`, which lets
/// `host:not-a-port` and `host:99999` through to fail later as transport errors, so the
/// authority is checked here from that same parse (no second parser to disagree with).
fn is_http_proxy_url(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    let host = authority.host();
    let hostport = authority.as_str().rsplit('@').next().unwrap_or_default();
    let suffix = hostport.get(host.len()..).unwrap_or("?");
    let port_ok = match suffix.strip_prefix(':') {
        None => suffix.is_empty(),
        Some(port) => {
            !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit())
                && port.parse::<u16>().is_ok_and(|p| p != 0)
        }
    };
    uri.scheme_str() == Some("http")
        && !host.is_empty()
        && port_ok
        && uri.query().is_none()
        && matches!(uri.path(), "" | "/")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(args: &[&str]) -> ColorChoice {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        prescan_color(&args)
    }

    #[test]
    fn prescan_reads_the_color_flag_in_every_position() {
        use ColorChoice::{Always, Auto, Never};
        let table: &[(&[&str], ColorChoice)] = &[
            (&[], Auto),
            (&["--help"], Auto),
            (&["--color", "always", "--help"], Always),
            (&["--color=never", "--help"], Never),
            (&["--color=auto", "--help"], Auto),
            (&["--help", "--color", "never"], Never),
            (&["--help", "--color=always"], Always),
            (&["public", "--help", "--color", "always"], Always),
            (&["public", "--color=always", "--help"], Always),
            (&["--color", "always", "public", "--help"], Always),
            (&["-v", "-4", "--color", "never", "where", "--help"], Never),
            // The last valid spelling wins, before or after the command.
            (
                &["--color", "always", "where", "--color", "never", "-h"],
                Never,
            ),
            (&["--color", "never", "--color", "always"], Always),
            (&["--color=never", "--color", "always"], Always),
            (&["--color", "always", "--color", "auto"], Auto),
            // An invalid spelling counts for nothing: it is skipped, or `auto` when alone.
            (&["--color", "always", "--color", "rainbow"], Always),
            (&["--color", "rainbow", "--color=never"], Never),
            (&["--color", "rainbow", "--help"], Auto),
            (&["--color=", "--help"], Auto),
            (&["--color", "", "--help"], Auto),
            (&["--color", "ALWAYS", "--help"], Auto),
            (&["--color=always=never", "--help"], Auto),
            // A bare `--` ends the options: what follows is a FIELD, not a flag.
            (&["--", "--color", "always"], Auto),
            (&["--color", "always", "--", "--color", "never"], Always),
            // A flag with no value, and flags that are not `--color`.
            (&["--help", "--color"], Auto),
        ];
        for (args, want) in table {
            assert_eq!(scan(args), *want, "{args:?}");
        }
        for args in [
            &["--colour", "always", "--help"][..],
            &["--colorful=always", "--help"],
            &["--format", "always", "--help"],
            &["-C", "always", "--help"],
            &["always", "--help"],
        ] {
            assert_eq!(scan(args), ColorChoice::Auto, "{args:?}");
        }
        assert_eq!(scan(&["--color", "always", "--color"]), ColorChoice::Always);
    }
}
