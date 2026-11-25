//! Documentation contract: a command in a fenced shell block runs when pasted as is.
//!
//! macOS's default shell is zsh, and an interactive zsh does not treat `#` as a comment, so
//! `myip local   # in a container` passes the comment to `myip` as arguments. A comment on a
//! line of its own is the same hazard. Explain a command in the prose above the block
//! instead. Reads files only; no network.

use std::fs;
use std::path::{Path, PathBuf};

fn documents() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("README.md")];
    let mut docs: Vec<PathBuf> = fs::read_dir(root.join("docs"))
        .expect("docs directory")
        .map(|entry| entry.expect("docs entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    docs.sort();
    files.extend(docs);
    files
}

fn is_shell_fence(info: &str) -> bool {
    matches!(info.trim(), "sh" | "bash" | "shell" | "zsh")
}

fn problem(line: &str) -> Option<&'static str> {
    if line.starts_with("$ ") {
        Some("starts with a `$ ` prompt")
    } else if line.starts_with('#') || line.contains(" #") || line.contains("\t#") {
        Some("has a `#` comment")
    } else {
        None
    }
}

#[test]
fn shell_blocks_in_the_docs_paste_as_is() {
    let mut found = Vec::new();
    for path in documents() {
        let text = fs::read_to_string(&path).expect("read document");
        let mut in_shell = false;
        let mut in_other = false;
        for (index, line) in text.lines().enumerate() {
            if let Some(info) = line.trim_start().strip_prefix("```") {
                if in_shell || in_other {
                    in_shell = false;
                    in_other = false;
                } else if is_shell_fence(info) {
                    in_shell = true;
                } else {
                    in_other = true;
                }
            } else if in_shell {
                if let Some(why) = problem(line) {
                    let name = path.file_name().unwrap().to_string_lossy();
                    found.push(format!("{name}:{}: {why}: {line}", index + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "commands that do not paste as is (move the explanation into the prose):\n{}",
        found.join("\n")
    );
}
