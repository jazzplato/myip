# 0002. Output formats, styled text and the CLI contract for 0.2.0

- Status: accepted
- Date: 2026-11-22
- Issue: #46

## Context
0.1.x has one machine format (`--json`, one compact line) and one human format (plain text that is explicitly not stable). Two requests need more: people want to read and diff a document (pretty JSON), Kubernetes and CI users want YAML, and the multi-line text of `local`, `where` and `all` is hard to scan (#40). Each is a new flag, and a flag is part of the output contract of [ADR 0001](0001-product-scope-and-cli-contract.md); a colour mistake is also a way to leak escape bytes into logs, pipes and bug reports. The contract must therefore say, before any code, what the new flags may and may not do.

Measured on 0.1.1 (the released binary): `--json` is accepted before and after a subcommand, repeating it is a clap usage error, and with a FIELD (`myip city`, `myip public city`) it changes nothing because the answer is ipinfo.io's raw body. `--help` is not coloured (clap's `color` feature is off), and `-o` is unused.

## Decision
Add two global flags, extend nothing else, and keep every 0.1.x byte stream as it is by default.

**Flags**
- `--format <text|json|json-pretty|yaml>`, default `text`. No short form.
- `--json` stays and means exactly `--format json`. `--json` together with `--format`, even with the same value, is a usage error (exit `2`).
- `--color <auto|always|never>`, default `auto`. No short form, no environment variable of our own.

**Scope.** `--format` and `--color` apply to `public`, `local`, `where` and `all`. With the legacy passthrough (a FIELD, `public FIELD`, or no subcommand), `--format` is a usage error (exit `2`, no request) because the answer is ipinfo.io's own body and cannot be re-rendered; `--color` is accepted and has no effect; `--json` keeps being accepted and changing nothing, as in 0.1.x.

**One document, three renderings.** `json`, `json-pretty` and `yaml` render the same document: same keys, same order, same values, same `schema_version` (still `1`: adding a rendering does not change the document). Only compact `json` is byte-stable (the 0.1.x promise). `json-pretty` and `yaml` are parse-stable: their whitespace and layout may change in a minor release, what a JSON or YAML parser reads from them may not. Exit codes, the stdout/stderr split and the `--verify` exit `3` report are unchanged; the report is rendered in the chosen format.

**YAML shape.** Block style, two-space indent. Every string is double-quoted with the JSON escapes (a JSON string is a valid YAML double-quoted scalar), plus a `\uXXXX` escape for any character YAML does not allow in a printable scalar (the same controls JSON already escapes, U+0085, U+2028, U+2029, U+FEFF, and the non-characters U+FFFE and U+FFFF). `null`, `true`, `false` and integers are bare. Empty collections are `[]` and `{}`. A key is bare when it matches `[a-z][a-z0-9_]*` and is not one of `y n yes no on off true false null`, otherwise quoted. The output means the same under YAML 1.1 and 1.2 (no bare `no`, `on`, `1e3`, `0x1f` or `~`), and is a single document without a `---` marker.

**Colour is cosmetic.**
- Only text on stdout is ever styled, and only the multi-line reports (`local`, `where`, `all`, and the `--verify` disagreement report). The `public` headline address and the legacy passthrough are never styled. `json`, `json-pretty`, `yaml`, `--help`, stderr diagnostics and the `-v` trace never contain an escape byte of ours, whatever `--color` says.
- For every command and outcome, removing the SGR sequences from `--color always` output yields the `--color never` output byte for byte. The words stay: colour is never the only signal.
- Styling is applied after untrusted values are sanitized, so the only escape bytes that can reach a terminal are our own fixed SGR codes (bold, dim, green, yellow, red). No 256-colour, truecolor or configuration.

**Colour decision order** (first match wins): `--color always` on; `--color never` off; `NO_COLOR` set and non-empty, off; `CLICOLOR_FORCE` set, non-empty and not `0`, on; `CLICOLOR=0`, off; `TERM=dumb`, off; otherwise on exactly when stdout is a terminal. On Windows `auto` additionally requires a terminal known to understand ANSI sequences (`WT_SESSION`, `ANSICON`, `ConEmuANSI=ON` or `TERM` set), because `myip` does not switch the console mode; `always` colours regardless.

**Dependencies.** None. YAML and SGR codes are written by hand, behind one function each, and checked in tests against a real YAML parser (a test-only dependency, never in the shipped binary).

**Out of scope, deferred.** `toml` (no null, so a lossy rendering with a different shape, and no consumer yet), `kv`/`env` (needs a documented quoting rule and a "never `eval` this" warning; its own idea), `-o/--output`, `--short`, a selector such as `--get public.ip` (use `jq`), colour themes and a pager.

## Alternatives considered
| Option | For | Against |
|---|---|---|
| `-o/--output` like kubectl | familiar to Kubernetes users, short | `-o` is a common short flag and colliding with a later meaning is costly; `--format` is clearer; `-o` can be added later as an alias without a break, so not now |
| `--json` only, plus `--pretty` | smallest change | no path to YAML or other formats; two interacting flags |
| Silently ignore `--format` on the legacy passthrough | no new error | printing JSON when `--format yaml` was asked is a trap in scripts |
| Accept `--json` together with `--format` | lenient | two ways to say it with possible conflict (`--json --format yaml`); an error is clearer |
| A YAML crate | correct by construction | `serde_yaml` is archived, `serde_yml` has an unsoundness advisory, both pull `serde`; the output we need is a small subset |
| Unquoted YAML strings | prettier | `no`, `on`, `1e3`, leading `*`, `: ` and `#` change meaning across parsers |
| `toml` now | requested in conversation | no null, so a different document; lossy; no consumer |
| A colour crate (`anstream`, `termcolor`, `owo-colors`) | handles Windows console quirks | a dependency, licence and MSRV check for about ten fixed codes |
| Colour stderr too | consistent look | diagnostics and the `-v` trace are logs: keep them greppable and stable |
| Post-process finished text with regexes | no renderer changes | guesses meaning from words, breaks when text changes, can mis-colour hostile values |

## Consequences
- Easier: reading (`json-pretty`, colour), Kubernetes and CI consumption (`yaml`), and review of renderer changes (one document tree, three writers; strip-equals-plain is a testable property).
- Harder: `--format` and `--color` are now promises. Removing either, or changing the meaning of a value, is a breaking change. `json-pretty` and `yaml` layout is not a byte promise, and the README says so.
- A user who diffs the `yaml` text instead of parsing it can be surprised by a layout change in a minor release; the ADR is explicit that only compact `json` is byte-stable.
- Text output stays explicitly not stable: colour and layout changes to `text` need no ADR.
- Cannot do in 0.2.0: `toml`, `kv`/`env`, selectors, colour on `--help`, a configurable theme.

## Reversal
Docs-only until the code PRs land. Before 0.2.0 ships, supersede this ADR. After it ships, removing `--format` or `--color` is a breaking change for scripts that pass them; the cost is a deprecation cycle. Revisit if a consumer needs a shape the document tree cannot express (then `kv` or `toml` get their own ADR), or if Windows `auto` proves too conservative (then reconsider `anstream`).
