# AGENTS.md

Contract for every contributor, human or AI agent. The process is in [docs/process.md](docs/process.md).

## Purpose
`myip` is a small Rust CLI that prints the machine's public IP information (today via ipinfo.io). Roadmap: GitHub issues, milestones `Now` / `Next` / `Later`.

## Commands (all must pass before opening a PR)
```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo deny check
cargo build --release --locked
```
The toolchain is pinned in `rust-toolchain.toml`; do not change it outside a `type:process` issue.

If the local toolchain or linker is broken, verify in Docker with the pinned version:
```sh
V=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/tmp/target rust:"$V" cargo test --locked
```
Run the other commands the same way (`rustup component add rustfmt clippy` first; `cargo install cargo-deny --locked` for deny). The container target dir keeps `target/` out of your tree.

## Definition of done
- All commands above pass and `ci-gate` is green on the PR, which must be up to date with `master` (use "Update branch" when it is not).
- Every acceptance criterion in the issue has evidence (command + output) in the PR body.
- Bug fixes include a test that fails on the base commit and passes with the fix.
- The PR states its assumptions, tradeoffs, risks and rollback.

## Rules
1. **One issue -> one small PR.** Prefer small, reversible changes.
2. **Work only from issues labeled `stage:ready`.** Do not invent scope. Propose new ideas only as new `stage:idea` issues, never as code.
3. **Protected paths:** never edit `.github/**`, `deny.toml`, `rust-toolchain.toml`, `AGENTS.md`, `docs/adr/**` except for a `type:process` issue. Code owners review them ([CODEOWNERS](.github/CODEOWNERS)).
4. **Unit tests are hermetic:** no real network, no reliance on the host environment (proxy variables, home directory). Live-provider checks belong in a separate non-blocking canary, not in `cargo test`.
5. **Dependencies:** a new dependency needs a justification in the PR and must pass `cargo deny check`.
6. **Never weaken a check:** no `#[allow]` to silence clippy, no loosened `deny.toml`, no skipped or deleted tests. Thresholds only get stricter.
7. **Never push to `master`.** Open a PR; a human merges.
8. **Commits by agents** must end with the trailer:
   `Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>`
9. **Hard-to-reverse decisions** (CLI output contract, provider strategy, MSRV policy, dependency swaps) need an ADR first: [docs/adr/README.md](docs/adr/README.md).
10. **State assumptions** in the PR instead of guessing silently. If the issue is ambiguous or its acceptance criteria cannot be met, stop and comment on the issue.
