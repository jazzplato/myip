//! Implementation detail of the `myip` binary. The API is not stable.

pub mod http;
pub mod legacy;

use std::ffi::OsString;
use std::io::Write;

use http::Fetch;

/// Everything `run` needs from the outside world, so tests can substitute it.
pub struct Context<'a> {
    pub fetch: &'a dyn Fetch,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Runs the CLI with the arguments after the program name and returns the exit code.
pub fn run(args: &[String], ctx: &mut Context<'_>) -> u8 {
    legacy::run(args.first().map(String::as_str), ctx)
}

/// Converts OS arguments to UTF-8 without altering them. A non-Unicode argument is
/// returned as the error so the caller can reject it (0.0.1 panicked here).
pub fn utf8_args(args: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, OsString> {
    args.into_iter().map(OsString::into_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_arguments_pass_through_unchanged() {
        let got = utf8_args(["ip", "8.8.8.8/org", "héllo"].map(OsString::from)).unwrap();
        assert_eq!(got, ["ip", "8.8.8.8/org", "héllo"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_argument_is_rejected_not_substituted() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![b'i', 0xFF, b'p']);
        assert_eq!(utf8_args([OsString::from("ip"), bad.clone()]), Err(bad));
    }
}
