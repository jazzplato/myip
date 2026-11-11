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
| `myip public` | What does the internet see? | several independent HTTPS providers; ordered fallback; `--verify` queries all of them and reports agreement or disagreement |
| `myip local` | Which addresses do my interfaces have, and which one leaves via the default route? | interface enumeration, scope classification, UDP-connect egress probe (no packets sent), gateway from the route table |
| `myip where` | Host, container or Kubernetes, and what identity does it have? | `/.dockerenv`, cgroups, Downward API environment, service-account namespace file, `KUBERNETES_SERVICE_HOST`, `resolv.conf` |
| `myip all` | All of the above, side by side (exposes NAT: local != public) | composes the three |
| `myip [FIELD]` | The legacy ipinfo passthrough | ipinfo.io |

Bare `myip` and `myip FIELD` keep printing the ipinfo body. Making an identity overview the default is deferred to 0.2.

**Scope of the rules below.** The output contract and the strict address parsing apply to the new subcommands (`public`, `local`, `where`, `all`). The legacy passthrough keeps its own contract: the raw ipinfo body (JSON or plain text, not re-encoded; `--json` is accepted and changes nothing there), exit codes, and the transport rules (HTTPS only, size cap, HTML rejection, redirect rule). It is never parsed as an address and has no `schema_version` or `source`.

**Output contract (new subcommands)**
- stdout carries the result only and ends with a newline; diagnostics go to stderr. With `--verify` and exit code `3` the per-provider report is the result, so it goes to stdout.
- `--json` emits one document with `"schema_version": 1`. Adding fields does not change the version; removing or renaming a field or changing its meaning does (and needs a new ADR). Human-readable text is explicitly not stable.
- Unknown values are `null`, never guessed, and each fact names its `source`.

**Exit codes**: `0` success; `1` no answer could be obtained (network, provider or HTTP error); `2` usage error; `3` `--verify` ran but providers disagreed.

**Cross-cutting flags**
- `--json`, `-v`, `--timeout <SECS>`, `--proxy none|env|URL`, and `-4`/`-6` (mutually exclusive; both together is a usage error).
- `--timeout` is an integer number of seconds from 1 to 600 (`0` and non-numbers are usage errors), applied to each HTTP request, default 10 (0.0.1 used 30). Fallback can therefore take up to providers x timeout; `--verify` queries concurrently and takes about one timeout.
- `--proxy` defaults to `none`, matching 0.0.1: environment proxies are ignored unless asked for. Userinfo in a proxy URL (and in `HTTPS_PROXY`/`ALL_PROXY` under `env`) is never printed: not in `-v`, errors or `--json`. Credentials on argv are visible to other local users, so `--proxy env` is the recommended way to pass them.
- `-4`/`-6` restrict remote lookups (`public`, the public part of `all`, the legacy path) and filter the addresses `local` prints. They do not affect `where`. They are implemented as IP-family control on the same hostname, never by switching to different (for example `api6.`) hostnames, so the provider list below stays the complete list of hosts. #14 owns the flag, mutual exclusion and the legacy path; #15 the provider path; #16 the `local` filter. Without either flag the family is whatever the connection uses (dual-stack hosts may therefore see IPv4 or IPv6).
- `--verify` belongs to `public` and `all`.

**Command semantics**
- `public`: the first provider that returns a valid answer wins; if none does, exit `1`. With `--verify` all providers are queried and answers are compared within each address family: a split between families (one provider answered with IPv4, another with IPv6) is not a disagreement. Exit `0` only if, in each family that answered, at least two providers answered and all of them agree; any disagreement inside a family exits `3` and prints the per-provider answers with the headline address `null`; a family with fewer than two answers cannot be verified and exits `1`. A provider that errored is listed but is not a disagreement.
- `local`, `where`: read-only; facts that cannot be determined are `null`. `where` always exits `0` (a bare host is a valid answer). `local` exits `1` only if interface enumeration itself fails.
- `all`: a report, not a gate. A failed section is reported in-band (`null` values plus an `error` string, with a note on stderr). It exits `1` only if every section failed, `3` if `--verify` found a disagreement, otherwise `0`; an unverifiable result (fewer than two answers) is reported in-band and does not change the exit code. Use `public --verify` for strict scripting.

**Providers.** `public` queries, in this order: Cloudflare trace, ipify, checkip.amazonaws.com, icanhazip, ident.me, ipinfo.io `/ip`. Changing the list or its order is a provider-strategy change and needs a new ADR; #15 implements it. `myip` sends no telemetry and contacts no host other than the provider it is querying.

**Trust rules**
- Remote answers are untrusted: HTTPS only, including redirect targets (a downgrade to `http` is an error, and so is a redirect to another host), size-capped (1 MiB for the legacy path, 4 KiB for provider answers; exceeding the cap is an error with exit `1`, never a truncated answer), and HTML bodies are errors. For provider answers the body must also parse as a valid, globally routable address of the requested family.
- Local reads are read-only and allow-listed. Credentials are never read, in particular `/var/run/secrets/kubernetes.io/serviceaccount/token`.
- The legacy FIELD is an opaque path suffix appended by string concatenation to the constant `https://ipinfo.io/`; it is never resolved as a URL reference. Surrounding whitespace is trimmed (as in 0.0.1), then FIELD must be non-empty, match `[A-Za-z0-9._:/-]`, not start with `/`, and contain neither `..` nor `//`. Anything else is a usage error (exit `2`) with no request made. Tests must show that `//example.com`, `https://example.com` and `file:///etc/passwd` never produce a request to another host.

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
- Legacy behavior changes are documented in the changelog as fixes, not silent drift: errors on stderr; non-2xx exits `1`; HTML bodies are errors; FIELD is validated; extra positional arguments are a usage error (0.0.1 ignored them); a trailing newline is guaranteed; default timeout 30s to 10s; response cap 10 MiB to 1 MiB; redirects to another host or to `http` are errors (0.0.1 followed them).
- Third-party providers see the caller's IP. That is inherent to the question; the README (and `SECURITY.md`'s scope note) will say so when `myip public` lands (#15).
- Cannot do in 0.1.0: cloud metadata, Kubernetes API access, an echo server mode, STUN/DNS providers, a cache, geolocation databases. They are the 0.2+ roadmap.

## Reversal
Docs-only until the child specs land; this ADR can be superseded by a new one. After 0.1.0 ships, reversing the output contract is a breaking change for consumers of `--json` and the exit codes, which is why `schema_version` exists. Revisit if the disagreement rate of `--verify` proves too noisy in the nightly canary (#7) or if a major user group needs the legacy default changed.
