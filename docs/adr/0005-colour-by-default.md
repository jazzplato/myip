# 0005. Colour by default: help, names and pretty documents

- Status: accepted
- Date: 2026-11-25
- Issue: #77
- Amends: [0002](0002-output-formats-and-styled-text.md) (the Colour paragraphs and "Dependencies", named under Decision)

## Context
[ADR 0002](0002-output-formats-and-styled-text.md) shipped `--color auto|always|never` in 0.2.0 for the multi-line text reports only, and deliberately left three things out: colour on `--help` ("Cannot do in 0.2.0"), colour in `json-pretty` and `yaml` ("never contain an escape byte of ours"), and any colour crate ("a dependency, licence and MSRV check for about ten fixed codes").

On 2026-11-25 the maintainer compared `cargo --help` (green bold headings, bold cyan commands and flags, cyan placeholders) with `myip` and asked for colour by default in the next release.

Measured on `origin/master` 284f589 (release build in Docker, with a pseudo-terminal):

| Output | Escape bytes today |
|---|---|
| `myip --help` | none (clap is built without its `color` feature, so help cannot be styled at all) |
| `myip local --color always` | yes: bold names, dim provenance, yellow `(likely virtual)` |
| `myip local --format json-pretty --color always` | none, by ADR 0002 |
| `myip local --format yaml --color always` | none, by ADR 0002 |

Cost of colouring help, measured in a scratch copy with `features = [..., "color"]` on clap 4.6.9:
- Eight new `Cargo.lock` entries: `anstream 1.0.0`, `anstyle-parse 1.0.0`, `anstyle-query 1.1.5`, `colorchoice 1.0.5`, `is_terminal_polyfill 1.70.2`, `utf8parse 0.2.2`, and for Windows only `anstyle-wincon 3.0.11` and `once_cell_polyfill 1.70.2`. `anstyle` is already in the tree. All are `MIT OR Apache-2.0`, none needs more than Rust 1.70, and `cargo +1.85.1 metadata` still resolves.
- `cargo deny check`: advisories, bans, licenses and sources ok, no `deny.toml` change.
- Linux release binary: 4,423,376 bytes before, 4,422,576 after (no measurable change).
- clap's `StyledStr::ansi()`, the only way to get its help with codes, exists only with the `color` feature, so there is no zero-dependency route short of re-implementing clap's help layout.

Two hazards shape the design:
- **Pipes.** Many shells and CI images export `CLICOLOR_FORCE=1` so `ls` and `grep` stay coloured through a pager. If that variable also forced colour in documents, `myip local --format json-pretty | jq` and `| yq` would break the day this ships.
- **Meaning.** In the text reports green means "verified" and yellow means "unknown". A document that coloured string values green would show `"status": "unverified"` in green.

## Decision
`--color auto|always|never` (default `auto`, no short form, no environment variable of our own) now covers four outputs. Colour is on by default whenever a person reads stdout on a terminal.

1. **Help.** `myip --help`, `-h`, and `COMMAND --help` are styled with cargo's palette: headings and `Usage:` bold green, literals (command and flag names) bold cyan, placeholders (`<FILE>`, `[OPTIONS]`) cyan. Help is static text, so no external value reaches it.
2. **Text reports** (`local`, `where`, `all`, the `--verify` disagreement report). Interface names and field names change from bold to bold cyan. Block names (`[public]`, `interfaces`, `egress`, `kubernetes`, `evidence`), address families, dim provenance and the green, yellow and red verdict spans are unchanged.
3. **Documents.** `json-pretty` and `yaml` colour keys (the whole key token, quotes included) cyan and `null` dim; every other value stays plain. No green, yellow or red in documents: there colour is syntax, never meaning. Compact `json` and `--json` are never styled: they are the machine line and keep their bytes.
4. **Unchanged.** The `public` headline address, the legacy passthrough, `--version`, and stderr (usage errors, diagnostics, the `-v` trace) stay plain, as ADR 0002 says. A usage error is plain even when stdout is a terminal.

**Colour decision.** One function, `style::enabled`, decides for help, text and documents; clap's own detection is not used.
- *Text and help* keep the ADR 0002 order (first match wins): `--color always` on; `--color never` off; `NO_COLOR` set and non-empty, off; `CLICOLOR_FORCE` set, non-empty and not `0`, on; `CLICOLOR=0`, off; `TERM=dumb`, off; otherwise on exactly when stdout is a terminal (on Windows also a console known to understand ANSI).
- *Documents* use the same order with one difference: `CLICOLOR_FORCE` does not force. Only `--color always` colours a document that is not on a terminal. So `CLICOLOR_FORCE=1 myip local --format yaml | cat` has no escape bytes, `CLICOLOR_FORCE=1 myip local | cat` has them, and `myip local --format yaml --color always | cat` has them.
- *Help and the flag.* clap stops parsing at `--help`, before `--color` may have been read, so for help only the arguments are scanned: the last `--color VALUE` or `--color=VALUE` before a bare `--` whose value is `auto`, `always` or `never` wins, anything else counts as `auto`. `myip --color never --help` and `myip --help --color never` are both plain.

**Invariants kept from ADR 0002.** For every command, outcome and format, including help, removing the SGR sequences from `--color always` output gives the `--color never` output byte for byte; words stay, colour is never the only signal. Styling follows sanitizing: strings in documents are JSON- or YAML-escaped (an ESC in a value is `\u001b`), text values are escaped by `sanitize`, so the only escape bytes are the fixed SGR codes of the palette above. `--color` still does nothing for the legacy passthrough, whose body is the raw ipinfo.io answer.

**Dependency.** clap gains the `color` feature (eight lock entries, listed under Context). Our own `Style` (about ten fixed codes) stays hand-written for the text reports and documents; clap's feature is used only to style and strip its own help. `deny.toml` does not change.

**Out of scope, deferred.** Colour on stderr, `--version`, a configurable theme (`NO_COLOR` is the only switch), 256-colour or truecolor, a `kv`/`toml` format.

## Alternatives considered
| Option | For | Against |
|---|---|---|
| Hand-render help with our own `Style` | no new crates | re-implements clap's layout and wrapping for every subcommand; drifts from `--help` content; the code is bigger than the dependency it avoids |
| A colour crate of our own (`owo-colors`, `termcolor`) for all output | one palette API | still needs clap's feature for help, so it adds crates on top of the eight |
| Leave help plain (status quo) | zero cost | the stated request is colour by default |
| Colour stderr (usage errors) too | matches cargo | diagnostics and the `-v` trace are logs kept greppable (ADR 0002); a styled error is one more thing a script can mis-parse |
| Let `CLICOLOR_FORCE` force documents | one rule for everything | breaks `\| jq`, `\| yq`, `\| kubectl apply -f -` in any environment that exports it |
| Green strings, yellow numbers in documents (`jq` style) | familiar | collides with the verdict colours: `"status": "unverified"` would be green |
| Colour compact `json` too | complete | it is the stable, scripted format; its bytes are a promise (ADR 0001, ADR 0002) |
| A new `--color` environment variable (`MYIP_COLOR`) | per-user default | `NO_COLOR`, `CLICOLOR` and `CLICOLOR_FORCE` already cover it |

## Consequences
- Easier: reading help and documents on a terminal; one palette that matches the tool the user already knows; one `enabled` function for every colour decision.
- Harder: `--color` now also affects help and documents. Removing it, or colouring `json`, is a breaking change. The help `--color` pre-scan is a second reading of one flag and has a test for every position.
- Documents: scripts that pipe `json-pretty` or `yaml` are unaffected whenever stdout is not a terminal, with or without `CLICOLOR_FORCE`. A script that passes `--color always` with a document asks for the codes.
- Text output stays explicitly not stable; palette changes to `text` need no ADR (this ADR records the change because it also touches help and documents).
- The shipped binary carries eight more crates in its dependency tree; `cargo deny` and Dependabot cover them like any other.
- Cannot do now: colour on stderr, a theme, a different palette per command.

## Reversal
Cheap until a script depends on the escape bytes. For documents no script can do that by accident: they are plain in every pipe except an explicit `--color always`. Help and text are different: an environment that exports `CLICOLOR_FORCE` already gets escape bytes from them in a pipe (ADR 0002), and now from help as well; `--color never` or `NO_COLOR` turns that off. To undo, drop the `color` feature, the pre-scan and the document styling in one release and supersede this ADR. Revisit if Windows `auto` proves too conservative (ADR 0002 already says to reconsider `anstream` then), if an environment sets `CLICOLOR_FORCE` and wants coloured documents (they can pass `--color always`), or if clap changes the `color` feature's dependency set.
