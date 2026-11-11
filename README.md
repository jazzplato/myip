# myip
A simple command line tool to get the ip related information for your host.

## Usage
Currently `myip` only serves as a command line interface to [ipinfo](https://ipinfo.io) to retrieve the public ip related information.

> To get public ip

```bash
myip ip
```
```
12.166.238.132
```

> To get your current city

```bash
myip ip city
```
```
Chicago
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
