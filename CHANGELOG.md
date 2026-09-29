# Changelog

Version numbers follow MAJOR.MINOR.PATCH. Pre-release APIs are experimental.

## 0.1.0-alpha.8 - 2026-09-29

- Add opt-in `accounts.strategy = "round_robin"` within each region, with an
  independent client, session, queue and cache per account. Preserve single-account
  selection by default. Skip blocked sessions and cool down transient failures
  without replaying failed queries. Add sanitized per-session dispatch counters.
- Pool membership is discovered at startup (maximum 32 accounts per region);
  SIGHUP reloads existing members independently. Readiness accepts any ready
  member; multi-session status uses `pool.members` with the legacy `session` null.
- Add `strategy` to the experimental Rust `AccountsConfig` struct; direct
  literals must supply `AccountStrategy::Single` to preserve previous behavior.
  HTTP query paths, public projection and session snapshot formats are unchanged.

## 0.1.0-alpha.7 - 2026-09-29

- Add JP credential import under `/accounts/jp`, and organize new international
  account-password defaults under `/accounts/international` (explicit legacy paths
  stay compatible). JP never invokes SDK login or automatic account recovery.
- Add JP protocol isolation, version response-header handling, request-header
  policy, profiles service mapping and explicit `/v1/jp/` routes.
- Add explicit `jp-register`, local registration-response import, `jp-check` and
  `init-accounts` operator commands. No public account-creation endpoint.

## 0.1.0-alpha.6 - 2026-09-28

- Add readable path aliases for the query API, comma-separated path lists and
  explicit `/v1/tw|en|kr/...` selection. Preserve existing query routes and fields.
- Route the new 11-digit profile path by observed prefixes 2/3/4. Reject unknown
  ranges, explicit-region mismatches and missing regional configuration without
  falling back or sending credentials to another region.
- Support independent regional sessions, credentials, lazy login, version polling,
  recovery, caches and SIGHUP reload in one private config. HTTP auth and response
  policy stay global. Expose per-region diagnostics and extend OpenAPI/docs.
- Add `regions` to the experimental Rust `Config`; direct struct literals must
  include the field. Existing session snapshot and HTTP query contracts stay intact.

## 0.1.0-alpha.5 - 2026-09-28

- Discover master/resource versions anonymously at startup and every 60 seconds
  in configured servers. Atomically update the pair, preserve credentials, reject
  stale cache/work and clear only explicit master-version mismatches. Retain
  authentication retry budgets and expose sanitized version-sync diagnostics.
- Generate private configuration templates on first startup, add `init-config`
  and `config-path`, and support `MOENOTES_CONFIG`. New containers default to
  persistent `/var/lib/moenotes/config.toml`; existing legacy mounts still work.
- Include the template in images and add a Compose example and Zeabur operations
  instructions. Account passwords remain separate; no automatic SDK renewal.
- Add `version_sync` to the experimental Rust `Config` and diagnostic `Status`
  types; direct struct literals must include the new field. HTTP v1 query and
  snapshot formats are unchanged. `MOENOTES_BOOTSTRAP_LISTEN` now takes precedence
  over a partial config's listen address in health-only mode.

## 0.1.0-alpha.4 - 2026-09-25

- Support inline `api_key`, `[login.context]` and `[login.sdk_http]` in one private
  config file; preserve legacy file sources and reject conflicting sources.
- Keep passwords in `/accounts` and persisted session state separate. Require
  private permissions for populated inline secrets, with redacted secret wrappers.
- Add explicit SDK `omit_common` for TOML null semantics without changing wire
  encoding. Extend health-only startup to missing inline fields and blank templates.
- Add fields to experimental Rust `Config`/`LoginConfig`; direct struct literals
  must provide the new optional fields. HTTP v1 and snapshot formats are unchanged.

## 0.1.0-alpha.3 - 2026-09-25

- Add an opt-in lazy `/accounts` email/password JSON source. Health/status and
  startup never load account passwords or trigger login. Missing files remain live;
  protected queries initialize one selected account/region with single-flight.
- Check regional role existence before login; automatically create only missing
  roles by default, with an option to disable creation. Require SDK-readiness
  confirmation, isolate persisted state, reuse saved sessions and never replay the
  triggering query or loop on unchanged failed inputs. Add SIGHUP rearming.
- Validate account-file schema, private permissions, selection and size bounds;
  keep passwords outside logs/snapshots and reject implicit account switching.

- Keep an unconfigured server/container alive in health-only mode. `/health` and
  `/healthz` return 200 without initializing or probing upstream clients. Log all
  missing required fields/files by name; business and readiness routes return 503.
- Preserve strict `check-config` validation and reject malformed configuration or
  unsafe API-key files. Add empty-container startup and graceful-shutdown checks.
- Reject dangling/nonregular session pointers before any login, and preserve the
  initialization failure category across repeated queries with unchanged inputs.
- Keep the existing HTTP `/v1` routes, query parameters and response contract.

## 0.1.0-alpha.2 - 2026-09-24

- Add operator `sdk-login`, `game-login`, `auth-status` commands, private login
  configuration, explicit SDK-readiness/account-creation flags and scoped snapshots.
- Add opt-in bounded game credential recovery from approved SDK authorization,
  single-flight/cooldown, no original-query replay, account identity checks and
  fail-closed persistence. SDK expiry requires explicit operator login.
- Add SIGHUP credential reload, authenticated readiness/status, correlation IDs
  and sanitized JSON diagnostics. Reject old authenticated cache after local blocks.
- Default HTTP responses to a recursive public-field whitelist; retain explicit raw
  mode and disable personalized recommendations in public mode.
- Establish the `/v1` compatibility baseline and pin its public OpenAPI contract.

- **Breaking HTTP change:** replace POST `/experimental/v1/...` queries with short
  GET `/v1/...` resource routes and URL parameters; remove old routes without redirects.
  Arrays use repeated keys and nested filters use dotted names. Preserve upstream
  gRPC, bearer authentication, protobuf JSON responses and no-store/cache behavior.
- Generate GET query parameters in OpenAPI and reject ambiguous parameters,
  invalid encoding, oversized URLs and GET request bodies before upstream work.

- Accept SDK RSA public keys with nonstandard Base64 line widths and no final
  newline, while preserving strict SPKI decoding and key-size limits.
- Decode integer SDK user IDs losslessly, including values above the JavaScript
  safe-integer range; reject fractional and overflowing numeric IDs.
- Add explicit private SDK authorization persistence and generation-checked game
  session export, compatible with static credential loading; no automatic saves.

## 0.1.0-alpha.1 - 2026-09-23

- Add descriptor-based protocol generation and protobuf JSON reflection.
- Add allowlisted read-query client with static credential injection and session isolation.
- Add opt-in raw HTTP queries, API key access, bounded cache and OpenAPI.
- Add offline protocol, loopback gRPC, session and HTTP/cache tests.
- Add redacted, origin/region-bound Android OneSDK callback import and explicit
  pre-login/game-login methods, outside the query allowlist and HTTP routes.
- Preserve native probe/login field differences and atomically rotate session
  generations only after validated login responses; never force device override.
- Add offline auth mapping, malformed-response, binding, cancellation and race tests.
- Add explicit SDK HTTP RSA, password and cached-key login primitives with
  recovered POST encoding/signing, instance-bound RSA challenges, bounded HTTPS
  transport and redacted pending results. No automatic game-session conversion.
- Add same-version OkHttp form vectors and loopback SDK HTTP regression tests.
- Fix Docker build inputs to include embedded SDK documentation; check the
  actual Docker context with an offline build before building the runtime image.
- Reject excessive scheduling intervals and cache lifetimes/capacities through
  direct Rust APIs, preventing duration-overflow panics and failed cache flights.
- Document security boundaries and the remaining RSA dependency advisory.
- Add native Linux amd64/arm64 container builds, offline container smoke tests,
  public GHCR publication and automated GitHub prereleases after CI passes.
- Cache Rust test artifacts and export cargo-chef dependency layers through
  architecture-scoped GitHub Actions BuildKit caches.
- Add matching manifest/lockfile/tag/changelog checks, OCI metadata, immutable
  digest release assets and a `moenotes-server --version` command.

No live API compatibility, complete SDK login flow, automatic renewal or stable public API is
claimed. No official `0.1.0` release has been made.
