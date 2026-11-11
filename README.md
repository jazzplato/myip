# myip
A small command line tool that answers "where am I on the network, and what will others see?".

`myip public` asks several independent providers for your public address, falls back when one is down, and can cross-check them. Without a subcommand `myip` still prints what [ipinfo.io](https://ipinfo.io) says about your address. Local interfaces and container/Kubernetes identity are planned; the contract they will follow is [ADR 0001](docs/adr/0001-product-scope-and-cli-contract.md).

## Usage

```
myip [OPTIONS] [FIELD]
myip [OPTIONS] public [--verify]
```

### Your public address: `myip public`

```bash
myip public
```
```
104.232.113.7
```

The providers are asked one at a time, in this order, until one gives a valid answer:

| # | Provider | URL | IPv6 |
|---|----------|-----|------|
| 1 | one.one.one.one | `https://one.one.one.one/cdn-cgi/trace` | yes |
| 2 | api64.ipify.org | `https://api64.ipify.org` | yes |
| 3 | checkip.amazonaws.com | `https://checkip.amazonaws.com` | no |
| 4 | icanhazip.com | `https://icanhazip.com` | yes |
| 5 | ident.me | `https://ident.me` | yes |
| 6 | ipinfo.io | `https://ipinfo.io/ip` | no |

A provider's answer counts only if it is a 2xx response of at most 4 KiB that holds exactly one IP address, and that address is a public one. HTML, an empty body, a private, loopback, link-local or other non-public address, and an address of the wrong family are errors, and the next provider is asked. If every provider fails, nothing goes to stdout, the exit code is 1, and each provider's error is listed on stderr. A provider's answer text is never echoed, only a short reason.

`-4` and `-6` limit the lookup to one address family. With `-6` the providers without an IPv6 address (3 and 6) are not asked and are listed as errors.

> To cross-check the answer

```bash
myip public --verify
```

`--verify` asks all six providers at the same time and groups the answers per address family. A family is verified when at least two providers answered and all of them agree. The address of the verified family is printed and the exit code is 0, even if a provider failed or the other family had only one answer. If both families are verified, the one whose first answer came from the earlier provider in the table is printed. If the providers of one family give different addresses the exit code is 3 and the answers are listed on stdout, so you can see who said what:

```
ipv4: DISAGREE, 2 of 3 agree
  one.one.one.one        1.2.3.4
  api64.ipify.org        5.6.7.8  <- differs
  checkip.amazonaws.com  1.2.3.4
no answer:
  icanhazip.com          timed out after 10s
```

If fewer than two providers answered in every family, the address cannot be verified and the exit code is 1.

> To see every attempt

```bash
myip -v public --verify
```
```
104.232.113.7
myip: one.one.one.one: 104.232.113.7 (34 ms)
myip: api64.ipify.org: 104.232.113.7 (126 ms)
myip: checkip.amazonaws.com: 104.232.113.7 (248 ms)
myip: icanhazip.com: 104.232.113.7 (36 ms)
myip: ident.me: 104.232.113.7 (527 ms)
myip: ipinfo.io: 104.232.113.7 (83 ms)
```

> To get a machine-readable result

`--json` prints one compact line of JSON on stdout for `myip public`, also when the lookup fails (the exit code and a note on stderr still tell you). Without `--verify` the document has the keys below. With `--verify` it adds `families`, one object per family that answered.

```bash
myip --json public
```
```json
{"schema_version":1,"ip":"104.232.113.7","family":"ipv4","provider":"one.one.one.one","latency_ms":29,"verified":null,"error":null,"attempts":[{"provider":"one.one.one.one","url":"https://one.one.one.one/cdn-cgi/trace","ip":"104.232.113.7","latency_ms":29,"error":null}]}
```

| Key | Value |
|-----|-------|
| `schema_version` | `1` |
| `ip`, `family`, `provider`, `latency_ms` | The headline answer: the address, `ipv4` or `ipv6`, the provider that gave it and how long it took. `null` when there is none (exit 1 or 3). |
| `verified` | `{"agree": N, "total": M}` for the headline family with `--verify`, otherwise `null` |
| `error` | The reason when there is no headline, otherwise `null` |
| `attempts` | One object per provider in ADR order: `provider`, `url`, `ip`, `latency_ms` (`null` when not asked) and `error` |
| `families` | Only with `--verify`: `family`, `status` (`verified`, `unverified` or `disagree`), `ip` (`null` on disagree), `agree`, `total` and the `providers` that answered |

### ipinfo.io lookup

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
| `--json` | With `myip public`, print one line of JSON (see above). For the ipinfo.io lookup it changes nothing, the ipinfo.io body is already passed through unchanged |
| `-v`, `--verbose` | Log requests to stderr |
| `--verify` | Only after `public`: ask all providers and cross-check them |

Credentials in a proxy URL are never printed. They are visible to other local users on the command line, so prefer `--proxy env`.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | No answer: network, provider or HTTP error (details on stderr); for `public --verify`, also when no family could be verified |
| 2 | Usage error |
| 3 | `public --verify`: the providers of an address family disagree |

The answer goes to stdout and diagnostics to stderr, so `ip=$(myip public) || exit 1` is safe in scripts. With exit code 3, stdout carries the list of who said what instead of an address, so test the exit code before using stdout as an address.

### Behavior to know about

- HTTPS only. A redirect is followed only to the same host; anything else is an error.
- In the ipinfo.io lookup responses are capped at 1 MiB (`myip public` caps a provider's answer at 4 KiB). HTML pages (captive portals) are reported as errors instead of being printed, and a non-2xx answer is an error.
- `local`, `where` and `all` are reserved subcommand names and currently report "not implemented".
- Changes from 0.0.1 are listed in [CHANGELOG.md](CHANGELOG.md).

### Privacy

`myip` sends its requests from your machine, so the provider you ask sees your public address (that is the point). The ipinfo.io lookup contacts only ipinfo.io. `myip public` contacts the providers in the table above, one at a time until one answers, or all six with `--verify`. There is no telemetry and no other host is contacted.

`one.one.one.one` and `icanhazip.com` are both served by Cloudflare, so `--verify` does not make them two independent opinions.

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
