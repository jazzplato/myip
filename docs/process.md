# Process

How an idea becomes a reviewed, verified change. Rules for agents and humans are in [AGENTS.md](../AGENTS.md).

## Lifecycle
```
idea -> spec -> ready -> PR -> CI gates -> human merge -> weekly health report -> new ideas
```

| Step | Who | Gate |
|---|---|---|
| `stage:idea` | anyone, agents included | none; an idea is not work |
| `stage:spec` | maintainer promotes | the Spec form is complete, with a falsifiable Verification table |
| `stage:ready` | maintainer only | spec reviewed and approved; acceptance criteria are testable |
| PR | human or agent | one `stage:ready` issue, PR template filled with evidence |
| CI gates | automated | required check `ci-gate` (fmt, clippy, test, deny, build, ...) must be green |
| Merge | maintainer only | squash merge by a human, after reading the diff and the evidence |
| Health report | scheduled (planned, #7) | weekly issue with advisories, drift, MSRV and test-quality signals; feeds new `stage:idea` issues |

`stage:blocked` marks an issue waiting on a decision or dependency.

## Labels
| Prefix | Values | Meaning |
|---|---|---|
| `stage:*` | `idea`, `spec`, `ready`, `blocked` | position in the lifecycle (exactly one) |
| `type:*` | `bug`, `feature`, `chore`, `epic`, `process` | kind of change; `process` = templates, CI, rules, settings |
| `area:*` | `cli`, `network`, `k8s`, `ci`, `deps`, `security` | where it lands |
| `risk:*` | `low`, `medium`, `high` | blast radius (see below) |

## Milestones
`Now: foundation` (the current work), `Next: correctness (0.1.0)`, `Later: new dimensions`. Move items between them deliberately.

## Risk
- `risk:low`: easy to revert, no user-visible change. May use a reduced spec.
- `risk:medium`: user-visible or hard-ish to revert.
- `risk:high`: changes a contract, the security posture or the release process. Requires:
  1. a **threat model** section in the spec (assets, trust boundaries, abuse cases);
  2. an **ADR** when the decision is hard to reverse;
  3. a **rehearsed rollback** with the evidence pasted in the PR;
  4. explicit maintainer approval of the spec before `stage:ready`, and no auto-merge.

## Protected paths and process changes
`.github/**`, `deny.toml`, `rust-toolchain.toml`, `AGENTS.md` and `docs/adr/**` change only through a `type:process` issue. These are covered by [CODEOWNERS](../.github/CODEOWNERS).

## Settings as code
Repository rules live in `.github/settings/`: the `master` and `v*` tag rulesets (JSON) and `apply.sh`, which also sets squash-only merges, a read-only default Actions token, vulnerability alerts, Dependabot security updates, secret scanning with push protection, private vulnerability reporting, and the `release` environment (maintainer as required reviewer). They are **not** applied automatically. After the CI workflow (which defines `ci-gate`) is on `master`, the maintainer runs:
```sh
.github/settings/apply.sh --dry-run   # review the plan
.github/settings/apply.sh             # apply; safe to re-run
```
To undo a ruleset, delete it in Settings -> Rules or with `gh api -X DELETE repos/OWNER/REPO/rulesets/ID`.

## What actually protects `master` (honest limitation)
GitHub cannot tell an agent that uses the maintainer's credentials from the maintainer. A `master` ruleset with no bypass actors stops accidents and unreviewed pushes, but it does not stop someone who holds the maintainer's token from merging. The real controls are:
1. **CI as an objective oracle**: `ci-gate` must pass, and tests must be able to fail (bug fixes show a test that fails on base).
2. **The human merge**: the maintainer reads the diff and evidence before clicking merge.
3. **Least-privilege workflows**: read-only default token, actions pinned by SHA, no `pull_request_target`, no secrets in PR workflows.

A separate bot identity would allow true review separation; it is a future option, not a current control.
