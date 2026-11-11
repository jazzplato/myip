# Releasing

A release is built by [`release.yml`](../.github/workflows/release.yml) from a `v*` tag. The workflow only ever creates a **draft** GitHub Release; publishing it, and `cargo publish`, are maintainer actions.

## Cut a release
1. Merge the release PR: it bumps `version` in `Cargo.toml` and updates `CHANGELOG.md`. The tag must be on `master`.
2. Tag the merge commit and push the tag (the `release-tags` ruleset makes it immutable, so check the version first):
   ```sh
   git tag "v$VERSION" <commit-on-master> && git push origin "v$VERSION"
   ```
3. Open the run (Actions -> Release). The `draft release` job uses the `release` environment: approve it if the environment has a required reviewer.
4. Verify the draft **before publishing**:
   ```sh
   gh release download "v$VERSION" -R jazzplato/myip -D /tmp/myip-release && cd /tmp/myip-release
   sha256sum --check SHA256SUMS            # shasum -a 256 -c SHA256SUMS on macOS
   for f in myip-v*.tar.gz myip-v*.zip SHA256SUMS; do gh attestation verify "$f" -R jazzplato/myip; done
   ```
5. Edit the notes from `CHANGELOG.md`, then publish the draft. Then run `cargo publish` yourself.

## Dry run
Actions -> Release -> Run workflow on any branch. It verifies the tag/version rule and `cargo publish --dry-run --locked`, builds all five targets, and produces `SHA256SUMS`, then stops: no release and no attestation are created. The optional `tag` input simulates a tag; anything but `v<Cargo.toml version>` must fail the run.

## What the workflow checks
- the tag equals `v` + the `Cargo.toml` version, and (real tags only) the commit is reachable from `master`;
- `cargo publish --dry-run --locked` succeeds (nothing is published);
- every binary prints `myip <version>` from `--version`; the Linux binaries also run in an empty `scratch` container (static); the Windows binary does not import the Visual C++ runtime;
- exactly five archives exist, and `SHA256SUMS` matches them (checked again in the release job).

## Rollback
Delete the draft release. A pushed `v*` tag cannot be deleted or moved while the `release-tags` ruleset is active; to retire a bad tag, temporarily disable that ruleset, delete the tag, and re-enable it, or ship a fixed `vX.Y.(Z+1)`. A **published** release and a crates.io upload cannot be taken back (crates.io allows only `cargo yank`), so do the verification in step 4 first.
