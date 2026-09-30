# Releases

## Version Contract

The workspace version in `Cargo.toml` is the source of truth. All three crates,
the CLI and OpenAPI use that version. Commit the matching `Cargo.lock` and a dated,
nonempty `CHANGELOG.md` section before tagging `vMAJOR.MINOR.PATCH`. Prereleases use
`-alpha.N`, `-beta.N` or `-rc.N`. The initial release is `0.1.0-alpha.1`.

APIs remain experimental during this prerelease series. Record breaking changes
explicitly; a container release is not a claim of live game compatibility. Crates
remain `publish = false` and are not published to crates.io.

## CI and Cache Design

Pull requests, pushes to `main`, version tags and manual runs execute the same
test gate: version consistency, formatting, Clippy, all-target tests, doctests,
Rust documentation, release-script tests and Docker build-input validation.

Then native `ubuntu-24.04` and `ubuntu-24.04-arm` runners build Linux amd64/arm64
images. Each image is loaded and tested for version, non-root identity, config
validation, health, authentication, default-disabled routes and graceful shutdown.
License and protocol notices are included under `/usr/share/doc/moenotes-api`.
Fixtures are synthetic; these checks never log in to or query game servers.

Two independent caches reduce repeat Rust compilation:

- `Swatinem/rust-cache` saves Cargo downloads, test artifacts and the exported-input
  check's target directory. Only non-PR runs save this cache.
- BuildKit uses `type=gha,mode=max`, scoped to `image-amd64` and `image-arm64`.
  Pinned cargo-chef generates a normalized recipe, so compiled dependency layers
  can be reused when application source changes. Rust/toolchain/manifest/lockfile
  changes can invalidate those layers. The final application build still runs
  when its inputs change. No ephemeral cache mount is relied upon for persistence.

GitHub cache access is branch-scoped. Main-branch caches can seed release builds;
pull-request caches cannot replace main-branch caches. Quota eviction, base image
updates and cold caches can cause full rebuilds. Caches are an optimization, not a
correctness requirement. The source export includes only build inputs, not operator
configuration, credentials or analysis artifacts. No game secrets are configured
in CI. Base image tags are mutable; versions are pinned but builds are not claimed
to be bit-for-bit reproducible across base-image updates.

## Publish

Use focused Conventional Commit messages (`feat:`, `fix:`, `docs:`, `ci:`,
`chore(release):`). From a clean, reviewed `main`:

```sh
python3 scripts/release.py --tag v0.1.0-alpha.11
git push origin main
# Wait for the main-branch Build and Release workflow to pass.
git tag -a v0.1.0-alpha.11 -m "Release v0.1.0-alpha.11"
git push origin v0.1.0-alpha.11
```

Only tag runs log in to GHCR and push tested per-architecture images by digest.
After both pass, the release job assembles and verifies the multi-platform index,
then publishes the GitHub Release with `image-digests.json`. Tags are:

- `ghcr.io/starmoe-org/moenotes-api:0.1.0-alpha.11` (exact version).
- `ghcr.io/starmoe-org/moenotes-api:sha-<full-40-character-commit>` (source revision).
- `ghcr.io/starmoe-org/moenotes-api@sha256:<index-digest>` (immutable content).

There are no `latest`, major or minor aliases. Prereleases are marked as such and
not marked as GitHub's latest release. Version and SHA tags can change on a workflow
rerun; deploy by digest when exact content matters. Never move a published Git tag.
Fix source problems with a new commit/version instead. Retry transient runner or
registry failures at the same tag; incomplete runs may leave untagged architecture
images but do not create a successful GitHub Release. Inspect the completed run,
release asset and image index before announcing a release.

Actions use only the short-lived `GITHUB_TOKEN`. Test jobs have `contents: read`;
image jobs add `packages: write`, and the final release job adds `contents: write`.
Actions are pinned to commit hashes. No personal PAT is required in repository
secrets. Public images can be pulled anonymously.

## Package Visibility

The repository and image are distributed publicly by the maintainer's decision.
This does not establish third-party protocol redistribution rights or settle the
raw response-field policy. Those remain documented limitations.

The OCI source label associates the package with this repository. A newly created
GHCR package defaults to private, even for a public repository. After the first
architecture image is pushed, the maintainer must open the package settings and
change visibility to **public**. GitHub does not currently expose a supported
REST/GraphQL operation for this change. The release job refuses to publish a
release until the package is public and linked to the expected repository. If it
stops at this guard, change visibility and rerun only the failed job at the same
tag; the successful image jobs' digest artifacts remain available for seven days.
No source or tag change is needed. Verify an anonymous pull before announcing.

Never add registry or game tokens to Docker build arguments, layers, examples,
Git history or release notes. Consult
[security boundaries](../SECURITY.md) and [protocol provenance](../proto/NOTICE.md)
before distributing images beyond the authorized team.
