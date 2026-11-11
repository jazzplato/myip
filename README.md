# myip
A small command line tool that answers "where am I on the network, and what will others see?".

Today `myip` prints what [ipinfo.io](https://ipinfo.io) says about your public address. Multi-provider public IP lookup with verification, local interfaces and container/Kubernetes identity are planned; the contract they will follow is [ADR 0001](docs/adr/0001-product-scope-and-cli-contract.md).

## Usage

```
myip [OPTIONS] [FIELD]
```

`FIELD` is an ipinfo.io path: `ip`, `city`, `region`, `country`, `loc`, `org`, `postal`, `timezone`, `8.8.8.8/org`, and so on. Without it you get the full JSON.

> To get your public ip

```bash
myip ip
```
```
12.166.238.132
```

> To get your current city

```bash
myip city
```
```
Chicago
```

> To get information about another address

```bash
myip 8.8.8.8/org
```

> To get full information in JSON format

```bash
myip
```
```json
{
  "ip": "12.166.238.132",
  "city": "Chicago",
  "region": "Illinois",
  "country": "US",
  "loc": "41.8500,-87.6500",
  "org": "AS7018 AT&T Services, Inc.",
  "postal": "60666",
  "timezone": "America/Chicago",
  "readme": "https://ipinfo.io/missingauth"
}
```

### Options

| Option | Meaning |
|--------|---------|
| `--timeout <SECS>` | Give up on a request after 1 to 600 seconds (default 10) |
| `-4` / `-6` | Use IPv4 or IPv6 only (mutually exclusive) |
| `--proxy none\|env\|URL` | `none` (default) ignores proxy environment variables; `env` honors `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`; or give an `http://HOST:PORT` URL |
| `--json` | Accepted for forward compatibility; the ipinfo.io body is already passed through unchanged |
| `-v`, `--verbose` | Log requests to stderr |

Credentials in a proxy URL are never printed. They are visible to other local users on the command line, so prefer `--proxy env`.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | No answer: network, provider or HTTP error (details on stderr) |
| 2 | Usage error |
| 3 | Reserved for `--verify` disagreement (not used yet) |

The answer goes to stdout and diagnostics to stderr, so `ip=$(myip ip) || exit 1` is safe in scripts.

### Behavior to know about

- HTTPS only. A redirect is followed only to the same host; anything else is an error.
- Responses are capped at 1 MiB, HTML pages (captive portals) are reported as errors instead of being printed, and a non-2xx answer is an error.
- `public`, `local`, `where` and `all` are reserved subcommand names and currently report "not implemented".
- Changes from 0.0.1 are listed in [CHANGELOG.md](CHANGELOG.md).

### Privacy

`myip` sends one request to ipinfo.io per run, from your machine, so ipinfo.io sees your public address (that is the point). There is no telemetry and no other host is contacted.

## Development
The 0.1.0 scope and CLI contract are recorded in [ADR 0001](docs/adr/0001-product-scope-and-cli-contract.md).

The Rust toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it automatically. The minimum supported Rust version is declared as `rust-version` in `Cargo.toml`.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo deny check        # https://github.com/EmbarkStudios/cargo-deny
```

No local C toolchain? Run the same commands in a container:

```bash
docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/tmp/target rust:1.99.1 cargo test --locked
```
