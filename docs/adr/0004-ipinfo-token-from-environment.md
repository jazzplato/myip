# 0004. ipinfo.io token from the environment

- Status: accepted
- Date: 2026-11-24
- Issue: #57
- Amends: [0001](0001-product-scope-and-cli-contract.md) (two sentences, named under Decision)

## Context
The ipinfo.io passthrough (`myip`, `myip FIELD`, `myip public FIELD`) can authenticate in one way: `?token=` inside FIELD ([ADR 0001](0001-product-scope-and-cli-contract.md): "the only way to pass a token"). The token is then in `argv`, so in `ps`, shell history and the command echo of CI logs. #33 wants it in the environment instead. That is a CLI input-contract change and the first time `myip` reads a credential, so it needs an ADR before code.

What the code does today (checked on `origin/master` 0f74cfc):
- **Environment contract.** `SystemEnv::var` returns a value only for a name in `ALLOWED_VARS` (exact match; eight Kubernetes and Downward API names, none a credential), and a test pins the list (`the_allow_list_is_exactly_what_detection_needs`). Paths work the same way (`ALLOWED_PATHS`), and `service_account_credentials_are_never_allowed` keeps the ServiceAccount `token` and `ca.crt` out. ADR 0001 says "Local reads are read-only and allow-listed. Credentials are never read, in particular `/var/run/secrets/kubernetes.io/serviceaccount/token`." The words "fixed list of environment variables" are README text about `where`, not ADR 0001.
- **Transport.** `UreqFetch` sends `Accept` and a fixed `User-Agent`, nothing else. It sets `max_redirects(0)` and follows `301`/`302`/`303`/`307`/`308` itself, at most `MAX_REDIRECTS = 5` hops, only to the same scheme and authority (`same_origin_target`); another host or `http` is refused. The host is the constant `https://ipinfo.io/`, so FIELD cannot change it.
- **Masking.** `-v` prints `GET <url>` through `redact_query` (`?token=***`). An error body is masked with `redact_arg_values(&body, args)`, which hides the values of the `?query` found in the command line `args` and nothing else; the README says "a value that you did not type on the command line is not masked". An environment value therefore has no masking path today. `FetchOptions` derives `Debug` and `Clone`, so a plain `String` field added to it would print the token in any `{:?}`.
- **Live finding** (checked 2026-11-23 and again 2026-11-24, no real token used; #33):

| Request | Result |
|---|---|
| `curl https://ipinfo.io/json` | `200`, anonymous answer |
| `curl -H 'Authorization: Bearer invalidtoken000' https://ipinfo.io/json` | `403`, `"title": "Unknown token"` |
| `curl 'https://ipinfo.io/json?token=invalidtoken000'` | the same `403`, `"title": "Unknown token"` |

  So the service reads the Bearer header and treats it as it treats `?token=`; an invalid token is an error, not a silent downgrade. The `403` body does not repeat the token. `/json`, `/ip` and `/city` answered `200` without a redirect, and an empty `?token=` answers `200` (anonymous).

## Decision
`myip` reads one more environment variable, `IPINFO_TOKEN`, and uses it only as an `Authorization: Bearer` header on the ipinfo.io passthrough. The token is never put in a URL, and `myip` never writes it itself; the one exemption is a successful (`2xx`) body, which stays the server's raw answer (see *Printing rule*).

**Variable.** `IPINFO_TOKEN` is added to `ALLOWED_VARS` (nine entries). There is no new `ALLOWED_PATHS` entry, no flag, no config file and no alias name.

**Who reads it.** Only the passthrough: `myip`, `myip FIELD` and `myip public FIELD` (ADR 0001: the same passthrough with all its rules). `public`, `public --verify`, `local`, `where` and `all` never ask for it and never send it, including the ipinfo.io provider (`https://ipinfo.io/ip`) of `public`, which stays anonymous. Tests use the recording `Env` to show that those commands never call `var("IPINFO_TOKEN")`.

**Transport.** When a token is used, the first request carries `Authorization: Bearer <token>`. The URL is built exactly as today and never gets a `token` added. A redirect that `same_origin_target` follows is sent without the header; one it refuses (another host, `http`) is still an error, so the header cannot leave `https://ipinfo.io`. Proxy, timeout, `-4`/`-6`, size cap and HTML rejection are unchanged.

**Precedence.** If FIELD carries a `token` parameter, FIELD wins: the variable is not read, not validated, and no header is sent; the request is the one 0.2.x sends, so `myip 'json?token=XYZ'` is unchanged. A parameter counts when its name equals `token` ignoring ASCII case after percent-decoding, whatever its value, even empty.

**Value rules** (when the variable is to be used):
- ASCII whitespace at both ends is trimmed (`IPINFO_TOKEN="$(cat file)"` leaves a newline). Empty after trimming means unset: an anonymous request, no message.
- Otherwise the value must be 1 to 256 characters from letters, digits and `. _ ~ + / = -` (the characters RFC 6750 allows in a Bearer credential). Anything else, a control character, a space inside, a non-ASCII character or more than 256 characters, is a usage error: exit `2`, one line on stderr that names `IPINFO_TOKEN` and the rule, no request. The message never contains the value, any character of it or its length.
- The check runs after the `--format` usage check and before `-v` prints anything or a request is made.
- A value that is not valid Unicode reads as unset, because `SystemEnv::var` is `std::env::var(name).ok()`. That is a known limit of the `Env` trait, not a rule of this ADR.

**Printing rule.** Nothing `myip` itself writes contains the value: not stdout or stderr output of its own making, not `-v`, any error, `--help`, `Debug` output or a panic message. The one exemption is a `2xx` body, which is the remote service's answer and is passed through unaltered, as ADR 0001 requires of the passthrough (it has no `schema_version`, but it may be a JSON document). `myip` does not mask or reject such a body: a server that echoes the caller's credential in a successful answer is its own behaviour (ipinfo.io does not in the `403` measured above), the user would see their own token in their own terminal, and altering the raw body would break the ADR 0001 contract. `-v` prints one fixed line when the header is sent (`myip: Authorization: Bearer *** (from IPINFO_TOKEN)`) and nothing about the token otherwise. The code holds the token in a type whose `Debug` and `Display` print `***`. Error bodies and transport messages are masked for the exact value, before they are cut to length, as `redact_arg_values` does for values from the command line. This is defence in depth: the primary control is that the value is only ever in the header, never in a URL or any text that is echoed. A `2xx` body is exempt, as above. No JSON document that `myip` builds contains the token: `public`, `local`, `where` and `all` never see it, and the passthrough only relays the server's body. `schema_version` stays `1`, and the exit codes are unchanged (an `Unknown token` `403` is exit `1`).

**ADR 0001 sentences superseded, and only these:**
1. In *Trust rules*: "Credentials are never read, in particular `/var/run/secrets/kubernetes.io/serviceaccount/token`." becomes "No credential of the machine or the cluster is read, in particular `/var/run/secrets/kubernetes.io/serviceaccount/token`. The one exception is the user's own ipinfo.io token in `IPINFO_TOKEN` ([ADR 0004](0004-ipinfo-token-from-environment.md)), which the user hands to `myip` and which is sent only to ipinfo.io."
2. In the legacy FIELD rule: "which 0.0.1 accepted and which is the only way to pass a token" becomes "which 0.0.1 accepted and which, with `IPINFO_TOKEN` ([ADR 0004](0004-ipinfo-token-from-environment.md)), is one of two ways to pass a token".

"Local reads are read-only and allow-listed" stays true; the list in `src/env.rs` grows by one name. ADR 0001 gets an `Amended by: 0004` header line and nothing else.

## Alternatives considered
| Option | For | Against |
|---|---|---|
| `Authorization: Bearer` from `IPINFO_TOKEN` (chosen) | token stays out of argv, history, URLs, `-v`, error bodies that echo the URL (#37) and server or proxy URL logs; the service accepts it (live check); `IPINFO_TOKEN` is the name ipinfo's own CLI uses (#33) | first credential read; a header path in `FetchOptions`; the value needs its own masking; the environment of a process is readable by the same user and root (`/proc/<pid>/environ` on Linux) |
| Append `?token=` built from the variable | smallest code change, no `FetchOptions` change | puts the token back in the URL: `-v`, error bodies and logs see it again, and the masking `redact_arg_values` offers covers only values typed on the command line |
| `--token-file PATH` | no environment exposure; fits Kubernetes secret volumes | a path the user chooses cannot be on the exact-match `ALLOWED_PATHS`, so the allow-list stops being a list; the flag could be pointed at the ServiceAccount `token`, which ADR 0001 forbids, so a deny rule is needed too; adds `..`, symlink and permission cases. `IPINFO_TOKEN="$(cat file)" myip` gives the same result |
| `--token VALUE` | explicit | the token is in argv again, which is the problem |
| Config file (for example under `~/.config`) | persistent, no shell setup | reads the home directory (hermetic tests, AGENTS.md rule 4), a new format and precedence layers, `myip` becomes a secret store; its own ADR |
| Also use the token for `public` and `all` | one setting for everything | `public` needs none; it would send a credential to a provider that gains nothing from it |
| Do nothing | free | the leaky `?token=` habit stays the only option |

## Consequences
- Easier: a token can come from a secret manager, a CI secret or a Kubernetes `secretKeyRef` without touching argv, history or the URL.
- Harder: a host that exports `IPINFO_TOKEN` for other tools now authenticates every passthrough call of `myip`, including in scripts that did not expect it, and its answers can change (richer fields, quota counted on that account). The README says so.
- Harder: a redirect that is followed carries no header. ipinfo.io sends none today, but if it did, an authenticated user would get an anonymous `200` without an error.
- The `Env` allow-list now holds a credential name. The pinned allow-list test and `tests/cli.rs` change with #33: the process tests clear `IPINFO_TOKEN` so the host cannot change a result.
- Cannot do: `--token-file`, a config file, a token for another command or provider, another header scheme.
- Work for #33, not this ADR: `src/**`, tests, README (the `where` "fixed list" sentence stays true for `where`) and CHANGELOG.

## Reversal
Revert #33; `IPINFO_TOKEN` is ignored again and this ADR is marked superseded. The cost is silent: a script that relies on the variable gets anonymous answers with exit `0`, so removing it needs a release that warns first. Revisit if ipinfo.io stops accepting Bearer, starts redirecting, or a second provider needs a credential (that gets its own ADR with a general rule for credentials).
