# Validation Record

## JP credential import and registration — 2026-09-29

124 Rust tests and two doctests passed, together with all-target Clippy, formatting,
seven release-script tests and Docker exported-input validation. JP coverage
includes separate descriptor selection, header omission/override rules, profile
route mapping, master-only versions, private origin-bound import, stale generation
rejection, no-overwrite registration output and local recovery when device_id is
omitted. Existing international tests remain enabled.

One explicitly authorized JP test account was created with Register. Its response
omitted device_id; the first normalizer rejected that assumption after saving the
raw response. The implementation was corrected and the retained response imported
locally, with no second registration. A fresh process loaded the credential and
passed Whoami identity matching, GetPlayerData and JP PlayerService/GetPlayerList.
HTTP announcements, self profile, favorite status and one music-ranking query
returned 200; repeated profile lookup returned HIT. The initial protected request
returned authentication_required while the lazy local loader ran, then the next
request succeeded. Status showed JP ready and valid version synchronization.

The registered profile ID happened to be 11 digits starting with 5; this observation
is not generalized into automatic region routing. Explicit `/v1/jp/profile/` is
required. No password transfer, device takeover, tutorial initialization, SDK login,
production configuration change or deployment was performed. Private credentials
and response bodies are retained outside Git and Docker build inputs.

## Alpha.4 Single-File Configuration

2026-09-25: 98 Rust tests (36 client, 3 protocol, 52 server unit, 7 process
integration), two doctests, seven release-script tests, fmt, all-target Clippy,
documentation and Docker exported-input validation passed. The local amd64
`moenotes-api:inline-config-dev` image passed five smoke configurations: absent
config, blank distributed template, legacy static settings, legacy account-directory
settings and inline settings with no auxiliary config files.

Coverage includes private permissions/symlink rejection for populated inline
values, redacted key formatting, 64 KiB bounds, conflicting sources, missing inline
fields, SDK omissions vs. empty strings, TOML-to-legacy JSON equivalence and
startup/health/status/reload without login. Existing mock role-login tests also
exercise inline device context. No production SDK/game requests were made.
Account passwords remain exclusively in `/accounts`; snapshots and the HTTP v1
contract are unchanged. Main-branch CI for commit `ec797a4` passed both native
architectures and all five container smoke configurations (run `36032528479`).
Release publication requires the versioned main/tag gates in [releasing](releasing.md).

## Alpha.3 Account Directory and Health Bootstrap

2026-09-25: 91 Rust tests (36 client, 3 protocol, 47 server unit, 5 process
integration), two doctests and seven release-script tests passed. Formatting,
all-target Clippy with warnings denied and documentation generation passed.
A local amd64 Docker build reused the cargo-chef dependency layer and passed
smoke tests for missing configuration, normal configuration and empty accounts.
All temporary server processes/containers were stopped. Publication is gated by
the main-branch and tag workflows described in [releasing](releasing.md).

New coverage includes missing/invalid/ambiguous/private account files, size and
directory-scan bounds, account/region state isolation, password redaction,
no account reads on startup/health/status/readiness, concurrent lazy loading,
input-change/SIGHUP rearming, no request replay, saved-session reuse, existing vs.
missing roles, pre-login failure, opted-in recovery and post-login persistence
failure. SDK HTTP primitives retain their loopback regression coverage; the new
directory orchestration uses synthetic saved SDK authorization and mock game
transport. No real password, new online login or additional live role creation
was used to verify this increment. Live end-to-end directory login is not claimed.

Pre-release review reproduced and fixed dangling state pointers initiating login
and repeated unloaded queries hiding the initialization failure category. Both
regressions fail on the previous implementation and pass with the fixes.
The HTTP v1 contract snapshot is unchanged. Configuration examples and lifecycle
details are in [Lazy Account Directory](accounts.md).

## Alpha.2 Operations

Pre-release checks passed: 70 Rust tests (36 client, 3 protocol, 28 server,
1 CLI lifecycle, 2 operator commands), two doctests, seven release-script tests,
fmt, all-target Clippy, documentation generation and Docker build-input check.
A full local amd64 Docker build and offline container smoke also passed. Recovery
faults were simulated; real SDK expiry was not forced on the operator's account.

Review adds a managed recovery layer with explicit operator CLI, scoped snapshots,
atomic state pointer, cross-process authentication lock, no account substitution,
no failed-query replay, one attempt per generation and a cooldown across generations.
Tests include real Client/mock Transport recovery through persistence, generation
invalidation, concurrent admission, reload exclusion, cancellation and fail-closed
post-login persistence errors. No test forces real token expiry or device conflicts.

Public-mode recursive projection, personalized-route rejection, diagnostic authentication,
request IDs, cache blocking and a pinned OpenAPI/route contract are regression-tested.
The operator CLI was exercised with an authorized dedicated account: SDK password
login saved pending authorization; explicit game login saved private SDK/game files.
No password was stored. A separate live gateway smoke verified public profile and
favorite queries, readiness transition, sanitized status/logs and SIGHUP reload;
the service was stopped afterward. Configuration and status commands are offline.

Container and GitHub Actions checks gate alpha.2 publication. Earlier records below
describe their historical code/version and do not override the current contract.

## 2026-09-24 Update

Current source includes GET `/v1` routing and SDK/session persistence. All 54
tests (36 client, 3 protocol, 14 HTTP/cache/query, 1 CLI) and two doctests pass,
along with formatting and all-target Clippy with warnings denied.
Query tests cover repeated array ordering/duplicates, exact 64-bit IDs, optional
presence, dotted filters, enums, invalid percent encoding/UTF-8, duplicate scalars,
unknown fields, size limits, GET bodies, removed routes, method rejection and
cache-key equivalence. OpenAPI declares all fifteen GET operations and parameters.

An authorized smoke test with a saved game session returned HTTP 200 for profile
and announcement GETs, MISS then HIT for the profile, 401 without a key, 400 for
invalid query/body, 404 for the old route and 405 for POST/HEAD. Only two upstream
query calls were needed; no login or game-state mutation was performed. The test
server was stopped afterward. See [live scope](live-validation.md) for prior
authentication and query coverage. No new image or release has been published.

## Initial Validation

Local validation on 2026-09-23, Linux x86_64 (WSL2), pinned Rust 1.98.1.
This record covers the initial framework, SDK exchange, SDK HTTP increments and
local preparation of `0.1.0-alpha.1`, not a stable release.

## Passed

- `cargo fmt --all --check`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace --all-targets`: 42 tests passed
  (30 client, 3 protocol, 8 HTTP/cache, 1 CLI process integration).
- `cargo test --locked --workspace --doc`: two login integration examples compiled.
- `cargo build --locked --release -p moenotes-server`
- `cargo doc --locked --workspace --no-deps`
- Seven Python release-script tests, including version/tag/lockfile consistency,
  stable/prerelease handling, public-package enforcement and manifest validation.
- GitHub Actions workflow syntax checked with actionlint 1.7.12.
- Loopback offline example: health response, authenticated query, MISS followed
  by HIT with the same original fetch timestamp. The example was then stopped.
- Protocol snapshot SHA-256 matches the provenance notice.
- `sh scripts/check-build-inputs.sh`: Docker's actual `inputs` stage exported
  under `.dockerignore`, then the complete exported workspace checked offline.
  Embedded SDK docs and the descriptor/lockfile are present; target/secrets are absent.

Pre-commit review fixed omitted Docker documentation inputs and direct-library
configuration overflow in scheduling/cache expiry. New regression tests failed
before the fixes and pass afterward. CI now covers all targets, doctests,
documentation and the build-input check with read-only repository permissions in
the test job. Tag-only image/release publication has narrowly scoped write access.

The client tests exercise all 18 query methods plus two explicit login methods through a local HTTP/2 gRPC
mock, including 15 business-query request encodings and conditional headers. Other
tests cover protobuf JSON integers/presence/enums/maps/unknown wire fields,
initial and trailing errors, secret redaction, origin binding, Unix secret-file
permissions, cancellation, deadlines, queue limits, session replacement and
blocked sessions. Server tests cover all 15 routes, default-disabled behavior,
authentication, cache TTL/capacity/byte budget/coalescing, full request keys,
stale-generation rejection, errors and CLI startup/shutdown.

Auth tests additionally cover the callback field mapping, pre-login omission of
idToken/client-version metadata, absence of inherited credentials/forced-device
headers, ignoring response device_id, malformed responses, cross-origin/region
rejection, no retries, cancellation/timeouts and competing login generations.

SDK HTTP adds 11 tests for the three POST methods, account-type fields,
root/prefixed paths, headers, signing, RSA encryption, old cached-key preservation,
strict/redacted responses, limits, no redirects/retries, instance binding,
cancellation, timeout, busy admission and rate waiting. Sixteen synthetic form
vectors were generated with the published OkHttp 3.12.13 / Okio 1.15.0 artifacts,
matching the sample's version. The small oracle's Uri.encode step models its
allowlist; it does not execute Android framework code. The RSA roundtrip test
uses one Rust library, not an independent Android implementation.

No game/SDK-server connections or real credentials were used. No
TLS connection to the game, live SDK/login flow or real account response was
validated. gRPC/SDK HTTP mocks use loopback plaintext; production transports
require HTTPS. Tests do not prove live TLS interoperability or account acceptance.

## Container Validation

Earlier local attempts timed out while downloading the Rust builder image.
This gap was closed on 2026-09-23 by GitHub Actions run
[`35872332332`](https://github.com/luoxiadesu/moenotes-api/actions/runs/35872332332)
at commit `645662d`: the full test gate and native Linux amd64/arm64 builds passed.
Each runtime image passed `--version`, UID/GID 65532, private synthetic config,
health, unauthenticated OpenAPI rejection, default-disabled query routes,
`check-config` and graceful shutdown checks. This was a main-branch validation run,
not a registry publication. Subsequent release runs are linked from GitHub Releases.

The cold run compiled cargo-chef, cooked locked dependencies, built the real
application and exported architecture-specific GHA caches. No Docker daemon
settings or unrelated local containers were changed. Container tests still do not
validate game connectivity or live authentication.

## Pending

Locked dependency advisories were inventoried from RustSec and reviewed manually;
see [security boundaries](../SECURITY.md). The RSA private-key timing advisory is
still present upstream, although production uses public-key encryption only.
No successful cargo-audit run is claimed. A staged-file scan found no recognized
secret patterns, private paths or executable analysis artifacts; this is not a
guarantee of legal redistribution rights or exhaustive secret detection.

At the initial validation stage, SDK authorization/renewal, live requests and a
public-field policy were pending. Subsequent results are recorded above and in
live-validation.md; complete SDK consent/renewal and upstream limits remain open.
The HTTP v1 baseline is now defined in api-stability.md; Rust APIs remain experimental.
