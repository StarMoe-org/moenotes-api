# HTTP API

For event cutoff fields, optional/default values, rank matching and a downstream
request example, see the [ranking integration guide](rankings.md).

The current source uses short GET routes under `/v1`. This replaces the original
`POST /experimental/v1/...` contract; old routes are not retained or redirected.
Published `0.1.0-alpha.1` images use the old contract. The alpha.2 baseline and
compatibility rules are documented in [API stability](api-stability.md).

Rust cache options are limited to a one-hour TTL and 16,384 entries, matching CLI
configuration limits. Invalid options fail before upstream work is scheduled.

The documented v1 baseline is compatibility-controlled. The service
is designed for one operator-configured game session per process. Use separate
processes and separate configuration for separate regions. All authorized HTTP
callers use that same upstream identity; API keys are not game credentials.

## Access and Data Boundary

`GET /health` and `GET /healthz` are unauthenticated and report process liveness,
not upstream health. Missing required configuration starts a health-only service:
both return 200, while readiness/business URLs return 503 `unconfigured`. No
upstream clients or calls are created in that mode. See [startup](operations.md#missing-configuration).
`GET /openapi.json` requires a bearer API key and describes the experimental routes
even when disabled. Query routes always require `Authorization: Bearer ...`.
`response_mode` defaults to `public`; `raw` is a trusted-operator opt-in and
`disabled` turns queries off. Legacy `enable_experimental_raw=true` selects raw.
Use the configuration/secret files locally; there is no remote management endpoint.

HTTP keys must be 32-4096 ASCII non-whitespace characters, stored separately from
game credentials. Secret files may have a trailing newline. The process retains a
SHA-256 hash of the HTTP key and compares hashes in constant time. Restart the
process to rotate a key or reload configuration. Never pass keys as URL parameters.

Public mode recursively whitelists fields, excludes myRank/myScore/isSentFavorite,
and rejects personalized circle recommendations with 403. It does not anonymize
public player IDs or guarantee account-independent results. Raw response mode is
deliberately not a shared public projection. Account-specific
fields are retained, including recommendation results and favorite status. Only
enable it for trusted callers until real response comparisons establish a suitable
public-field policy. Announcement HTML is returned as a JSON string, never executed;
consumers must not treat it as sanitized HTML. No CDN authorization values are added.

## Queries

All query routes use **GET without a request body**, with URL query parameters.
Names use the protobuf lowerCamelCase spelling. IDs are decimal text, parsed without
floating-point conversion; do not interchange profile, account and player IDs.
Repeated fields use repeated keys, not commas, brackets or JSON arrays:
`ranks=1&ranks=10&ranks=100`. Order and duplicate values are retained.
Unknown fields, duplicate scalar fields, malformed URL encoding and invalid UTF-8
are rejected. Enum values accept their exact name or decimal integer value.

Nested circle filters use dotted names such as `options.name`. An unfiltered
circle search automatically includes an empty `options` message. Use normal URL
encoding for text; a literal `+` must be encoded as `%2B`.

| Route under `/v1` | Query parameters |
|---|---|
| `/profile` | `playerProfileId` |
| `/profiles` | repeated `accountIds` |
| `/profile/favorites` | `playerId` (string) |
| `/announcements` | optional `selectedTab`: ALL, OPENING_EVENTS, BUG; default ALL |
| `/announcement` | `id` |
| `/gacha/rates` | `gachaId`, optional repeated `selectedPickUp` |
| `/music/ranking` | `musicId` |
| `/event/ranking` | `eventId`, repeated `ranks` (int32) |
| `/event/challenge-ranking` | `challengeMusicId` |
| `/event/deck` | `playerId` (string), `eventId` |
| `/arena/ranking` | `arenaSeasonId`, `rankingStart`, `rankingEnd`, optional `bandId` |
| `/arena/deck-trend` | `musicId`, `arenaSeasonId` |
| `/circle` | `circleId` (uint64) |
| `/circles/recommended` | None |
| `/circles/search` | optional `options.name`, `options.memberRange`, `options.joinRule`, `options.playStyle` |

Examples with synthetic IDs:

```http
GET /v1/profile?playerProfileId=12345678901
Authorization: Bearer <HTTP_API_KEY>
```

```http
GET /v1/event/ranking?eventId=123&ranks=1&ranks=10&ranks=100
Authorization: Bearer <HTTP_API_KEY>
```

The deprecated probability parameter `productId` is accepted only as zero;
new integrations should omit it. `GET /openapi.json` provides the complete query
parameter schema using OpenAPI `style: form`, `explode: true`, suitable for
Swagger/Postman imports. A GET body is rejected with 400. Other query methods,
including HEAD, return 405 without performing a query; removed paths return 404.

`GET /readyz` and `GET /v1/status` require the same bearer key and provide sanitized
readiness and diagnostics. They never initiate upstream probes. Every HTTP response
has a server-generated `X-Request-Id`. See [operations](operations.md).

There is no HTTP route for arbitrary RPCs, session credentials, Whoami, raw server
discovery or version management. Client-library support methods are independent.
The OpenAPI schema is generated from the bundled descriptor; it describes protobuf
fields, while the following local admission limits are additional policies.

Query strings are limited to 8,192 encoded bytes and 256 parameter pairs; at most 64 authenticated HTTP handlers can
run concurrently. The cache admits at most 32 different inflight keys; duplicate
requests share the same operation. Additional work receives HTTP 429. Query
validation uses the [local client limits](client.md); none are asserted server limits.

## Responses, Cache and Errors

Successful response bodies contain the selected-mode protobuf JSON object directly,
without a `data` envelope. Defaults are omitted per protobuf JSON; int64/uint64
are strings, maps are JSON objects, known enums are names and unknown enums are
numbers. Public mode drops unapproved and account-specific fields; raw mode leaves
the upstream JSON intact. No enrichment or UI formatting occurs.

Headers:

- `X-Moenotes-Fetched-At`: original successful fetch time, Unix milliseconds.
- `X-Moenotes-Cache`: `MISS`, `HIT` or `COALESCED`.
- `Cache-Control: no-store`: callers/proxies must not turn raw data into a shared cache.

Internal caching defaults to 15 seconds, 1024 entries and a 64 MiB serialized-JSON
budget (an accounting bound, not an exact process RSS limit). Eviction is least
recently touched. Keys include the random session generation, method and complete
encoded request. Region/origin/identity are immutable within a generation.
Errors are never cached, expired successes are never served on failure, and replacing
the session invalidates old entries. A caller disconnect does not abort a shared
inflight query; it continues within the client deadline, which can still fill cache.
Graceful process shutdown cancels pending work. Process restart clears all cache.

Locally blocked authenticated sessions cannot serve cached authenticated responses.
Anonymous support queries remain callable. Recovery changes the session generation.

Error body: `{"error":{"kind":"maintenance"}}`. No raw upstream body or status
message is exposed. HTTP 401 means the HTTP key failed, **not** game authentication.

| HTTP status | Meaning |
|---|---|
| 400 | Invalid query encoding/parameter, nonempty body or local validation failure |
| 401 | Missing/invalid/duplicate bearer authorization |
| 403 | Response policy prohibits this route |
| 404 | Unknown or disabled route |
| 405 | Unsupported method; read-query routes accept GET only |
| 429 | Local HTTP/inflight/upstream admission limit reached |
| 502 | Upstream transport, protocol or other business failure |
| 503 | Missing/rejected game session, maintenance, version/device conflict, cancellation or session change |
| 504 | Operation deadline exceeded |

The gateway does not translate unknown business errors into empty success. Opt-in
recovery may perform one background game login after token rejection; it never
replays the failed query or polls. SDK reauthentication remains an operator command.

## Deployment

Loopback is the default. For remote access, terminate TLS at a trusted reverse proxy,
limit request rate and connection/body-read timeouts there, and restrict network
access. No browser CORS permission is granted. Query strings can contain account
identifiers or search text and may enter browser history or proxy logs. Keep API
keys in the Authorization header, never in URLs; redact query strings and headers
in access logs. `Cache-Control: no-store` remains set on query responses despite
the GET transport. Live visibility, account isolation semantics,
query depth, refresh rate and server error behavior still need authorized validation.
