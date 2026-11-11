//! How `local`, `where` and `all` print: one compact JSON line or plain text on stdout, and
//! with `-v` a trace of what was looked at on stderr.

use std::io::{self, Write};

use crate::cli::Options;
use crate::env::RecordingEnv;
use crate::json::Json;
use crate::sanitize;

/// Prints `json` as one line when `--json` is set, otherwise `text` (when there is any).
/// Plain mode prints nothing on stdout when `text` is `None`: a failure belongs on stderr.
///
/// This is the boundary where text stops being trusted: control characters in `text`
/// (other than its own newlines) are shown as visible escapes, never printed raw.
pub fn emit(
    options: &Options,
    json: &Json,
    text: Option<&str>,
    out: &mut dyn Write,
) -> io::Result<()> {
    if options.json {
        writeln!(out, "{}", json.render(false))
    } else if let Some(text) = text {
        out.write_all(sanitize::escape_keeping_newlines(text).as_bytes())
    } else {
        Ok(())
    }
}

/// One `myip: ` line on stderr, with control characters shown as visible escapes.
pub fn diagnostic(err: &mut dyn Write, message: &str) {
    let _ = writeln!(err, "myip: {}", sanitize::escape(message));
}

/// With `-v`: every variable and file that was consulted, then the notes about what could
/// not be determined. Without `-v` nothing is written.
pub fn trace(options: &Options, env: &RecordingEnv<'_>, notes: &[String], err: &mut dyn Write) {
    if !options.verbose {
        return;
    }
    for access in env.accesses() {
        diagnostic(err, &access.to_string());
    }
    for note in notes {
        diagnostic(err, note);
    }
}

/// Maps a failed stdout write to exit code 1, like the legacy command does.
pub fn exit_code(written: io::Result<()>, otherwise: u8) -> u8 {
    if written.is_ok() { otherwise } else { 1 }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli::Cli;

    fn options(args: &[&str]) -> Options {
        Cli::try_parse_from(std::iter::once("myip").chain(args.iter().copied()))
            .unwrap()
            .options
    }

    fn printed(args: &[&str], json: &Json, text: Option<&str>) -> String {
        let mut out = Vec::new();
        emit(&options(args), json, text, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn text_shows_control_characters_visibly_and_keeps_its_own_newlines() {
        let text = "a\u{1b}[2Jb\nc\r\u{7f}\u{9b}\u{202e}\0\t\n";
        assert_eq!(
            printed(&[], &Json::Null, Some(text)),
            "a\\x1b[2Jb\nc\\r\\x7f\\x9b\\u{202e}\\x00\\t\n"
        );
    }

    #[test]
    fn ordinary_text_including_non_ascii_is_printed_as_is() {
        let text = "interfaces (observed)\n  h\u{e9}llo \u{4e16}\u{754c} \u{1f980}\n";
        assert_eq!(printed(&[], &Json::Null, Some(text)), text);
    }

    #[test]
    fn json_mode_prints_one_line_and_never_the_text() {
        let json = Json::object([("name", "a\u{1b}\u{85}\u{202e}b".into())]);
        let out = printed(&["--json"], &json, Some("text\n"));
        assert_eq!(out, "{\"name\":\"a\\u001b\\u0085\\u202eb\"}\n");
    }
}
