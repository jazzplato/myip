use std::env;
use std::process;

const URL_IPINFO: &str = "https://ipinfo.io/";
const URL_ININFO_JSON: &str = "json";
const USER_AGENT: &str = concat!("myip/", env!("CARGO_PKG_VERSION"));

fn build_url(query: &str) -> String {
    format!("{URL_IPINFO}{query}")
}

fn run(query: &str) -> Result<(), ureq::Error> {
    // Preserve the historical behavior of printing whatever body came back,
    // even for non-2xx responses. ipinfo.io picks HTML vs JSON error pages
    // from the User-Agent, so ask for JSON explicitly to keep output stable.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into();
    let body = agent
        .get(build_url(query))
        .header("Accept", "application/json")
        .call()?
        .body_mut()
        .read_to_string()?;
    print!("{body}");

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut query = URL_ININFO_JSON;
    if args.len() > 1 {
        query = &args[1];
    }

    if let Err(e) = run(query) {
        println!("Error occured: \n{e}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_appends_query_to_base() {
        assert_eq!(build_url("json"), "https://ipinfo.io/json");
        assert_eq!(build_url("ip"), "https://ipinfo.io/ip");
    }
}
