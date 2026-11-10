#!/usr/bin/env bash
#
# Apply the repository settings declared in .github/settings/ (settings as code).
#
#   .github/settings/apply.sh [--dry-run] [--force]
#
#   --dry-run  Print every mutating `gh api` command instead of running it. Read-only
#              GET requests still run so the plan shows create vs update.
#   --force    Apply even if `ci-gate` is not defined on the default branch yet.
#   REPO=owner/name overrides the repository detected by `gh repo view`.
#
# Needs an authenticated `gh` with admin rights on the repository.
# Idempotent: every step declares a state (PUT/PATCH, rulesets upserted by name),
# so re-running changes nothing. Run it only after the CI workflow that defines
# `ci-gate` is on the default branch: a required check that never reports blocks every merge.
set -euo pipefail

DRY_RUN=0
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --force) FORCE=1 ;;
    -h | --help)
      sed -n '3,15p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown argument: $arg (see --help)" >&2
      exit 2
      ;;
  esac
done

die() {
  echo "error: $*" >&2
  exit 1
}

command -v gh >/dev/null 2>&1 || die "the gh CLI is required"

SETTINGS_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=${REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

RESULTS=""
FAILED=0

record() {
  RESULTS+="$(printf '%-8s %s' "$1" "$2")"$'\n'
}

# step <description> <gh api args...>: the only place that mutates the repository.
step() {
  local desc=$1
  shift
  if ((DRY_RUN)); then
    local arg cmd="" body=""
    for arg in "$@"; do
      if [[ $arg == "$TMP"/* ]]; then
        body=$(cat "$arg")
        arg=-
      fi
      cmd+=" $(printf '%q' "$arg")"
    done
    echo "[dry-run] $desc"
    if [[ -n $body ]]; then
      echo "    echo '$body' | gh api$cmd"
    else
      echo "    gh api$cmd"
    fi
    record DRY-RUN "$desc"
    return 0
  fi
  local out
  if out=$(gh api "$@" 2>&1); then
    echo "ok: $desc"
    record OK "$desc"
  else
    echo "FAILED: $desc" >&2
    printf '%s\n' "$out" | sed 's/^/    /' >&2
    record FAILED "$desc"
    FAILED=1
  fi
}

# step_json <description> <method> <endpoint> <json body>
step_json() {
  local desc=$1 method=$2 endpoint=$3 json=$4 file
  file=$(mktemp "$TMP/body.XXXXXX")
  printf '%s\n' "$json" >"$file"
  step "$desc" --method "$method" "$endpoint" --input "$file"
}

# upsert_ruleset <name> <file>: create the ruleset, or update it in place by name.
upsert_ruleset() {
  local name=$1 file=$2 id
  grep -q "\"name\": \"$name\"" "$file" || die "$file does not declare ruleset \"$name\""
  id=$(gh api "repos/$REPO/rulesets" --paginate \
    --jq ".[] | select(.name == \"$name\" and (.source_type // \"Repository\") == \"Repository\") | .id")
  [[ $id != *$'\n'* ]] || die "several rulesets named \"$name\"; delete the duplicates first"
  if [[ -n $id ]]; then
    step "ruleset $name: update (id $id)" --method PUT "repos/$REPO/rulesets/$id" --input "$file"
  else
    step "ruleset $name: create" --method POST "repos/$REPO/rulesets" --input "$file"
  fi
}

echo "Repository: $REPO"
if ((DRY_RUN)); then
  echo "Mode: dry-run (no changes are made; read-only lookups still run)"
fi

[[ $(gh api "repos/$REPO" --jq .permissions.admin) == true ]] ||
  die "your gh account needs admin rights on $REPO"
DEFAULT_BRANCH=$(gh api "repos/$REPO" --jq .default_branch)
ME_LOGIN=$(gh api user --jq .login)
ME_ID=$(gh api user --jq .id)

# Preflight: refuse to lock the repo behind a check that cannot report yet.
CI_YML=$(gh api -H 'Accept: application/vnd.github.raw+json' \
  "repos/$REPO/contents/.github/workflows/ci.yml?ref=$DEFAULT_BRANCH" 2>/dev/null || true)
if [[ $CI_YML != *ci-gate* ]]; then
  msg="ci-gate is not defined in .github/workflows/ci.yml on $DEFAULT_BRANCH; the master ruleset would block every merge"
  if ((FORCE || DRY_RUN)); then
    echo "warning: $msg" >&2
  else
    die "$msg (merge the CI change first, or pass --force)"
  fi
fi

step_json "repo: squash-only merges, delete head branches, auto-merge off" PATCH "repos/$REPO" \
  '{"allow_squash_merge":true,"allow_merge_commit":false,"allow_rebase_merge":false,"allow_auto_merge":false,"delete_branch_on_merge":true}'

step_json "actions: default token read-only, workflows cannot approve PRs" PUT "repos/$REPO/actions/permissions/workflow" \
  '{"default_workflow_permissions":"read","can_approve_pull_request_reviews":false}'

step "security: Dependabot alerts on" --method PUT "repos/$REPO/vulnerability-alerts"
step "security: Dependabot security updates on" --method PUT "repos/$REPO/automated-security-fixes"
step "security: private vulnerability reporting on" --method PUT "repos/$REPO/private-vulnerability-reporting"
step_json "security: secret scanning and push protection on" PATCH "repos/$REPO" \
  '{"security_and_analysis":{"secret_scanning":{"status":"enabled"},"secret_scanning_push_protection":{"status":"enabled"}}}'

# Self-review stays allowed: a solo maintainer must be able to approve their own release.
step_json "environment release: required reviewer $ME_LOGIN" PUT "repos/$REPO/environments/release" \
  "{\"prevent_self_review\":false,\"reviewers\":[{\"type\":\"User\",\"id\":$ME_ID}]}"

# Rulesets last, so a failure above can never leave a half-protected repo that is also locked.
upsert_ruleset master-protection "$SETTINGS_DIR/ruleset-master.json"
upsert_ruleset release-tags "$SETTINGS_DIR/ruleset-tags.json"

echo
echo "== Summary for $REPO =="
printf '%s' "$RESULTS"
if ((DRY_RUN)); then
  echo "Dry run only. Re-run without --dry-run to apply."
elif ((FAILED)); then
  echo "Some steps FAILED (see above). Fix the cause and re-run; completed steps are safe to repeat." >&2
else
  echo
  echo "Active rulesets:"
  gh api "repos/$REPO/rulesets" --jq '.[] | "  \(.name)\t\(.target)\t\(.enforcement)\tid=\(.id)"'
  echo "There are no bypass actors: an emergency change means consciously editing the ruleset."
fi
exit "$FAILED"
