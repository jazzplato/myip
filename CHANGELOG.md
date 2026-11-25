# Changelog

All notable changes to this project are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- The commands in the README and `docs/` paste as is: no trailing `# comment` (an interactive zsh, the default shell on macOS, passes it to the command), and the verify block no longer names a release that does not exist. A test (`tests/docs.rs`) keeps it that way.

## [0.4.0] - 2026-11-25

### Changed

- With colour on (`--color always`, or `auto` on a terminal), the field names of `myip where` and `myip all` and the interface names of `myip local` and `myip all` are now bold cyan (one `ESC[1;36m` sequence, as cargo writes its literals) instead of plain bold, so they stand out from the values ([ADR 0005](docs/adr/0005-colour-by-default.md)). Block names, address families, provenance and the green, yellow and red verdicts keep their colours. With the codes removed the text is still exactly what `--color never` prints, and `--color never`, `NO_COLOR`, a pipe and compact JSON are byte for byte what 0.3.0 printed.
- `myip --help`, `-h` and `myip COMMAND --help` are now coloured like `cargo --help` on a terminal: headings and `Usage:` in bold green, command and option names in bold cyan, placeholders in cyan ([ADR 0005](docs/adr/0005-colour-by-default.md)). The decision is the one of the text reports (`--color`, then `NO_COLOR`, `CLICOLOR_FORCE`, `CLICOLOR=0`, `TERM=dumb`, then whether stdout is a terminal), so a pipe, a file and `NO_COLOR=1` still get plain help. `--color` is read from the arguments for help, in any position: `myip --help --color never` is plain and `myip public --help --color always` is coloured. With the codes removed the help is the plain help, byte for byte, and its words are those of 0.3.0 except the line of the `--color` option, which now says what it colours. `--version` and usage errors (stderr) stay plain. clap gains its `color` feature, which adds eight crates to `Cargo.lock` (`anstream`, `anstyle-parse`, `anstyle-query`, `colorchoice`, `is_terminal_polyfill`, `utf8parse`, and for Windows only `anstyle-wincon` and `once_cell_polyfill`; all `MIT OR Apache-2.0`); the release binary does not grow.
- `--format json-pretty` and `--format yaml` now colour their keys (cyan, quotes included, the colon left out) and their `null` values (dim) on a terminal, and with `--color always` anywhere ([ADR 0005](docs/adr/0005-colour-by-default.md)). With the codes removed the document is byte for byte what `--color never` prints, and a string value `"null"` is never dimmed. `NO_COLOR`, `CLICOLOR=0`, `TERM=dumb` and `--color never` turn it off. **`CLICOLOR_FORCE` does not colour a document**: only `--color always` forces one into a pipe, so a script that exports `CLICOLOR_FORCE` still gets a document `jq` and `yq` can read. Compact `json` (`--json`), `-v` traces, stderr and the legacy ipinfo.io passthrough are never coloured. A script that pipes or captures a document is unaffected, because stdout is then not a terminal.

## [0.3.0] - 2026-11-24

### Changed

- `myip public --verify` (and `myip all --verify`) now counts independent operators, not URLs ([ADR 0003](docs/adr/0003-operator-quorum.md)). Each provider has an operator (`one.one.one.one` and `icanhazip.com` are both `cloudflare`, the other four are their own), and a family is `verified` only when answers from at least two distinct operators were received and all answers agree. Two answers from one operator, which used to be `verified`, are now `unverified`. Any two different answers still make a family `disagree` (exit 3), and 5 of 6 agreeing is still a disagreement.
- **Behaviour change: a run where only Cloudflare answered (providers 1 and 4) goes from exit 0 to exit 1.** It prints nothing on stdout and says on stderr `the address could not be verified: fewer than two independent operators answered in any address family`. The same applies to `-6 --verify`, which now needs two of Cloudflare, ipify and ident.me. `all --verify` is unchanged: it reports the unverifiable result in band and still exits 0. A family that is `unverified` next to a verified one still does not fail the run.
- The documented meaning of the JSON `status` is tightened without a `schema_version` bump, as an explicit exception that ADR 0003 records for this one change: `verified` is "answers from at least two operators, all agree" and `unverified` is "answers from fewer than two operators, all agree", which can now have `total` 2 or more. The field names, the three values and the exit codes are unchanged. Read `status` or the exit code; do not infer it from `agree == total >= 2`. `agree` and `total` still count answers.
- The text report of a disagreement now says `verified, N of M agree, K operators` and `unverified, N answers from K operators`.
- A host that exports `IPINFO_TOKEN` (a CI job, a container) now authenticates every `myip`, `myip FIELD` and `myip public FIELD` call with it, where those calls were anonymous before. `myip public` (without a FIELD), `public --verify`, `local`, `where` and `all` never read the variable and never send it. Unset the variable to go back to anonymous requests.

### Added

- `--verify` documents (`--json`, `--format json-pretty`, `--format yaml`) gain `families[].operators`, the number of distinct operators among a family's answers, and `families[].providers[].operator`, the operator of each answer. The README lists the operator of each provider.
- `myip`, `myip FIELD` and `myip public FIELD` (the ipinfo.io lookup) read the `IPINFO_TOKEN` environment variable and send its value as an `Authorization: Bearer` header on the one request, so a token no longer has to be written as `?token=` in your shell history, the process list or a CI log (ADR 0004). The header is not sent on a redirect that `myip` follows, and a redirect to another host or to `http` is still an error. A `token` parameter in FIELD (name matched ignoring case, any value, even an empty one) wins: the variable is then neither read nor checked. Spaces around the value are ignored; an unset, empty, blank or non-Unicode value means no token, as before; otherwise it must have 1 to 256 characters, each a letter, a digit or one of `. _ ~ + / = -`, and anything else is a usage error (exit code 2) that names the variable and the rule but never shows the value, and no request is made. `-v` prints the fixed line `myip: Authorization: Bearer *** (from IPINFO_TOKEN)`, and an error message or error body that repeats the value shows `***`, also when the body is cut to its length limit. A successful answer is ipinfo.io's and is printed as it came. The README has a short section.

## [0.2.1] - 2026-11-23

### Added

- When every request of `myip public` or of the ipinfo.io lookup (`myip`, `myip FIELD`) fails because the system has no CA certificates (the transport error `No CA certificates were loaded from the system`, typical of a minimal container image), `myip` now prints one more line to stderr after the error and the attempt list: `hint: no CA certificates were found; install ca-certificates, or set SSL_CERT_FILE or SSL_CERT_DIR to a bundle`. It is printed once, never for other failures, and never in a `--json`, `--format` document. The exit code and stdout do not change. The README has a short troubleshooting note.

### Changed

- `myip public` (and the public part of `myip all`) no longer waits for six failed lookups when there is no name resolution, for example in a container without a network, where it took about 30 seconds to exit 1. Once the host names of two providers in a row could not be resolved and no provider has responded yet, the other providers are not asked; stdout is still empty, the exit code is still 1, and the stderr attempt list shows them as `not asked`. A timeout, a TLS error or a refused connection starts the count over, any HTTP answer turns the early stop off for the run, `--timeout` still applies to each request, and `--verify` still asks all six at once. Recognising a failed lookup needs the text std gives it on Unix (Linux, macOS); on Windows nothing changes.

### Fixed

- An HTTP error body that repeats a query value from the command line is no longer printed with that value. `myip 'json?token=abc123'` against a server that answers 403 `{"error":"token abc123 rejected"}` used to print the body as it was, and now prints `{"error":"token *** rejected"}` (the `HTTP 403` and the rest of the body are unchanged). Each `&`-separated value of every `?query` on the command line is masked, also in its percent-decoded form and before the body is cut to its length limit, whatever its length (a short value such as `1` also hides every `1` of the body, which is the safe direction). `myip public` never prints a provider's body, so it has nothing to mask.
- `-4` or `-6` with `--proxy URL` also restricts the proxy host, so `myip -6 --proxy http://127.0.0.1:3128` could only fail with a bare `host not found`. When the lookup of the host name fails, the error now adds a hint to give the proxy an address of that family (`http://[::1]:PORT`) or drop the flag, once per run; there is no hint without `-4` or `-6`, or with `--proxy none` or `--proxy env`. A host with no address of the wanted family (`host not found`) is now told apart from other transport errors; it still does not count towards the early stop of the offline lookup, which is only for a name that cannot be resolved at all. Which hop the family applies to is unchanged.
- `myip where` in a containerd Kubernetes pod on cgroup v2 (where `/proc/self/cgroup` is just `0::/`) now reports `container.runtime` `containerd` (observed from `/proc/self/mountinfo`, with an evidence line naming the pod sandbox) instead of an empty `container` section. `container.id` stays `null`: containerd's CRI mounts the pod sandbox's `hostname` and `resolv.conf`, which every container of the pod shares, so no container id can be read from them. Checked against a real `kind` v0.22.0 pod (Kubernetes v1.29.2, containerd, cgroup v2); CRI-O and cgroup v1 nodes are unchanged. No JSON field is added and `schema_version` stays 1.
- `myip where` reads a container id from `/proc/self/mountinfo` only from a bind mount of the runtime's `hostname`, `hosts` or `resolv.conf` at this process's own `/etc/hostname`, `/etc/hosts` or `/etc/resolv.conf`. A host-side tool that shows another container's files at another path (an inspection tool) is no longer reported as running in that container. Every real container mounts those files at those paths, so detection there is unchanged; the JSON schema is unchanged.
- A 6to4 address (`2002::/16`) is now classified by the IPv4 address it embeds, so a provider answer such as `2002:0a00:0001::1` (embedding `10.0.0.1`) is rejected instead of being accepted as a public address; `myip local` and `myip where` show the embedded address's scope for such an interface address. `myip public` also trims only ASCII whitespace around an answer, so an answer padded with a no-break space, or a Cloudflare trace line `ip= 8.8.8.8`, is rejected. The error that `myip FIELD` prints for a failed request (the transport error or the body of an HTTP error) shows control characters such as ESC as visible escapes (`\x1b`) instead of passing them to the terminal.

### Documentation

- The README and the `-v` notes of `myip local` say that the gateway comes from the main routing table (`/proc/net/route`, `/proc/net/ipv6_route`) and that policy routing (`ip rule`, other tables) is not consulted. `-v` now also notes where a reported gateway came from; nothing else about the output changes.

## [0.2.0] - 2026-11-22

### Added

- `--format text|json|json-pretty|yaml` chooses how `public`, `local`, `where` and `all` write their result. `json-pretty` is the `--json` document indented, and `yaml` is the same document as block-style YAML in which every string is double-quoted and no control character is printed raw. `--json` is now exactly `--format json` and its output is unchanged byte for byte; combining the two is a usage error (exit 2), as is `--format` with the ipinfo.io lookup (`FIELD`, `public FIELD`, or no command), before any request. Exit codes and the stdout/stderr split do not change. Only `json` is stable byte for byte; `json-pretty` and `yaml` promise the document, not the whitespace. See [ADR 0002](docs/adr/0002-output-formats-and-styled-text.md).
- `--color auto|always|never` (default `auto`) colours the text of `local`, `where`, `all` and the `--verify` disagreement report: bold names, dim provenance, green for verified and "no NAT", yellow for unknown, unverified, guesses and a translated address, red for failures and the answer that differs. With the codes removed the text is exactly what 0.1.1 printed, so every word stays. `auto` colours only when stdout is a terminal and honours `NO_COLOR`, `CLICOLOR=0`, `CLICOLOR_FORCE` and `TERM=dumb`; on Windows it also needs a console known to understand ANSI codes. Nothing else changes: pipes and files get the same plain text as before, and the `public` headline, the ipinfo.io lookup, JSON and YAML documents, `--help`, `-v` and stderr are never coloured. Only six fixed SGR codes are written, and text from the network or the machine is still shown with its control characters escaped. See [ADR 0002](docs/adr/0002-output-formats-and-styled-text.md).
- `myip --version` in a release binary also shows the short commit it was built from and that commit's date in UTC, for example `myip 0.2.0 (f02e182 2026-11-22)`. A build without that information (`cargo install myip`, a plain `cargo build`) still prints just `myip 0.2.0`. The release workflow now checks the exact text on all five targets. There is no build timestamp and no update check.

### Internal

- New test-only dependency `yaml-rust2` (MIT OR Apache-2.0) parses every YAML rendering back in the tests, so the hand-written YAML writer is checked against a real parser. It and its five transitive crates are in `Cargo.lock` but not in the shipped binary.
- `src/style.rs` (colour) and `src/yaml.rs` (YAML) are new, with no new runtime dependency. The release workflow checks the exact `--version` text on all five targets.

## [0.1.1] - 2026-11-22

### Added

- `myip public FIELD` is accepted as another spelling of `myip FIELD` (for example `myip public city`, which prints what `myip city` prints). It is exactly the ipinfo.io lookup, with the same FIELD rules, options and output: the answer comes from ipinfo.io alone and is not verified. `myip public --verify FIELD` and a FIELD both before and after `public` stay usage errors (exit 2, no request). `myip public` without a FIELD is unchanged. In 0.0.1 `myip public city` asked ipinfo.io for `/public` and printed an HTML page; in 0.1.0 it was a usage error.

## [0.1.0] - 2026-11-22

### Changed (behavior differences from 0.0.1)

- HTTPS uses [ureq](https://crates.io/crates/ureq) with rustls and the operating system's trust store instead of reqwest 0.9 with the platform TLS library (OpenSSL on Linux), so the Linux release binaries are static. `SSL_CERT_FILE` and `SSL_CERT_DIR` are honored. As with 0.0.1, an image without a CA bundle (`scratch`, a slim image without `ca-certificates`) cannot make HTTPS requests.
- Builds with Rust 1.85 or newer (edition 2024); 0.0.1 used edition 2018.
- Errors go to stderr with a non-zero exit code; stdout carries only the answer. Previously some failures printed an error body to stdout and exited 0.
- A non-2xx answer from ipinfo.io is an error (exit 1), including 404 for an unknown field.
- An HTML response (a captive portal or proxy page) is an error instead of being printed.
- `FIELD` is validated: the path may use letters, digits and `. _ : / -` only, no `..`, no `//`; one leading `/` is accepted and dropped, and a `?query` of letters, digits and `. _ ~ - % = & + , :` may follow (ipinfo.io's `?token=...`). Query values are never printed: `-v`, error messages and usage errors show `?token=***`. Anything else, including an empty FIELD, is a usage error (exit 2) and no request is made. `myip ip city` (two fields) is now a usage error; run `myip city`.
- The output always ends with a newline.
- The default timeout is 10 seconds (was 30) and is configurable with `--timeout`.
- The response size cap is 1 MiB (was 10 MiB).
- Redirects are followed only to the same host over HTTPS (at most 5). A redirect to another host or a downgrade to `http` is an error.
- Bytes that are not valid UTF-8 in a response are shown as U+FFFD instead of failing; a non-Unicode command-line argument is a usage error (exit 2) instead of a panic.
- Proxy environment variables are still ignored by default, as in 0.0.1; opt in with `--proxy env`.

### Added

- Release binaries for Linux (x86_64 and aarch64, static musl), macOS (x86_64 and aarch64) and Windows (x86_64) on the GitHub Releases page, with `SHA256SUMS` and a build provenance attestation for each file (see [docs/releasing.md](docs/releasing.md)).
- `myip public`: the public address from six providers in a fixed order (one.one.one.one, api64.ipify.org, checkip.amazonaws.com, icanhazip.com, ident.me, ipinfo.io). A timeout, a non-2xx answer, a body over 4 KiB, an HTML page, or an answer that is not exactly one public address of the requested family is an error and the next provider is asked; if all fail the exit code is 1 and every attempt is listed on stderr.
- `myip public --verify`: asks all providers concurrently and cross-checks them per address family. At least two agreeing answers in a family verify it (exit 0); a split exits 3 and lists who said what on stdout; fewer than two answers in every family exits 1.
- `myip --json public`: one compact JSON line (`schema_version` 1) with the headline answer, every attempt and, with `--verify`, the per-family result.
- `-4`/`-6` apply to `public`, `local` and `all`. With `-6` the providers without an IPv6 address (checkip.amazonaws.com, ipinfo.io) are not asked.
- `myip local`: interface addresses with their scope, likely-virtual interfaces and the primary egress address (found with a UDP `connect()` that sends nothing) and its gateway on Linux. The gateway is the default route on the egress interface whose source constraint (IPv6 source-specific routes) contains the egress address, the most specific first and then the lowest metric. A direct default (`default dev wg0`) takes part in that ranking: if it wins or ties with a gateway route the gateway is `null`, not the backup's. When no single route applies, or the egress address is carried by several interfaces (a virtual IP on `lo` and `eth0`, so the egress interface is `null` too), the gateway is `null`. An egress probe that finds no address is reported as `unknown`, not "no route": a sandbox can deny the probe.
- `myip where`: host, container or Kubernetes pod, with runtime, container id, namespace and pod/node identity; every value is labelled declared or observed and unknown values are `null`. A pod is recognised from `KUBERNETES_SERVICE_HOST`, the ServiceAccount `namespace` file, a cluster search domain in `/etc/resolv.conf`, or (observed) a `kubepods` cgroup, so a pod with `dnsPolicy: Default` and no ServiceAccount mount is still a pod. It never calls the Kubernetes API and never opens the ServiceAccount token.
- `myip all`: the public address (the `public` lookup), `local` and `where` together, with NAT and pod highlights. `myip all --verify` cross-checks the public address like `public --verify` and exits 3 on a disagreement. `all` never exits 1: a failed section is reported in place (`null` values and an `error`).
- `--json` for these commands: one compact document, `schema_version` first.
- Documented Pod spec snippet that exposes the Downward API variables `where` reads (see the README).
- `--timeout`, `--proxy`, `-4`/`-6`, `--json`, `-v`/`--verbose`, `--help` and `--version`.
- Documented exit codes: 0 success, 1 no answer, 2 usage error, 3 `--verify` disagreement.
- Proxy credentials are redacted from anything printed.
- Text that comes from outside the process (interface names, `/etc/resolv.conf`, environment variables, container files) never reaches a terminal raw: control characters (ESC, NUL, DEL, the C1 range) and characters that reorder or hide text are shown as visible escapes such as `\x1b` in text output and in `-v` lines, and as `\uXXXX` in JSON. Each such value (including OS and provider error messages) is escaped on its own, line breaks included, so it can neither end its line nor start a heading or result line of a report. Cluster domains and namespaces must also be plain lowercase DNS names to count as a signal.

### Internal

- New dependency `if-addrs` (MIT OR BSD-3-Clause) to list network interfaces portably.
- Library plus thin binary, with the HTTP layer behind a trait so every behavior above is tested without the network. The trait is now `Sync` and the test double `RouteFetch` answers per URL, which lets the concurrent `--verify` path be tested. The interfaces, the environment and the egress probe are traits too, so `local`, `where` and `all` are tested against fixtures.
- A pure address classifier (`net::classify`, `Scope`) that tells loopback, link-local, private, CGNAT, unique-local, multicast, unspecified, documentation and global addresses apart; `public` uses it to reject non-public answers and `local` to label interface addresses.
- A small dependency-free JSON writer (`json::Json`) with insertion-ordered objects.
- CI on Linux, macOS and Windows, plus a minimum-supported-Rust-version check, clippy, rustfmt, cargo-deny, CodeQL, dependency review and a package check.
- The toolchain is pinned in `rust-toolchain.toml` (1.100.0 at this release) and Dependabot keeps it, the cargo dependencies and the GitHub Actions up to date.
- A tag-triggered release workflow that builds the five binaries, checks that the tag equals the `Cargo.toml` version and that `cargo publish --dry-run` passes, and creates a draft release; a maintainer publishes it.

## [0.0.1] - 2019

- Initial version: prints the body of `https://ipinfo.io/<FIELD>`.

[Unreleased]: https://github.com/jazzplato/myip/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/jazzplato/myip/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jazzplato/myip/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/jazzplato/myip/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/jazzplato/myip/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/jazzplato/myip/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/jazzplato/myip/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/jazzplato/myip/releases/tag/v0.0.1
