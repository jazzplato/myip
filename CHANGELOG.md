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
- `-4`/`-6` apply to `public`. With `-6` the providers without an IPv6 address (checkip.amazonaws.com, ipinfo.io) are not asked.
- `--timeout`, `--proxy`, `-4`/`-6`, `--json`, `-v`/`--verbose`, `--help` and `--version`.
- Documented exit codes: 0 success, 1 no answer, 2 usage error, 3 `--verify` disagreement.
- Reserved subcommand names `local`, `where` and `all` (they report "not implemented" until they ship).
- Proxy credentials are redacted from anything printed.

### Internal

- Library plus thin binary, with the HTTP layer behind a trait so every behavior above is tested without the network. The trait is now `Sync` and the test double `RouteFetch` answers per URL, which lets the concurrent `--verify` path be tested.
- A pure address classifier (`net::classify`, `Scope`) that tells loopback, link-local, private, CGNAT, unique-local, multicast, unspecified, documentation and global addresses apart; `public` uses it to reject non-public answers.
- A small dependency-free JSON writer (`json::Json`) with insertion-ordered objects.
- CI on Linux, macOS and Windows, plus a minimum-supported-Rust-version check, clippy, rustfmt, cargo-deny, CodeQL and a package check.

## [0.0.1] - 2019

- Initial version: prints the body of `https://ipinfo.io/<FIELD>`.
