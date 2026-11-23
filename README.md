# myip
A small command line tool that answers "where am I on the network, and what will others see?".

`myip public` asks several independent providers for your public address, falls back when one is down, and can cross-check them. `myip local` lists this machine's network interfaces and says which address is which, and `myip where` tells you whether the process runs on a host, in a container or in a Kubernetes pod. `myip all` shows the three together. Without a subcommand `myip` still prints what [ipinfo.io](https://ipinfo.io) says about your address. The contract these commands follow is [ADR 0001](docs/adr/0001-product-scope-and-cli-contract.md).

## Install

Download the archive for your platform from the [latest release](https://github.com/jazzplato/myip/releases/latest) together with `SHA256SUMS`: Linux x86_64 and aarch64 (static musl), macOS x86_64 and aarch64, Windows x86_64. Check the archive before you run it (the file name below is an example; use the one you downloaded):

```sh
sha256sum --check --ignore-missing SHA256SUMS     # macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
gh attestation verify myip-v0.1.0-x86_64-unknown-linux-musl.tar.gz -R jazzplato/myip
```

The binaries are not notarized. If macOS refuses one that a browser downloaded, run `xattr -d com.apple.quarantine myip`; `gh release download` and `curl` do not set that flag. Or build from crates.io (needs Rust 1.85 or newer):

```sh
cargo install myip --locked
```

`myip --version` from a release binary prints the version, the short commit it was built from and that commit's date in UTC, for example `myip 0.2.0 (f02e182 2026-11-22)`, so a bug report says exactly which build it is. A build from crates.io or a plain `cargo build` has no repository to ask and prints just `myip 0.2.0`. `myip` never checks for updates.

## Usage

```
myip [OPTIONS] [FIELD]
myip [OPTIONS] public [--verify | FIELD]
myip [OPTIONS] local | where
myip [OPTIONS] all [--verify]
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

`-4` and `-6` limit the lookup to one address family. With `-6` the providers without an IPv6 address (3 and 6) are not asked and are listed as errors. With `--proxy URL` the family applies to the proxy host too, since that is the host the connection goes to: `-6 --proxy http://127.0.0.1:3128` fails with `host not found` because an IPv4 address is not an IPv6 one. Give the proxy an address of that family (`http://[::1]:3128`) or drop the flag, and when a lookup fails that way the error says so.

When the host names of two providers in a row cannot be resolved (no DNS, usually no network) and no provider has responded yet, the remaining providers are not asked: each would cost the resolver's timeout again. They are listed as not asked, so the attempt list stays complete, and the exit code is 1 as before. A timeout, a TLS error or a refused connection starts the count over, and once any provider has responded over HTTP, even with an error status, the rest are asked as usual. `--timeout` still applies to each request, and `--verify` always asks all six.

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

`--json` prints one compact line of JSON on stdout for `myip public`, also when the lookup fails (the exit code and a note on stderr still tell you). `--format json-pretty` and `--format yaml` print the same document indented or as YAML (see [Output formats](#output-formats)). Without `--verify` the document has the keys below. With `--verify` it adds `families`, one object per family that answered.

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

`FIELD` is an ipinfo.io path, optionally followed by a `?query`: `ip`, `city`, `region`, `country`, `loc`, `org`, `postal`, `timezone`, `8.8.8.8/org`, `json?token=...`, and so on. Without it you get the full JSON.

`myip public FIELD` is another spelling of `myip FIELD` (so `myip public city` prints the same as `myip city`). The answer comes from ipinfo.io alone and is not verified, unlike `myip public` without a FIELD; `public --verify FIELD` is a usage error.

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

> To use an ipinfo.io access token

```bash
myip 'json?token=YOUR_TOKEN'
```

A `?query` after the path is sent to ipinfo.io as is (allowed: letters, digits and `. _ ~ - % = & + , :`), and a single leading `/` is accepted (`myip /ip`), as in 0.0.1. `myip` never prints a query value: `-v`, error messages and usage errors show `?token=***`. If an HTTP error body from ipinfo.io repeats a value from your command line (`token abc123 rejected`), the value is shown as `***` too; a value shorter than 4 characters is not masked, and neither is one that you did not type on the command line. The token is still visible in your shell history and in the process list, so prefer a token scoped for this use.

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

### Where is this machine? `local`, `where`, `all`

A laptop has several network interfaces (Wi-Fi, Ethernet, VPN tunnels, Docker bridges) and most of them are irrelevant to "what is my IP". These commands say which address is which, and where the process runs. `local` and `where` do not contact the network; `all` asks the `public` providers for the public address and nothing else.

| Command | Answers |
|---------|---------|
| `myip local` | Every interface address with its scope (`private`, `cgnat`, `global`, `link-local`, ...), interfaces that are *likely virtual* (a guess from the name, marked as such), and the **primary egress** address: the one this machine would send from towards the internet |
| `myip where` | `host`, `container` or `kubernetes` (a pod), plus the runtime, container id, namespace, pod and node identity that can be established |
| `myip all` | The public address (as `myip public` finds it), `local` and `where` side by side, plus highlights: whether the egress address differs from the public one (NAT) and, in a pod, the pod, node, egress and public addresses together. `myip all --verify` cross-checks the public address like `public --verify`. A section that fails is reported in place (`null` values and an `error` in JSON, `unavailable` in text) and the exit code stays 0; only a `--verify` disagreement exits 3 |

Every value in `where` says how it is known: **declared** (handed to the process by an orchestrator: an environment variable or a file it wrote) or **observed** (measured here: the cgroup, a mount, a marker file, the interface list). What cannot be established is `unknown` (`null` in JSON), never a guess.

```bash
myip local   # run inside a Docker container
```
```
interfaces (observed)
  lo (likely virtual)
    127.0.0.1/8    loopback
  eth0
    172.17.0.2/16  private   primary egress (IPv4)
egress (observed: a UDP connect() towards 1.1.1.1 and 2606:4700:4700::1111, which sends nothing)
  IPv4  172.17.0.2 (private) on eth0 via 172.17.0.1
  IPv6  unknown (the egress probe found no address; -v shows why)
"likely virtual" is a guess from the interface name, not a fact.
```
```bash
myip where
```
```
environment    container
runtime        docker  (observed: file:/proc/self/mountinfo)
container id   8d5088d50c1e...  (observed: file:/proc/self/mountinfo)
evidence
  - /proc/self/mountinfo shows docker bind-mounting this container's files
  - /.dockerenv exists (Docker creates it in its containers)
```

- `--json` prints one compact JSON document with `"schema_version": 1` as the first key. Fields only ever get added within a schema version; unknown values are `null`. The text output is for people and may change. `--format json-pretty` and `--format yaml` carry the same document (see [Output formats](#output-formats)).
- `-4` / `-6` limit `local` (and the local part of `all`) to one IP family. `where` ignores them.
- `-v` lists every environment variable and file that was looked at, and why a probe or route table could not be used. For the gateway it also says that the address comes from the main routing table.
- The gateway in `local` comes from the Linux main routing table (`/proc/net/route` and `/proc/net/ipv6_route`); elsewhere it is `null`. Policy routing (`ip rule`, extra routing tables, as on multi-homed hosts, VPN clients and some CNI setups) is not visible there, so on such a host the gateway the kernel really uses for the egress address can differ from the one `myip` shows; only the main table is consulted. It is the default route on the egress interface that the kernel would pick for the egress address: for IPv6, a source-specific constraint (if any) must contain that address and the most specific source wins; then the lowest metric. A direct default (`default dev wg0`, no gateway) competes like any other, so when it wins or ties with a gateway route the gateway is `null`, never the backup's. It is also `null` when no single route applies, and when the egress address is carried by several interfaces (a virtual IP on `lo` and `eth0`): then the egress `interface` is `null` as well. `-v` says which.
- The container id that `where` reads from `/proc/self/mountinfo` (under a private cgroup namespace) comes only from a bind mount of the runtime's `hostname`, `hosts` or `resolv.conf` at this process's own `/etc/hostname`, `/etc/hosts` or `/etc/resolv.conf`. Another container's files shown at any other path, as a host-side inspection tool might, do not make the process a container. In a containerd Kubernetes pod on cgroup v2 (cgroup file `0::/`) those files are the pod sandbox's, shared by every container of the pod, so `where` reports `runtime` `containerd` (observed, from the mountinfo) and leaves the container `id` `unknown`: the sandbox id names the pod's sandbox, not this container, and is shown only in shortened form in the evidence. CRI-O and cgroup v1 nodes are not covered by this rule.
- `where` never calls the Kubernetes API and never opens the ServiceAccount token or CA certificate. It reads only the ServiceAccount `namespace` file, `/etc/resolv.conf` and a fixed list of environment variables and marker files.
- Text that other parties control (interface names, resolver and cgroup files, environment variables) is never printed raw. Control characters and characters that reorder or hide text show up as visible escapes such as `\x1b` in text output and `-v` lines, and as `\u001b` in JSON. Each such value (including OS and provider error messages) is escaped on its own, line breaks included, so it can neither end its line nor start a heading or result line of a report. An interface with an empty name is shown as `(unnamed)` in text.

#### Kubernetes pods

A pod does not know its own name, node or addresses unless you tell it. Expose them with the Downward API and `myip where` reports them as *declared*:

```yaml
env:
  - name: POD_NAME
    valueFrom: { fieldRef: { fieldPath: metadata.name } }
  - name: POD_NAMESPACE
    valueFrom: { fieldRef: { fieldPath: metadata.namespace } }
  - name: POD_IP
    valueFrom: { fieldRef: { fieldPath: status.podIP } }
  - name: POD_IPS
    valueFrom: { fieldRef: { fieldPath: status.podIPs } }
  - name: NODE_NAME
    valueFrom: { fieldRef: { fieldPath: spec.nodeName } }
  - name: HOST_IP
    valueFrom: { fieldRef: { fieldPath: status.hostIP } }
```

The variable names are a convention `myip` reads, not something Kubernetes sets by itself. Without them `where` still recognises a pod (from `KUBERNETES_SERVICE_HOST`, the ServiceAccount `namespace` file, a cluster search domain in `/etc/resolv.conf`, or a `kubepods` cgroup in `/proc/self/cgroup`, which is *observed*) and reports the missing identity as `unknown`, naming the variable that would fill it in. The Downward API variables are read for a pod recognised by any of these signals, including one with `dnsPolicy: Default` and no ServiceAccount mount. A variable such as `POD_NAME` on its own, in a plain container, is ignored.

### Output formats

`--format text|json|json-pretty|yaml` picks how `public`, `local`, `where` and `all` write their result. `text` is the default.

| Format | What you get | Stable? |
|--------|--------------|---------|
| `text` | The layout for people | No, it may change in any release |
| `json` | One compact line; the same as `--json` | Yes, byte for byte within `schema_version` 1 |
| `json-pretty` | The same document, indented | The document, not the whitespace |
| `yaml` | The same document as YAML | The document, not the whitespace |

All three document formats carry exactly the same keys and values. Parse the output (`jq`, `yq`, a YAML library) rather than matching its text. Failures keep their exit code, and the document still goes to stdout.

```bash
myip --format yaml public
```
```yaml
schema_version: 1
ip: "104.232.113.7"
family: "ipv4"
provider: "one.one.one.one"
latency_ms: 29
verified: null
error: null
attempts:
  - provider: "one.one.one.one"
    url: "https://one.one.one.one/cdn-cgi/trace"
    ip: "104.232.113.7"
    latency_ms: 29
    error: null
```

In the YAML, every string is double-quoted (so `"no"` or `"1.10"` is never read as a boolean or a number), `null`, `true`, `false` and integers are bare, and no control character is ever printed raw. `--format` cannot be combined with `--json` (use one of them), and it is a usage error with the ipinfo.io lookup (a FIELD, `public FIELD`, or no command): that answer is ipinfo.io's own body, passed through unchanged. Like the other options, a `--format` given after the command replaces one given before it.

### Colour

`--color auto|always|never` (default `auto`) colours the text of `local`, `where`, `all` and the `--verify` disagreement report. Colour only adds codes around words that are printed anyway: with the codes removed the text is exactly what `--color never` prints, so nothing is said by colour alone.

| Colour | Used for |
|--------|----------|
| Bold | Block names (`[public]`, `[local]`, `[where]`, `[highlights]`), field names, interface names, address families in a disagreement report |
| Dim | Provenance (`(declared: env:POD_IP)`), explanations and footnotes |
| Green | A verified answer, and the "no NAT" verdict |
| Yellow | Something unknown, an unverified answer, a guess (`likely virtual`), a translated address (NAT) |
| Red | A failure (`unavailable`), no answer, and the answer that differs in a disagreement |

`auto` colours only when stdout is a terminal, decided in this order (the first match wins):

1. `NO_COLOR` set to anything non-empty: no colour ([no-color.org](https://no-color.org)).
2. `CLICOLOR_FORCE` set to anything non-empty but `0`: colour, even into a pipe or a file.
3. `CLICOLOR=0`: no colour.
4. `TERM=dumb`: no colour.
5. Otherwise colour exactly when stdout is a terminal. On Windows that also needs a console known to understand the codes (`WT_SESSION`, `ANSICON`, `ConEmuANSI=ON` or a `TERM`), because `myip` does not switch the console mode.

`--color always` and `--color never` skip all of this and win over the environment; `always` also colours on a Windows console that `auto` would leave plain. The `public` headline (the bare address), the ipinfo.io lookup, every JSON and YAML document, `--help`, `-v` traces and stderr are never coloured, so `myip public` in `$(...)` and pipes always get the plain text. A `--color` given after the command replaces one given before it.

### Options

| Option | Meaning |
|--------|---------|
| `--timeout <SECS>` | Give up on a request after 1 to 600 seconds (default 10) |
| `-4` / `-6` | Use IPv4 or IPv6 only (mutually exclusive); filters `local` and `all`. With `--proxy URL` it applies to the proxy host too |
| `--proxy none\|env\|URL` | `none` (default) ignores proxy environment variables; `env` honors `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`; or give an `http://HOST:PORT` URL |
| `--json` | With `public`, `local`, `where` and `all`: one line of JSON (see above); the same as `--format json`. For the ipinfo.io lookup it changes nothing, the ipinfo.io body is already passed through unchanged |
| `--format <FORMAT>` | With `public`, `local`, `where` and `all`: `text` (default), `json`, `json-pretty` or `yaml` (see [Output formats](#output-formats)). Not with `--json`, and not with the ipinfo.io lookup |
| `--color <WHEN>` | With `local`, `where`, `all` and a `--verify` disagreement report: `auto` (default; a terminal, `NO_COLOR`, `CLICOLOR` and `CLICOLOR_FORCE` apply), `always` or `never` (see [Colour](#colour)). Never colours a document, the `public` headline, the ipinfo.io lookup, `-v` or stderr |
| `-v`, `--verbose` | Log requests to stderr; with `local`, `where` and `all`, also log what was read |
| `--verify` | Only after `public` or `all`: ask all providers and cross-check them |
Credentials in a proxy URL are never printed. They are visible to other local users on the command line, so prefer `--proxy env`.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | No answer: network, provider or HTTP error (details on stderr); for `public --verify`, also when no family could be verified. `local` exits 1 only when the interface list cannot be read; `where` always exits 0 and `all` never exits 1 |
| 2 | Usage error |
| 3 | `public --verify` or `all --verify`: the providers of an address family disagree |

With `--json` or any document `--format` a failing command still prints one parseable document (unknown values `null`, plus an `error` string), and the message goes to stderr as well. If stdout cannot be written, the exit code is 1.

The answer goes to stdout and diagnostics to stderr, so `ip=$(myip public) || exit 1` is safe in scripts. With exit code 3, stdout carries the list of who said what instead of an address, so test the exit code before using stdout as an address.

### Behavior to know about

- HTTPS only. A redirect is followed only to the same host; anything else is an error.
- In the ipinfo.io lookup responses are capped at 1 MiB (`myip public` caps a provider's answer at 4 KiB). HTML pages (captive portals) are reported as errors instead of being printed, and a non-2xx answer is an error.
- Changes from 0.0.1 are listed in [CHANGELOG.md](CHANGELOG.md).

### Privacy

`myip` sends its requests from your machine, so the provider you ask sees your public address (that is the point). The ipinfo.io lookup contacts only ipinfo.io. `myip public` (and the public section of `myip all`) contacts the providers in the table above, one at a time until one answers, or all six with `--verify`. There is no telemetry and no other host is contacted.

`one.one.one.one` and `icanhazip.com` are both served by Cloudflare, so `--verify` does not make them two independent opinions.

`local` and `where` send nothing: the egress address is found by asking the operating system to route a UDP socket towards a public address (`connect()`), which transmits no packet. They only read the interface list and the allow-listed environment variables and files above. The container id, pod name and addresses they print are not sent anywhere; `all` asks the providers for the public address only.

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
