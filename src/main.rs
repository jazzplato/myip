use std::io;
use std::process::ExitCode;

use myip::Context;
use myip::http::UreqFetch;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let (stdout, stderr) = (io::stdout(), io::stderr());
    let (mut out, mut err) = (stdout.lock(), stderr.lock());
    let mut ctx = Context {
        fetch: &UreqFetch,
        out: &mut out,
        err: &mut err,
    };
    ExitCode::from(myip::run(&args, &mut ctx))
}
