# API Stability

Starting with `0.1.0-alpha.2`, HTTP `/v1` has an explicit compatibility baseline,
independent of the game's evolving protocol. Package versions use MAJOR.MINOR.PATCH
with SemVer prerelease suffixes. This is still a prerelease, not a production SLA.

## Contract

The fifteen GET paths, documented query names, repeated-key array encoding, integer
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
