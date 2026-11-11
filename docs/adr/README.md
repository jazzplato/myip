# Architecture Decision Records

Short records of decisions that are expensive to undo. Copy [0000-template.md](0000-template.md) to `NNNN-short-title.md` (next free number), open it in a `type:process` PR, and link it from the spec.

## When an ADR is required
Write one **before** implementing a decision that is hard to reverse:
- the CLI output contract (subcommands, flags, exit codes, stdout vs stderr, JSON shape);
- provider strategy (which services are queried, quorum or fallback behavior, trust of responses);
- MSRV policy;
- swapping or adding a core dependency (HTTP client, TLS stack, CLI parser);
- anything labeled `risk:high`.

Not needed for bug fixes, refactors or reversible tweaks.

## Index
| # | Title | Status |
|---|---|---|
| 0000 | [Template](0000-template.md) | n/a |
| 0001 | [Product scope and CLI contract for 0.1.0](0001-product-scope-and-cli-contract.md) | accepted |
