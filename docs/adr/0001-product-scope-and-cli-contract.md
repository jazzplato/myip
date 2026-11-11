# 0001. Product scope and CLI contract for 0.1.0

- Status: accepted
- Date: 2026-11-10
- Issue: #12

## Context
`myip` prints the body of `https://ipinfo.io/<arg>` and nothing else. "What is my IP?" has several honest answers depending on who is asking and from where: a laptop has many interfaces (Wi-Fi, VPN `utun`, Docker bridges); a container has an address its host does not; a Kubernetes pod has a pod IP, a node IP and an egress (SNAT) address that differs from both. The single public-IP source is a single point of failure: one provider, rate limits, no validation, and HTML or captive-portal pages are printed as if they were the answer.

0.0.1 also has defects that are visible to scripts: errors go to stdout with exit code 1, non-2xx bodies exit 0, and the argument is spliced into a URL path unvalidated. Fixing them changes behavior, so the new contract must be explicit before it is implemented.

## Decision
`myip` 0.1.0 answers **"where am I on the network, and what will others see?"**, by dimension, each fact carrying provenance.

**Commands**

| Command | Question | How we know |
|---|---|---|
| `myip public` | What does the internet see? | several independent HTTPS providers; ordered fallback; `--verify` = quorum and disagreement is reported |
| `myip local` | Which addresses do my interfaces have, and which one leaves via the default route? | interface enumeration, scope classification, UDP-connect egress probe (no packets sent), gateway from the route table |
| `myip where` | Host, container or Kubernetes, and what identity does it have? | `/.dockerenv`, cgroups, Downward API environment, service-account namespace file, `KUBERNETES_SERVICE_HOST`, `resolv.conf` |
| `myip all` | All of the above, side by side (exposes NAT: local != public) | composes the three |
| `myip [FIELD]` | The legacy ipinfo passthrough | ipinfo.io |

Bare `myip` and `myip FIELD` keep printing the ipinfo body. Making an identity overview the default is deferred to 0.2.

**Output contract**
- stdout carries the result only and ends with a newline; diagnostics go to stderr.
- `--json` emits one document with `"schema_version": 1`. The schema is stable within a minor series; human-readable text is explicitly not stable.
- Unknown values are `null`, never guessed, and each field names its `source`.

**Exit codes**: `0` success; `1` no answer could be obtained (network, provider or HTTP error); `2` usage error; `3` `--verify` ran but providers disagreed.

**Cross-cutting flags**: `--json`, `-4`/`-6`, `--timeout` (default 10s), `--proxy none|env|URL` (default `none`, matching 0.0.1: environment proxies are ignored unless asked for), `-v`.

**Trust rules**
- Remote answers are untrusted: HTTPS only, size-capped, parsed strictly (a valid, globally routable address of the requested family); HTML bodies are errors.
- Local reads are read-only and allow-listed. Credentials are never read, in particular `/var/run/secrets/kubernetes.io/serviceaccount/token`.
- The legacy argument is restricted to `[A-Za-z0-9._:/-]`, may not contain `..`, and must be non-empty after trimming.

**Architecture**: a library with a thin binary; HTTP and environment access sit behind traits so everything is testable without a network or a real host (#13).

## Alternatives considered
| Option | For | Against |
|---|---|---|
| Keep one provider (ipinfo) | simplest, no new behavior | single point of failure; cannot detect a lying or blocked provider |
| Change the default output to an identity overview now | best first impression | breaks every script that parses `myip` today |
| Hand-rolled argument parsing | no new dependency | worse errors, no `--help`/completions, more code to get wrong |
| clap | standard, good errors and completions | larger dependency tree (accepted) |
| Add cloud metadata and Kubernetes API access in 0.1.0 | richer answers | credentials, SSRF-shaped risks and per-cloud code; needs its own ADRs |
| One merged "best guess" IP | simple output | hides NAT, VPN and split routing, which is the information users want |

## Consequences
- Easier: the answers are auditable (provenance), scriptable (`--json`, exit codes), and degrade gracefully when a provider is down.
- Harder: the output contract is now a promise; changing JSON field meanings needs a `schema_version` bump and a new ADR.
- Legacy behavior changes (stderr, exit codes, argument validation, non-2xx handling) are documented in the changelog as fixes, not silent drift.
- Third-party providers see the caller's IP. That is inherent to the question; the README will say so when `myip public` lands (#15).
- Cannot do in 0.1.0: cloud metadata, Kubernetes API access, an echo server mode, STUN/DNS providers, a cache, geolocation databases. They are the 0.2+ roadmap.

## Reversal
Docs-only until the child specs land; this ADR can be superseded by a new one. After 0.1.0 ships, reversing the output contract is a breaking change for consumers of `--json` and the exit codes, which is why `schema_version` exists. Revisit if provider quorum proves too noisy in the nightly canary (#7) or if a major user group needs the legacy default changed.
