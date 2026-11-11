//! Process-level contract tests: they spawn the real binary and must never need a network.

use std::process::{Command, Output};

fn myip(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_myip"))
        .args(args)
        .output()
        .expect("spawn myip")
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
fn version_matches_cargo() {
    let out = myip(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        concat!("myip ", env!("CARGO_PKG_VERSION"), "\n")
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

#[cfg(unix)]
#[test]
fn a_non_unicode_argument_exits_2_and_is_never_echoed() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let secret = OsStr::from_bytes(b"user:S3CR3T@h/\xFF");
    let out = Command::new(env!("CARGO_BIN_EXE_myip"))
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
