extern crate reqwest;

use std::env;
use std::process;

const URL_IPINFO: &str = "https://ipinfo.io/";
const URL_ININFO_JSON: &str = "json";

fn run(query: &str) -> Result<(), reqwest::Error> {
    let url = format!("{}{}", URL_IPINFO, query);
    let body = reqwest::get(&url)?.text()?;
    print!("{}", body);

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut query = URL_ININFO_JSON;
    if args.len() > 1 {
        query = &args[1];
    }

    if let Err(e) = run(&query) {
        println!("Error occured: \n{}", e);
        process::exit(1);
    }
}
