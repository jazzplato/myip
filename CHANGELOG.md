# Changelog

All notable changes to this project are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed (behavior differences from 0.0.1)

- Errors go to stderr with a non-zero exit code; stdout carries only the answer. Previously some failures printed an error body to stdout and exited 0.
- A non-2xx answer from ipinfo.io is an error (exit 1), including 404 for an unknown field.
- An HTML response (a captive portal or proxy page) is an error instead of being printed.
- `FIELD` is validated: letters, digits and `. _ : / -` only, not empty, no `..`, no leading `/`, no `//`. Anything else is a usage error (exit 2) and no request is made. `myip ip city` (two fields) is now a usage error; run `myip city`.
- The output always ends with a newline.
- The default timeout is 10 seconds (was 30) and is configurable with `--timeout`.
- The response size cap is 1 MiB (was 10 MiB).
- Redirects are followed only to the same host over HTTPS (at most 5). A redirect to another host or a downgrade to `http` is an error.
- Bytes that are not valid UTF-8 in a response are shown as U+FFFD instead of failing; a non-Unicode command-line argument is a usage error (exit 2) instead of a panic.
- Proxy environment variables are still ignored by default, as in 0.0.1; opt in with `--proxy env`.

### Added

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
- CI on Linux, macOS and Windows, plus a minimum-supported-Rust-version check, clippy, rustfmt, cargo-deny, CodeQL and a package check.

## [0.0.1] - 2019

- Initial version: prints the body of `https://ipinfo.io/<FIELD>`.
