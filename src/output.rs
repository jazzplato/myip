//! How `local`, `where` and `all` print: one compact JSON line or plain text on stdout, and
//! with `-v` a trace of what was looked at on stderr.

use std::io::{self, Write};

use crate::cli::Options;
use crate::env::RecordingEnv;
use crate::json::Json;

/// Prints `json` as one line when `--json` is set, otherwise `text` (when there is any).
/// Plain mode prints nothing on stdout when `text` is `None`: a failure belongs on stderr.
pub fn emit(
    options: &Options,
    json: &Json,
    text: Option<&str>,
    out: &mut dyn Write,
) -> io::Result<()> {
    if options.json {
        writeln!(out, "{}", json.render(false))
    } else if let Some(text) = text {
        out.write_all(text.as_bytes())
    } else {
        Ok(())
    }
}

/// With `-v`: every variable and file that was consulted, then the notes about what could
/// not be determined. Without `-v` nothing is written.
pub fn trace(options: &Options, env: &RecordingEnv<'_>, notes: &[String], err: &mut dyn Write) {
    if !options.verbose {
        return;
    }
    for access in env.accesses() {
        let _ = writeln!(err, "myip: {access}");
    }
    for note in notes {
        let _ = writeln!(err, "myip: {note}");
    }
}

/// Maps a failed stdout write to exit code 1, like the legacy command does.
pub fn exit_code(written: io::Result<()>, otherwise: u8) -> u8 {
    if written.is_ok() { otherwise } else { 1 }
}
