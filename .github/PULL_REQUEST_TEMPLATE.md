<!-- One issue -> one small PR. Delete guidance comments. Keep it short. -->

Closes #N

## Goal
<!-- One or two sentences, copied from the spec. -->

## Design summary
<!-- What changed and why this approach. Link the ADR if one applies. -->

## Tradeoffs
<!-- What this makes worse, and the alternatives you rejected. -->

## Risks
<!-- What could break, and how you would notice. State assumptions you made. -->

## Evidence
<!-- One row per acceptance criterion in the issue. Paste real command output, not "passes". -->

| # | Command | Output / result |
|---|---|---|
| 1 |  |  |

<!-- Bug fixes only: show the new test FAILS on the base commit.
     e.g. `git stash`/checkout base, run the test, paste the failure; then paste it passing with the fix. -->

**Test fails on base (bug fixes):** <!-- command + failing output, or "n/a" -->

## Rollback
<!-- Exact steps to undo this (usually `git revert <squash commit>`) and any state left behind. -->

## Checklist
- [ ] Linked issue is `stage:ready` (or this is a `type:process` change approved by the maintainer)
- [ ] fmt, clippy, test, deny, release build pass (`ci-gate` is green)
- [ ] Protected path touched (`.github/**`, `deny.toml`, `rust-toolchain.toml`, `AGENTS.md`, `docs/adr/**`)? Only allowed under a `type:process` issue
- [ ] New dependency? Justified here and passes `cargo deny check`
- [ ] No real network in unit tests
- [ ] No check was weakened (lint levels, deny rules, test thresholds)
