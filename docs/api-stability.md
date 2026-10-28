# API Stability

Starting with `0.1.0-alpha.2`, HTTP `/v1` has an explicit compatibility baseline,
independent of the game's evolving protocol. Package versions use MAJOR.MINOR.PATCH
with SemVer prerelease suffixes. This is still a prerelease, not a production SLA.

## Contract

The sixteen GET paths, documented query names, repeated-key array encoding, integer
precision, authentication, error categories, cache/correlation headers, public
field policy and no-store behavior form the v1 baseline. Operator command names,
configuration keys and snapshot formats are also compatibility-controlled.

Compatible additions may introduce optional parameters/endpoints/diagnostic fields
or explicitly reviewed response fields. Consumers must ignore unknown response
fields and preserve unknown upstream enum values. Required parameters, renames,
removals, type changes or semantic reinterpretations require a new HTTP major
namespace and migration notes. Security fixes may narrow disclosure immediately,
with a changelog notice.

Raw responses and generated protobuf types track the upstream, not a stable public
projection. New upstream fields do not enter public mode without explicit review.
Rust libraries remain experimental in 0.x: breaking library changes require an
explicit changelog entry and a new minor version after the current alpha series.

## Migration

`response_mode="public"` is now the default. It filters account-specific and
unapproved fields, omits myRank/myScore/isSentFavorite, and rejects personalized
circle recommendations with 403. It does not anonymize public player data or prove
all responses independent of the operator. Bearer authentication remains required.

`response_mode="raw"` is for trusted operators. Legacy
`enable_experimental_raw=true` aliases raw; conflicting settings fail validation.
`response_mode="disabled"` leaves only support/diagnostic routes. Review existing
callers that consumed private fields when migrating to public mode.

alpha.1 POST `/experimental/v1/...` routes are removed, not redirected. Use the GET
paths in [HTTP API](http-api.md). Contract tests pin routes, parameters and public
schemas in `server/tests/fixtures/v1-openapi.json`; CI never regenerates the fixture.
Changes to that snapshot require explicit compatibility review. Package-version
metadata is excluded to avoid irrelevant version-update churn.

alpha.6 adds readable path aliases and explicit-region paths without changing the
established query endpoints. The baseline test compares every original path and
schema to the existing snapshot; additive paths have separate routing/OpenAPI
tests. Automatic profile-region inference is limited to the new profile path.

### 1.1.0 protocol snapshot (unreleased)

The bundled descriptor moved from the 1.0.1 to the 1.1.0 sample: 125 files,
57 services and 276 unary methods (previously 123/56/261). Every change is
additive except two methods the 1.1.0 client no longer declares, which are
retained deliberately (see below). Reviewed public-contract effects:

- New route `GET /v1/event/challenge-total-ranking?eventId=...` and its readable
  alias `/v1/event/{eventId}/challenge-total-ranking`, including region prefixes.
  It is a new endpoint, not a reinterpretation of an existing one.
- `/v1/arena/deck-trend` gains the optional `bandId` int32 parameter, because the
  1.1.0 `DeckTrendRequest` adds field 3. The gateway forwards it unchanged and
  still accepts requests without it; omitting it is not the same as sending 0.
- The same request marks `musicId` as deprecated in the descriptor, so the
  generated OpenAPI parameter now reports `deprecated: true`. The parameter is
  still required and still forwarded; nothing is removed or renamed.
- `entity.Announcement` gains `platform` in 1.1.0. It is **not** in the public
  allowlist, so it is pruned from the public OpenAPI schema and never returned in
  public mode. Raw mode may expose it.
- `app.player.PlayerService/Whoami` and `app.membercard.MemberCardService/LinkSkillLevelUp`
  are absent from the 1.1.0 client descriptors. The snapshot retains both
  definitions (plus `WhoamiRequest`/`WhoamiResponse`) from 1.0.1 so the existing
  `whoami` client operation and identity-probe callers keep compiling and their
  wire contract does not change. Whether a 1.1.0 server still answers `Whoami`
  is **unverified**; treat a failure as an upstream error, not as a local bug.
  `LinkSkillLevelUp` has no gateway route and no client operation.

The 1.1.0 APK sample is a third-party re-signed build; its client descriptors are
used as protocol evidence, not as proof of official binary equivalence.
