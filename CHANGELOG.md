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

- `--timeout`, `--proxy`, `-4`/`-6`, `--json`, `-v`/`--verbose`, `--help` and `--version`.
- Documented exit codes: 0 success, 1 no answer, 2 usage error (3 is reserved for `--verify`).
- Reserved subcommand names `public`, `local`, `where` and `all` (they report "not implemented" until they ship).
- Proxy credentials are redacted from anything printed.

### Internal

- Library plus thin binary, with the HTTP layer behind a trait so every behavior above is tested without the network.
- CI on Linux, macOS and Windows, plus a minimum-supported-Rust-version check, clippy, rustfmt, cargo-deny, CodeQL and a package check.

## [0.0.1] - 2019

- Initial version: prints the body of `https://ipinfo.io/<FIELD>`.
