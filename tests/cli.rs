//! Process-level contract tests: they spawn the real binary and must never need a network.

use std::process::{Command, Output};

/// Every process test starts without `IPINFO_TOKEN`, so a developer or CI job that exports
/// one does not change what these tests see.
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_myip"));
    command.env_remove("IPINFO_TOKEN");
    command
}

fn myip(args: &[&str]) -> Output {
    command().args(args).output().expect("spawn myip")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn help_exits_zero_and_documents_the_exit_codes() {
    let out = myip(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("Usage: myip"), "{stdout}");
    assert!(stdout.contains("Exit codes"), "{stdout}");
    assert!(out.stderr.is_empty());
}

#[test]
fn version_is_the_cargo_version_with_an_optional_build_suffix() {
    let out = myip(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    let rest = stdout
        .strip_prefix(concat!("myip ", env!("CARGO_PKG_VERSION")))
        .and_then(|rest| rest.strip_suffix('\n'))
        .unwrap_or_else(|| panic!("unexpected version line {stdout:?}"));
    assert!(
        rest.is_empty() || (rest.starts_with(" (") && rest.ends_with(')')),
        "unexpected version line {stdout:?}"
    );
}

#[test]
fn a_usage_error_exits_2_with_nothing_on_stdout() {
    let out = myip(&["--no-such-flag"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("error:"));
}

#[test]
fn an_invalid_field_exits_2_without_touching_the_network() {
    let out = myip(&["a/../b"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("error:"));
}

#[test]
fn public_accepts_a_field_but_not_together_with_verify() {
    // `--verify` is checked by the parser, so this never reaches the network.
    let out = myip(&["public", "--verify", "city"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("error:"));

    let out = myip(&["public", "a/../b"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn a_query_is_accepted_but_its_value_is_never_printed() {
    // A proxy that hangs up on every connection makes the real transport fail without
    // leaving the machine, whatever else listens on this host.
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in proxy.incoming() {
            drop(stream);
        }
    });
    let out = myip(&[
        "-v",
        "--timeout",
        "2",
        "--proxy",
        &proxy_url,
        "json?token=S3CR3T",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("https://ipinfo.io/json?token=***"),
        "{stderr}"
    );
    assert!(!stderr.contains("S3CR3T"), "{stderr}");
}

// `-6` also applies to the proxy host. An IPv4 literal has no IPv6 address, so the lookup
// fails before any connection is made, whatever the host can reach.
#[test]
fn a_family_flag_with_a_proxy_that_has_no_such_address_says_so() {
    let out = myip(&["-6", "--proxy", "http://127.0.0.1:9", "json?token=S3CR3T"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = text(&out.stderr);
    let hint = "also applies to the proxy host";
    assert_eq!(stderr.matches(hint).count(), 1, "{stderr}");
    assert!(stderr.contains("http://[::1]:PORT"), "{stderr}");
    assert!(!stderr.contains("S3CR3T"), "{stderr}");

    // A proxy that hangs up on every connection, so that without `-6` the request fails in
    // the transport and never reaches whatever listens on this host.
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in proxy.incoming() {
            drop(stream);
        }
    });
    let out = myip(&["--proxy", &proxy_url, "--timeout", "2", "json"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr.contains(hint), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_non_unicode_argument_exits_2_and_is_never_echoed() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let secret = OsStr::from_bytes(b"user:S3CR3T@h/\xFF");
    let out = command()
        .arg("--proxy")
        .arg(secret)
        .output()
        .expect("spawn myip");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not valid UTF-8"), "{stderr}");
    assert!(!stderr.contains("S3CR3T"), "{stderr}");
}

/// A proxy that hangs up on every connection, so a request made through it fails in the
/// transport without leaving the machine.
fn hangup_proxy() -> String {
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", proxy.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in proxy.incoming() {
            drop(stream);
        }
    });
    url
}

#[test]
fn an_invalid_token_variable_is_a_usage_error_that_never_shows_it() {
    let proxy = hangup_proxy();
    let out = command()
        .env("IPINFO_TOKEN", "Qz9 not a token Xw8")
        .args(["-v", "--timeout", "2", "--proxy", &proxy, "city"])
        .output()
        .expect("spawn myip");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = text(&out.stderr);
    assert!(stderr.contains("IPINFO_TOKEN"), "{stderr}");
    assert!(
        !stderr.contains("Qz9") && !stderr.contains("Xw8"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("GET"),
        "no request line before the check: {stderr}"
    );
}

#[test]
fn a_valid_token_variable_is_announced_by_a_fixed_line_and_never_printed() {
    let proxy = hangup_proxy();
    let out = command()
        .env("IPINFO_TOKEN", "Q7abc.DEF_123\n")
        .args(["-v", "--timeout", "2", "--proxy", &proxy, "city"])
        .output()
        .expect("spawn myip");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("myip: Authorization: Bearer *** (from IPINFO_TOKEN)\n"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("Q7abc") && !stderr.contains("DEF_123"),
        "{stderr}"
    );
}

#[test]
fn only_the_passthrough_looks_at_the_token_variable() {
    for args in [&["where"][..], &["--json", "where"]] {
        let out = command()
            .env("IPINFO_TOKEN", "not a token")
            .args(args)
            .output()
            .expect("spawn myip");
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
    }
}

// `SystemEnv::var` is `std::env::var(name).ok()`, so a value that is not valid Unicode reads
// as unset (ADR 0004): no check, no header and no message.
#[cfg(unix)]
#[test]
fn a_token_variable_that_is_not_unicode_reads_as_unset() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let proxy = hangup_proxy();
    let out = command()
        .env("IPINFO_TOKEN", OsStr::from_bytes(b"Qz9\xFFXw8"))
        .args(["-v", "--timeout", "2", "--proxy", &proxy, "city"])
        .output()
        .expect("spawn myip");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("GET https://ipinfo.io/city"), "{stderr}");
    assert!(
        !stderr.contains("Authorization") && !stderr.contains("IPINFO_TOKEN"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("Qz9") && !stderr.contains("Xw8"),
        "{stderr}"
    );
}

// The two tests below assert only what holds on any host: they read the real machine.
#[test]
fn where_always_exits_zero_with_a_versioned_json_document() {
    let out = myip(&["--json", "where"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("{\"schema_version\":1,"), "{stdout}");
    assert!(stdout.ends_with("}\n"), "{stdout}");
}

#[test]
fn local_prints_a_versioned_json_document_whatever_the_host() {
    let out = myip(&["--json", "local"]);
    // 0, or 1 on a host that refuses interface enumeration; the document is there either way.
    assert!(matches!(out.status.code(), Some(0 | 1)), "{:?}", out.status);
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("{\"schema_version\":1,"), "{stdout}");
    assert!(stdout.ends_with("}\n"), "{stdout}");
}

fn parsed(stdout: &[u8]) -> yaml_rust2::Yaml {
    let stdout = text(stdout);
    let mut documents = yaml_rust2::YamlLoader::load_from_str(&stdout)
        .unwrap_or_else(|error| panic!("{error}\n---\n{stdout}"));
    assert_eq!(documents.len(), 1, "{stdout}");
    documents.remove(0)
}

#[test]
fn where_is_one_document_in_json_pretty_json_and_yaml() {
    let json = myip(&["--format", "json", "where"]);
    let pretty = myip(&["--format", "json-pretty", "where"]);
    let yaml = myip(&["where", "--format", "yaml"]);
    for out in [&json, &pretty, &yaml] {
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stderr.is_empty());
    }
    assert_eq!(json.stdout, myip(&["--json", "where"]).stdout);
    assert!(text(&yaml.stdout).starts_with("schema_version: 1\n"));
    assert!(text(&pretty.stdout).starts_with("{\n  \"schema_version\": 1,\n"));
    let tree = parsed(&json.stdout);
    assert_eq!(tree["schema_version"].as_i64(), Some(1));
    assert_eq!(parsed(&pretty.stdout), tree);
    assert_eq!(parsed(&yaml.stdout), tree);
}

#[test]
fn local_yaml_is_a_versioned_document_whatever_the_host() {
    let out = myip(&["--format", "yaml", "local"]);
    assert!(matches!(out.status.code(), Some(0 | 1)), "{:?}", out.status);
    assert!(text(&out.stdout).starts_with("schema_version: 1\n"));
}

#[test]
fn format_is_a_usage_error_for_the_passthrough_and_with_json() {
    for args in [
        &["--format", "yaml", "city"][..],
        &["--format", "json"],
        &["--format", "text", "public", "city"],
        &["--json", "--format", "json", "where"],
        &["--format", "toml", "where"],
    ] {
        let out = myip(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(text(&out.stderr).contains("--format"), "{args:?}");
    }
}

/// Everything that steers colour, so the shell the tests run in cannot change a result.
const COLOUR_VARS: [&str; 7] = [
    "NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "TERM",
    "WT_SESSION",
    "ANSICON",
    "ConEmuANSI",
];

fn myip_with(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = command();
    command.args(args);
    for var in COLOUR_VARS {
        command.env_remove(var);
    }
    command.envs(env.iter().copied());
    command.output().expect("spawn myip")
}

/// Removes the colour codes of ADR 0002 (`ESC [ digits and semicolons m`) and nothing else.
fn without_codes(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("\u{1b}[") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        let digits = tail.len()
            - tail
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == ';')
                .len();
        match tail[digits..].strip_prefix('m') {
            Some(after) => rest = after,
            None => {
                out.push_str("\u{1b}[");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn a_pipe_gets_plain_text_and_always_adds_only_codes() {
    let piped = myip_with(&["where"], &[]);
    assert_eq!(piped.status.code(), Some(0));
    let plain = text(&piped.stdout);
    assert!(!plain.contains('\u{1b}'), "{plain:?}");
    assert!(plain.starts_with("environment"), "{plain:?}");

    let never = myip_with(&["--color", "never", "where"], &[]);
    assert_eq!(never.stdout, piped.stdout);

    let always = myip_with(&["--color", "always", "where"], &[]);
    assert_eq!(always.status.code(), Some(0));
    assert!(always.stderr.is_empty());
    let styled = text(&always.stdout);
    assert!(
        styled.starts_with("\u{1b}[1;36menvironment\u{1b}[0m"),
        "{styled:?}"
    );
    assert_eq!(without_codes(&styled), plain);
}

#[test]
fn every_way_of_turning_colour_off_prints_the_same_bytes_for_text_and_documents() {
    for args in [
        &["where"][..],
        &["--format", "json", "where"],
        &["--format", "json-pretty", "where"],
        &["--format", "yaml", "where"],
    ] {
        let never = myip_with(&[&["--color", "never"][..], args].concat(), &[]);
        assert_eq!(never.status.code(), Some(0), "{args:?}");
        assert!(!text(&never.stdout).contains('\u{1b}'), "{args:?}");
        for (spelled, env) in [
            (args.to_vec(), &[][..]),
            (
                [&["--color", "auto"][..], args].concat(),
                &[("CLICOLOR", "0")][..],
            ),
            (
                args.to_vec(),
                &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")][..],
            ),
            (args.to_vec(), &[("TERM", "dumb")][..]),
        ] {
            let out = myip_with(&spelled, env);
            assert_eq!(
                (out.status.code(), &out.stdout, &out.stderr),
                (never.status.code(), &never.stdout, &never.stderr),
                "{spelled:?} {env:?}"
            );
        }
    }
}

#[test]
fn the_environment_steers_auto_and_the_flag_overrules_it() {
    let styled = |args: &[&str], env: &[(&str, &str)]| {
        let out = myip_with(args, env);
        assert_eq!(out.status.code(), Some(0), "{args:?} {env:?}");
        assert!(out.stderr.is_empty(), "{args:?} {env:?}");
        text(&out.stdout).contains('\u{1b}')
    };
    assert!(styled(&["where"], &[("CLICOLOR_FORCE", "1")]));
    assert!(styled(
        &["--color", "always", "where"],
        &[("NO_COLOR", "1")]
    ));
    assert!(!styled(
        &["where"],
        &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")]
    ));
    assert!(!styled(
        &["--color", "never", "where"],
        &[("CLICOLOR_FORCE", "1")]
    ));
    assert!(!styled(&["where"], &[("CLICOLOR_FORCE", "0")]));
    assert!(!styled(&["where"], &[]));
}

/// The only escape sequences a document holds: cyan keys, dim `null`s and their resets.
fn only_document_codes(text: &str) {
    let rest = ["\u{1b}[36m", "\u{1b}[2m", "\u{1b}[0m"]
        .iter()
        .fold(text.to_owned(), |text, code| text.replace(code, ""));
    assert!(!rest.contains('\u{1b}'), "{text:?}");
}

/// `text` without what varies between two runs of the same command through a proxy that
/// hangs up: each measured latency and each transport error, which one of two failure
/// kinds the kernel reports.
fn without_noise(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.contains("latency_ms") {
                "latency_ms"
            } else if line.contains("\"io: ") {
                "io"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `local`, `where` and `all`; `all` goes through a proxy that hangs up, so it never leaves
/// the machine and still prints its document.
fn document_commands() -> Vec<(Vec<String>, bool)> {
    let proxy = hangup_proxy();
    let all = ["--proxy", &proxy, "--timeout", "2", "all"].map(str::to_owned);
    vec![
        (vec!["local".to_owned()], false),
        (vec!["where".to_owned()], false),
        (all.to_vec(), true),
    ]
}

fn args_of<'a>(flags: &[&'a str], command: &'a [String]) -> Vec<&'a str> {
    flags
        .iter()
        .copied()
        .chain(command.iter().map(String::as_str))
        .collect()
}

#[test]
fn a_pretty_document_with_always_adds_only_key_and_null_codes() {
    for (command, measured) in document_commands() {
        for format in ["json-pretty", "yaml"] {
            let label = format!("{format} {command:?}");
            let plain = myip_with(
                &args_of(&["--color", "never", "--format", format], &command),
                &[],
            );
            let always = myip_with(
                &args_of(&["--color", "always", "--format", format], &command),
                &[],
            );
            assert!(matches!(always.status.code(), Some(0 | 1)), "{label}");
            assert_eq!(always.status, plain.status, "{label}");
            let styled = text(&always.stdout);
            assert!(styled.contains("\u{1b}[36m"), "{label}: {styled:?}");
            only_document_codes(&styled);
            let (unstyled, want) = (without_codes(&styled), text(&plain.stdout));
            if measured {
                assert_eq!(without_noise(&unstyled), without_noise(&want), "{label}");
            } else {
                assert_eq!(unstyled, want, "{label}");
                assert_eq!(parsed(&always.stdout), parsed(&plain.stdout), "{label}");
            }
            assert!(!text(&always.stderr).contains('\u{1b}'), "{label}");
        }
    }
}

#[test]
fn compact_json_with_always_has_no_escape_byte_at_all() {
    for (command, _) in document_commands() {
        for flags in [
            &["--color", "always", "--format", "json"][..],
            &["--color", "always", "--json"],
            &["--json", "--color", "always"],
        ] {
            let out = myip_with(&args_of(flags, &command), &[("CLICOLOR_FORCE", "1")]);
            assert!(
                matches!(out.status.code(), Some(0 | 1)),
                "{flags:?} {command:?}"
            );
            assert!(!out.stdout.contains(&0x1b), "{flags:?} {command:?}");
            assert!(!out.stderr.contains(&0x1b), "{flags:?} {command:?}");
            assert!(out.stdout.starts_with(b"{"), "{flags:?} {command:?}");
        }
    }
}

#[test]
fn clicolor_force_colours_the_text_and_not_a_document() {
    for (command, measured) in document_commands() {
        for format in ["json-pretty", "yaml", "json"] {
            let args = args_of(&["--format", format], &command);
            let out = myip_with(&args, &[("CLICOLOR_FORCE", "1")]);
            assert!(matches!(out.status.code(), Some(0 | 1)), "{args:?}");
            assert!(!out.stdout.contains(&0x1b), "{args:?}");
            assert!(!out.stderr.contains(&0x1b), "{args:?}");
            let plain = myip_with(&args, &[]);
            assert_eq!(out.status, plain.status, "{args:?}");
            if measured {
                let (got, want) = (text(&out.stdout), text(&plain.stdout));
                assert_eq!(without_noise(&got), without_noise(&want), "{args:?}");
            } else {
                assert_eq!(out.stdout, plain.stdout, "{args:?}");
            }
        }
    }
    let forced = myip_with(&["where"], &[("CLICOLOR_FORCE", "1")]);
    assert!(text(&forced.stdout).contains('\u{1b}'));
}

type Case = (
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
);

#[test]
fn without_colour_every_format_is_byte_for_byte_what_it_was() {
    let cases: [Case; 8] = [
        (&["--color", "never"], &[]),
        (&["--color", "auto"], &[]),
        (&["--color=never"], &[("CLICOLOR_FORCE", "1")]),
        (&[], &[("NO_COLOR", "1")]),
        (&[], &[("CLICOLOR", "0")]),
        (&[], &[("TERM", "dumb")]),
        (&[], &[("CLICOLOR_FORCE", "0")]),
        (&[], &[]),
    ];
    for command in ["local", "where"] {
        for format in ["text", "json", "json-pretty", "yaml"] {
            let plain = myip_with(&["--format", format, command], &[]);
            assert!(
                matches!(plain.status.code(), Some(0 | 1)),
                "{format} {command}"
            );
            assert!(!plain.stdout.contains(&0x1b), "{format} {command}");
            for (flags, env) in cases {
                let args = [flags, &["--format", format, command]].concat();
                let out = myip_with(&args, env);
                assert_eq!(out.status, plain.status, "{args:?} {env:?}");
                assert_eq!(out.stdout, plain.stdout, "{args:?} {env:?}");
                assert_eq!(out.stderr, plain.stderr, "{args:?} {env:?}");
            }
        }
    }
}

#[test]
fn colour_never_reaches_compact_json_a_trace_or_a_usage_error() {
    for args in [
        &["--color", "always", "--json", "where"][..],
        &["--color", "always", "--format", "json", "where"],
        &["--color", "always", "--json", "local"],
        &["--color", "always", "--format", "json", "local"],
    ] {
        let out = myip_with(args, &[("CLICOLOR_FORCE", "1")]);
        assert!(!text(&out.stdout).contains('\u{1b}'), "{args:?}");
        // `local` may write a diagnostic when the interfaces cannot be listed.
        assert!(!text(&out.stderr).contains('\u{1b}'), "{args:?}");
    }
    let out = myip_with(&["-v", "--color", "always", "where"], &[]);
    assert!(text(&out.stdout).contains('\u{1b}'));
    assert!(!text(&out.stderr).contains('\u{1b}'));
    let out = myip_with(
        &["-v", "--color", "always", "--format", "yaml", "where"],
        &[],
    );
    assert!(text(&out.stdout).contains("\u{1b}[36m"));
    assert!(!text(&out.stderr).contains('\u{1b}'));

    for args in [
        &["--color", "rainbow", "where"][..],
        &["--color", "ALWAYS", "where"],
        &["--color"],
    ] {
        let out = myip_with(args, &[("CLICOLOR_FORCE", "1")]);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let stderr = text(&out.stderr);
        assert!(
            stderr.contains("--color") && !stderr.contains('\u{1b}'),
            "{stderr:?}"
        );
    }
}

/// The one place a real terminal is involved: `script` gives the child a pseudo-terminal.
/// `command` is what the shell runs after the path of the binary, and the output is
/// everything the child wrote to its terminal.
#[cfg(target_os = "linux")]
fn on_a_terminal(command: &str, env: &[(&str, &str)]) -> String {
    let quoted = format!(
        "'{}' {command}",
        env!("CARGO_BIN_EXE_myip").replace('\'', "'\\''")
    );
    let mut script = Command::new("script");
    script.env_remove("IPINFO_TOKEN");
    script.args(["-qec", &quoted, "/dev/null"]);
    for var in COLOUR_VARS {
        script.env_remove(var);
    }
    script.env("TERM", "xterm").envs(env.iter().copied());
    let out = script
        .output()
        .expect("`script` (util-linux) is needed for the pseudo-terminal test");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    text(&out.stdout)
}

#[cfg(target_os = "linux")]
#[test]
fn a_real_terminal_gets_colour_by_default_and_not_with_no_color() {
    let on = on_a_terminal("where", &[]);
    assert!(on.contains("\u{1b}[1;36menvironment\u{1b}[0m"), "{on:?}");
    let off = on_a_terminal("where", &[("NO_COLOR", "1")]);
    assert!(off.starts_with("environment"), "{off:?}");
    assert!(!off.contains('\u{1b}'), "{off:?}");
    let dumb = on_a_terminal("where", &[("TERM", "dumb")]);
    assert!(!dumb.contains('\u{1b}'), "{dumb:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_real_terminal_gets_a_coloured_help_by_default_and_not_with_no_color() {
    let on = on_a_terminal("--help", &[]);
    assert!(on.contains(USAGE_HEADING), "{on:?}");
    let sub = on_a_terminal("local --help", &[]);
    assert!(sub.contains(USAGE_HEADING), "{sub:?}");
    for (env, args) in [
        (&[("NO_COLOR", "1")][..], "--help"),
        (&[("TERM", "dumb")], "--help"),
        (&[("CLICOLOR", "0")], "--help"),
        (&[], "--color never --help"),
        (&[], "--help --color never"),
    ] {
        let off = on_a_terminal(args, env);
        assert!(off.contains("Usage: myip"), "{off:?}");
        assert!(!off.contains('\u{1b}'), "{args} {env:?}: {off:?}");
    }
}

/// `Usage:` as `--help` writes it when coloured: bold, then green, then reset.
const USAGE_HEADING: &str = "\u{1b}[1m\u{1b}[32mUsage:\u{1b}[0m";

/// The help of every form, as 0.3.0 printed it but for the `--color` line (ADR 0005 says
/// that help is covered now), as `(arguments, text)`.
fn help_golden() -> Vec<(Vec<String>, String)> {
    let golden = include_str!("golden/help.txt").replace("\r\n", "\n");
    let mut sections: Vec<(Vec<String>, String)> = Vec::new();
    for line in golden.split_inclusive('\n') {
        match line.strip_prefix("=== ") {
            Some(args) => sections.push((
                args.trim_end().split(' ').map(str::to_owned).collect(),
                String::new(),
            )),
            None => sections
                .last_mut()
                .expect("the golden starts with a header")
                .1
                .push_str(line),
        }
    }
    sections
}

#[test]
fn help_is_coloured_like_cargo_when_asked_for() {
    for args in [
        &["--color", "always", "--help"][..],
        &["local", "--color", "always", "--help"],
        &["--help", "--color", "always"],
        &["public", "--help", "--color=always"],
    ] {
        let out = myip_with(args, &[]);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}");
        let stdout = text(&out.stdout);
        assert!(stdout.contains(USAGE_HEADING), "{args:?}: {stdout:?}");
        assert!(
            stdout.contains("\u{1b}[1m\u{1b}[36m--json\u{1b}[0m"),
            "{args:?}: {stdout:?}"
        );
        assert!(
            stdout.contains("\u{1b}[36m<FORMAT>\u{1b}[0m"),
            "{args:?}: {stdout:?}"
        );
    }
}

#[test]
fn help_without_its_codes_is_the_plain_help_of_0_3_0_in_every_form() {
    let golden = help_golden();
    assert_eq!(golden.len(), 10, "the long and short help of five commands");
    for (args, want) in golden {
        let run = |color: &str| {
            let mut full = vec!["--color", color];
            full.extend(args.iter().map(String::as_str));
            myip_with(&full, &[])
        };
        let (never, always) = (run("never"), run("always"));
        for out in [&never, &always] {
            assert_eq!(out.status.code(), Some(0), "{args:?}");
            assert!(out.stderr.is_empty(), "{args:?}");
        }
        let (never, always) = (text(&never.stdout), text(&always.stdout));
        assert_eq!(never, want, "{args:?}");
        assert!(always.contains('\u{1b}'), "{args:?} has no colour");
        assert_eq!(without_codes(&always), never, "{args:?}");
    }
}

#[test]
fn help_is_plain_in_a_pipe_and_whenever_colour_is_off() {
    type Env<'a> = &'a [(&'a str, &'a str)];
    let cases: [(&[&str], Env); 12] = [
        (&["--help"], &[]),
        (&["-h"], &[]),
        (&["public", "--help"], &[]),
        (&["--color", "never", "--help"], &[]),
        (&["--help", "--color", "never"], &[]),
        (&["--color=never", "local", "--help"], &[]),
        (&["local", "--help", "--color", "never"], &[]),
        (&["--color", "auto", "--help"], &[("NO_COLOR", "1")]),
        (&["--help"], &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")]),
        (&["--help"], &[("CLICOLOR", "0")]),
        (&["--help"], &[("CLICOLOR_FORCE", "0")]),
        (
            &["--color", "never", "all", "--help"],
            &[("CLICOLOR_FORCE", "1")],
        ),
    ];
    let golden = help_golden();
    for (args, env) in cases {
        let out = myip_with(args, env);
        assert_eq!(out.status.code(), Some(0), "{args:?} {env:?}");
        assert!(out.stderr.is_empty(), "{args:?} {env:?}");
        assert!(
            !out.stdout.contains(&0x1b),
            "{args:?} {env:?}: {}",
            text(&out.stdout)
        );
        let words: Vec<String> = args
            .iter()
            .filter(|arg| !arg.starts_with("--color") && !["never", "auto"].contains(arg))
            .map(|arg| (*arg).to_owned())
            .collect();
        let (_, want) = golden
            .iter()
            .find(|(golden_args, _)| *golden_args == words)
            .unwrap_or_else(|| panic!("no golden for {words:?}"));
        assert_eq!(text(&out.stdout), *want, "{args:?} {env:?}");
    }
}

#[test]
fn clicolor_force_colours_help_in_a_pipe_like_the_text_reports() {
    for args in [&["--help"][..], &["-h"], &["all", "--help"]] {
        let out = myip_with(args, &[("CLICOLOR_FORCE", "1")]);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}");
        assert!(text(&out.stdout).contains(USAGE_HEADING), "{args:?}");
    }
    // The flag still wins over the environment, and NO_COLOR over CLICOLOR_FORCE.
    let out = myip_with(&["--color", "always", "--help"], &[("NO_COLOR", "1")]);
    assert!(text(&out.stdout).contains(USAGE_HEADING));
}

#[test]
fn the_version_and_usage_errors_are_never_coloured() {
    for env in [&[][..], &[("CLICOLOR_FORCE", "1")]] {
        let out = myip_with(&["--version", "--color", "always"], env);
        assert_eq!(out.status.code(), Some(0), "{env:?}");
        assert!(out.stderr.is_empty(), "{env:?}");
        assert!(!out.stdout.contains(&0x1b), "{env:?}");
        assert!(text(&out.stdout).starts_with("myip "), "{env:?}");

        for args in [
            &["--bogus", "--color", "always"][..],
            &["where", "--bogus", "--color=always"],
            &["--color", "rainbow", "--help"],
        ] {
            let out = myip_with(args, env);
            assert_eq!(out.status.code(), Some(2), "{args:?} {env:?}");
            assert!(out.stdout.is_empty(), "{args:?} {env:?}");
            assert!(!out.stderr.contains(&0x1b), "{args:?} {env:?}");
            assert!(text(&out.stderr).starts_with("error: "), "{args:?} {env:?}");
        }
    }
}
