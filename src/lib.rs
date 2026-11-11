//! Implementation detail of the `myip` binary. The API is not stable.

pub mod http;
pub mod legacy;

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
