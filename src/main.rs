use std::io;
use std::process::ExitCode;

use myip::Context;
use myip::http::UreqFetch;

fn main() -> ExitCode {
    let args = match myip::utf8_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(bad) => {
            eprintln!("myip: argument {bad:?} is not valid UTF-8");
            return ExitCode::from(2);
        }
    };
    let (stdout, stderr) = (io::stdout(), io::stderr());
    let (mut out, mut err) = (stdout.lock(), stderr.lock());
    let mut ctx = Context {
        fetch: &UreqFetch,
        out: &mut out,
        err: &mut err,
    };
    ExitCode::from(myip::run(&args, &mut ctx))
}
