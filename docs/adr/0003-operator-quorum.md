# 0003. Operator-based quorum for `--verify`

- Status: accepted
- Date: 2026-11-24
- Issue: #56
- Amends: [0001](0001-product-scope-and-cli-contract.md) (the `verified` quorum, named under Decision)

## Context
ADR 0001 sells `--verify` as a cross-check between "several independent HTTPS providers" and defines a *verified* family as one where "at least two providers answered in it and all of them agree". The code does exactly that, by counting answers: `FamilyReport::new` in `src/public/verdict.rs` returns `Unverified` for one answer, `Verified` when `agree == total`, `Disagree` otherwise, and `Verdict::exit_code` maps that to 3, 0 or 1. Nothing asks who runs the providers. The README's Privacy section already admits the gap: "`one.one.one.one` and `icanhazip.com` are both served by Cloudflare, so `--verify` does not make them two independent opinions."

So two answers from one party satisfy the quorum. That is reachable: under `-6` only providers 1, 2, 4 and 5 can answer, and if ipify and ident.me are unreachable, Cloudflare's two answers alone exit `0` as "verified". The same happens on any network that lets only Cloudflare through. Nothing was cross-checked in those runs.

**Measured on 2026-11-23 and re-checked on 2026-11-24 with the same servers, headers and networks** (this ADR's author, from a host with working IPv4; the answering address rotates between lookups):

| # | Provider host | Answering IP(s) | Evidence | Network | Operator |
|---|---|---|---|---|---|
| 1 | one.one.one.one | 1.0.0.1 | `server: cloudflare`, `cf-ray`; `/cdn-cgi/trace` is Cloudflare's own edge feature | AS13335 Cloudflare | Cloudflare |
| 2 | api64.ipify.org | 104.237.62.213, 173.231.16.77 | `Server: nginx`, nothing else identifying; reverse DNS of both is `api64.ipify.org` | AS18450 WebNX (rented hosting) | ipify |
| 3 | checkip.amazonaws.com | 35.175.157.143, 174.129.18.38, 100.58.89.140 | `server: nginx`; `dig` shows CNAME `checkip.check-ip.aws.a2z.com` | AS14618 Amazon | Amazon |
| 4 | icanhazip.com | 104.16.185.241, 104.16.184.241 | `server: cloudflare`, `cf-ray`; owned and operated by Cloudflare since June 2021 per the previous owner (<https://major.io/icanhazip-com-faq/>) | AS13335 Cloudflare | Cloudflare |
| 5 | ident.me | 65.108.151.63 | only `alt-svc`; reverse DNS is `ident.me` | AS24940 Hetzner (rented hosting) | ident.me |
| 6 | ipinfo.io | 34.117.59.81 | `via: 1.1 google` (a Google Cloud load balancer in front); reverse DNS is `*.bc.googleusercontent.com` | AS396982 Google (cloud) | IPinfo |

Reproduce with `curl -s -D - -o /dev/null -w 'remote_ip=%{remote_ip}\n' <url>` (headers and the address that answered; `curl -I` is unreliable here: HEAD gives 404 on `/cdn-cgi/trace`, 405 on `ipinfo.io/ip`, and printed no status line for `api64.ipify.org`), then `curl -s https://ipinfo.io/<answering-ip>/json` (`org` is ASN and name, `hostname` the reverse DNS) and `dig +short <host> A` / `AAAA`. The IPv6 addresses of providers 1, 2, 4 and 5 resolve to the same networks (AS13335, AS18450, AS13335, AS24940), but IPv6 headers were not measured: this host has no working IPv6 path (`curl -6 https://api64.ipify.org` printed an IPv4 address).

What this shows and what it does not: no two rows share an operator except 1 and 4. "Operator" for 2, 5 and 6 rests on the absence of any shared header, network or reverse-DNS evidence; it is not proof of independent ownership. IPv4 therefore has five operators behind six providers; IPv6 has three (Cloudflare, ipify, ident.me) behind four.

Contract constraints from ADR 0001: a change of "the quorum" needs a new ADR; `schema_version` is 1 and "removing or renaming a field or changing its meaning" bumps it, while adding fields does not. Today the tests pin `providers.len() == total` and `unverified` implies `agree == total == 1` (`src/public/mod.rs`).

## Decision
Count distinct operators, not URLs: a family is *verified* when answers from at least two distinct operators were received and all answers in it agree. Keep all six providers, record each one's operator in the provider table, and keep `schema_version` at 1 by only adding JSON fields.

**What an operator is.** The party that authors the answer: whose software turns the connection into the address text, and who therefore could change two answers with one decision or one bug. Two hostnames have one operator when one organisation runs both services (Cloudflare: `one.one.one.one` is its own edge, `icanhazip.com` is its own site). Hosting does not decide it: AWS, Google Cloud, Hetzner and WebNX rent machines and load balancers to many independent services whose software, keys and configuration differ; what tenants share is availability, which `--verify` already tolerates because an errored provider is not a disagreement. So the ASN and any `server`/`via` header are recorded as evidence, never as the operator. Residual risk, stated plainly: two providers behind the same third-party proxy would share that proxy's view of the client address. Today only ipinfo.io has a third-party front (Google's), so nothing is merged now; a provider added later in that position needs a sentence in its PR and the maintainer may declare it the same operator.

**The table.** `Provider` gains `operator: &'static str`, a lowercase slug compared by equality: 1 `cloudflare`, 2 `ipify`, 3 `amazon`, 4 `cloudflare`, 5 `ident.me`, 6 `ipinfo`. The provider list, URLs and order do not change.

**The quorum.** For each address family, over the answers received:
1. if any two answers differ: `disagree` (as today, whichever operators they come from; one operator contradicting itself is still a conflict);
2. else if the answers come from at least two distinct operators: `verified`;
3. else: `unverified`.

The minimum of 2 is a constant in `verdict.rs`, not a flag. It is a floor on operators with unanimity above it, not a 2-of-3 majority: 5 of 6 agreeing is still `disagree` and exit 3. `Verdict::exit_code` is untouched (3 if any family disagrees, else 0 if any family is verified, else 1).

| Answers in one family | Today | Under this ADR |
|---|---|---|
| all six agree | verified, exit 0 | verified (5 operators), exit 0 |
| providers 1 and 4 agree, nobody else answered | verified, exit 0 | unverified, exit 1 |
| providers 1, 4 and 2 agree | verified, exit 0 | verified (2 operators), exit 0 |
| provider 1 says A, provider 4 says B, nobody else | disagree, exit 3 | disagree, exit 3 |
| five agree, one differs | disagree (5 of 6), exit 3 | unchanged |
| IPv4 verified by two operators, IPv6 only from 1 and 4 | verified + verified, exit 0 | verified + unverified, exit 0 (a lone family never fails the run) |

**Fewer than two operators.** The family is `unverified`; with no verified family and no disagreement the run exits `1` with `the address could not be verified: fewer than two independent operators answered in any address family` (replacing the `NOT_VERIFIED` text). `public -6 --verify` needs answers from two of Cloudflare (provider 1 or 4), ipify (2) and ident.me (5). `all` stays a report: the unverifiable result is in-band and the exit code does not change.

**JSON.** `schema_version` stays 1. `verified: {agree, total}` and `families[].agree`/`total` keep counting answers, so `providers.len() == total` and "verified implies `agree == total >= 2`" still hold. Additive only: each `families[].providers[]` entry gets `operator` and each `families[]` entry gets `operators` (the number of distinct operators among its answers). The documented meaning of `status` tightens: `verified` is "answers from at least two operators, all agree"; `unverified` is "answers from fewer than two operators, all agree" (it can now have `total` 2 or more). The headline provider is still the first answer in provider order. Text output is not stable (ADR 0001) and says "operators" where it now means them.

**Keeping the attribution honest.**
- `PROVIDERS` is the single source of truth; the pinned-literal test `the_provider_list_is_the_adr_list_in_order` gains the operator, so a change is deliberate and visible in review.
- This ADR's table is a dated snapshot, not a live record. Any PR that adds a provider, changes a URL or changes an operator must re-run the commands above and paste the output in the PR body, as a bug fix does for its failing test. Correcting an operator is a fact fix and needs no new ADR; adding, removing or reordering providers still does (ADR 0001).
- No unit test can see the real world (AGENTS.md rule 4). Drift (a provider moving behind Cloudflare, an ownership change) is for the provider canary of #7 (stage:idea, not built): when it exists it should compare `server`/`cf-ray` and the ASN of the answering address with the table, non-blocking.
- Not inferred at runtime: headers are absent for ipify and ident.me (measured above) and an ASN lookup is another request to another provider.

**Amends ADR 0001.** Its "verified" clause ("at least two providers answered in it") and the JSON `status` definitions ("at least two answers", "fewer than two answers") now read "operators" for the threshold; "several independent HTTPS providers" is now qualified by the operator rule; the `-6` sentence reads as above; the `all` note "(fewer than two answers)" reads "fewer than two operators". `total`, `agree`, exit codes, the provider list and order are unchanged. The PR that lands this ADR adds `- Amended by: [0003](0003-operator-quorum.md) (the quorum counts operators)` to ADR 0001's header and the row `| 0003 | [Operator-based quorum for --verify](0003-operator-quorum.md) | accepted |` to `docs/adr/README.md`. Code, tests, README and CHANGELOG are #24.

## Alternatives considered
| Option | For | Against |
|---|---|---|
| One vote per URL (status quo) | nothing to maintain, no attribution claims | Cloudflare votes twice; "verified" can mean one party said it twice; the README already apologises for it |
| Count operators, unanimous, floor of 2 (chosen) | smallest change to ADR 0001; every exit code keeps its meaning; any dissent still surfaces as exit 3 | needs an attribution table that can rot |
| Majority threshold (2 of 3 style) | tolerates one liar or bug | hides a dissent behind exit 0, the opposite of exit 3; IPv6 has only 3 operators, so a 2-of-3 majority can be wrong while exiting 0 |
| Floor of 3 operators | stronger evidence | IPv6 has exactly 3, so one outage makes `-6 --verify` unverifiable; more exit 1 for little gain |
| Configurable threshold (`--quorum N`) | flexible | a new flag is a permanent contract (ADR 0002 style); no user has asked; can be added later without a break |
| Drop `icanhazip.com`, keep one vote per URL | the rule stays as is; every provider is its own operator | a list change needs its own ADR anyway; one fewer fallback (5 providers, 3 under `-6`); no guard if a later provider repeats the problem; a replacement must be independent, plain-text and IPv6-capable, and none is measured yet (candidates belong to epic #8) |
| Count `total`/`agree` as operators, bump `schema_version` to 2 | numbers match the rule | breaks every consumer and the `providers.len() == total` invariant, for a case that only arises when the other providers fail |
| Keep numbers as answers, add no fields | zero contract growth | `unverified` with `agree 2, total 2` is unexplained in JSON |
| Treat one cloud or ASN as one operator | automatic, no table | merges unrelated tenants of AWS or Google Cloud and correlates availability, not answers; changes nothing today yet is wrong for later providers |
| Infer operators at runtime (headers, ASN lookup) | no table to rot | ipify and ident.me send no identifying header; an ASN lookup is another network call; a failed lookup would change exit codes and the tests stop being hermetic |
| Weight Cloudflare's two answers at half a vote | keeps both answers counted | fractional quorums are hard to explain and to test; no better than one vote per operator |

## Consequences
- Easier: `verified` means answers from at least two operators; the README Privacy caveat can become the rule; attribution is one reviewable table; the STUN and DNS providers of epic #8 can declare their own operator and join without new quorum logic.
- Harder: operator attribution is a factual claim that can go stale and a new provider needs a measurement. The README table, `PROVIDERS` and the pin test must change together.
- Behaviour change: runs where only Cloudflare answers now exit `1` instead of `0` (for example `-6` with ipify and ident.me unreachable, or a network that allows only Cloudflare). Those runs verified nothing; the CHANGELOG must say so.
- Two vocabularies: JSON numbers count answers while the rule counts operators. The additive `operators` and `operator` fields and the text make the gap visible, and they are now promises: removing them later bumps `schema_version`.
- `status` definitions in ADR 0001 and the README tighten, and the JSON invariant "unverified means 1 of 1" no longer holds. A consumer that infers `status` from `agree == total >= 2` instead of reading `status` or the exit code would now see `verified` where this ADR says `unverified`.
- Cannot do: weighted votes, a configurable threshold, runtime attribution, or detect drift before the canary exists.

## Reversal
The rule is cheap to undo: stop counting operators and #24's verdict change reverts to the status quo. The additive JSON fields are not: once released, removing `operators` or `operator` is a breaking change under ADR 0001, so ship them only with #24. Revisit if a provider changes operator (for example ipify behind Cloudflare), if the canary of #7 shows attribution drift, or if epic #8 raises the IPv6 operator count enough to want a floor of 3. Rollback of this ADR's PR: revert it; #24 stays blocked.
