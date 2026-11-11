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
