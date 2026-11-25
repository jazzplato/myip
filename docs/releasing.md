# Releasing

A release is built by [`release.yml`](../.github/workflows/release.yml) from a `v*` tag. The workflow only ever creates a **draft** GitHub Release; publishing it, and `cargo publish`, are maintainer actions.

## Cut a release
1. Merge the release PR: it sets `version` in `Cargo.toml` (and the matching `Cargo.lock` entry) and gives `CHANGELOG.md` a `## [X.Y.Z] - YYYY-MM-DD` section (the day you tag) plus the compare links at the bottom, with `## [Unreleased]` left empty above it. The tag must be on `master`. Before you tag, check that `ci-gate` is green on master, that no Dependabot PR you want in this release is still open, and that `cargo package --locked --list` shows what you expect.
2. Tag the merge commit and push the tag (the `release-tags` ruleset makes it immutable, so check the version first):
   ```sh
   git tag "v$VERSION" <commit-on-master> && git push origin "v$VERSION"
   ```
3. Open the run (Actions -> Release). The `draft release` job uses the `release` environment: approve it if the environment has a required reviewer.
4. Verify the draft **before publishing** (`shasum` is on macOS and Linux with Perl):
   ```sh
   gh release download "v$VERSION" -R jazzplato/myip -D /tmp/myip-release && cd /tmp/myip-release
   shasum -a 256 -c SHA256SUMS
   for f in myip-v*.tar.gz myip-v*.zip SHA256SUMS; do
     gh attestation verify "$f" -R jazzplato/myip \
       --signer-workflow jazzplato/myip/.github/workflows/release.yml \
       --source-ref "refs/tags/v$VERSION" --deny-self-hosted-runners
   done
   ```
5. Edit the notes from `CHANGELOG.md`, then publish the draft. Then run `cargo publish` yourself from your checkout (step 4 left you in the download directory), on the tagged commit: `cd <your checkout> && git switch --detach "v$VERSION" && cargo publish --locked`.
6. After publishing: check that `cargo install myip --version "$VERSION" --locked` works in a clean container, close the release issue and the milestone, and leave `Cargo.toml` alone (there is no `-dev` bump; the next release PR sets the next version).

## Dry run
Actions -> Release -> Run workflow on any branch. It verifies the tag/version rule and `cargo publish --dry-run --locked`, builds all five targets, and produces `SHA256SUMS`, then stops: no release and no attestation are created. The optional `tag` input simulates a tag; anything but `v<Cargo.toml version>` must fail the run.

## What the workflow checks
- the tag equals `v` + the `Cargo.toml` version, and (real tags only) the commit is reachable from `master`;
- `cargo publish --dry-run --locked` succeeds (nothing is published);
- every binary prints `myip <version> (<short commit> <commit date, UTC>)` from `--version` (the `verify` job computes the commit and date once and the build reads them at compile time through `MYIP_BUILD_COMMIT` and `MYIP_BUILD_DATE`; `cargo publish` sets neither, so the crate prints the bare version); the Linux binaries also run in an empty `scratch` container (static) and print the same line; the Windows binary does not import the Visual C++ runtime;
- exactly the five archives and `SHA256SUMS` exist, and `SHA256SUMS` matches them; the release job downloads only the `myip-*` and `SHA256SUMS` artifacts, re-checks that exact file set and the checksums, and only then attests and uploads.

Build artifacts are kept for 30 days, the longest a `release` environment approval can stay pending. If an approval is older than that, the run fails; re-run all jobs to rebuild.

## Rollback
Delete the draft release. A pushed `v*` tag cannot be deleted or moved while the `release-tags` ruleset is active; to retire a bad tag, temporarily disable that ruleset, delete the tag, and re-enable it, or ship a fixed `vX.Y.(Z+1)`. A **published** release and a crates.io upload cannot be taken back (crates.io allows only `cargo yank`), so do the verification in step 4 first.
