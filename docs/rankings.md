# Ranking integration guide

This guide describes the HTTP `/v1` contract in `0.1.0-alpha.4` for downstream
ranking viewers and cutoff tools. It was checked against the request validators,
protobuf descriptor, JSON serializer and public-field policy on 2026-09-27.
Examples use **synthetic data**, not live ranking observations.

alpha.6 also supports `/v1/event/123/ranking/1,10,100` and other
[readable path aliases](path-routes.md). Prefix `/v1/en/` to select a configured
region. Response schemas and rank-matching rules below are unchanged.

## Event point cutoffs

```http
GET /v1/event/ranking?eventId=123&ranks=1&ranks=10&ranks=100
Authorization: Bearer <HTTP_API_KEY>
```

Use GET with no body. The key is issued by the gateway operator; callers do not
send game credentials. The default backend determines the region and upstream
account unless the path explicitly selects another configured region; there is
no region query parameter.

| Parameter | Type | Required | Local validation |
| --- | --- | --- | --- |
| `eventId` | Decimal string, signed int64 | Yes | `1` through `9223372036854775807` |
| `ranks` | Repeated positive int32 | Yes | 1–100 values, each `1` through `2147483647` |

Repeat `ranks` for each desired position. `ranks=1,10`, `ranks[]=1`, JSON arrays,
unknown parameters and duplicate scalar parameters are rejected. Order and
duplicates are preserved. The 100-value limit is a gateway batch limit, **not**
evidence that the upstream accepts any rank up to int32's maximum. Depth beyond
100, ties and missing-rank behavior have not been verified live.

Obtain the event ID from the matching region's `MasterEvent` data. An
`ActivityConfig.activity_id` or challenge music ID is not an interchangeable ID.
This gateway has no event-discovery HTTP route and does not select a current
event automatically. Event names, schedules, ranking-disabled settings and reward
cutoffs must come from master data separately.

### Success body

HTTP 200, `Content-Type: application/json`. The body is a protobuf JSON object
directly, with no `data`, `code` or `message` wrapper:

```json
{
  "ranking": [
    {
      "profile": {
        "id": "example-player-id-a",
        "name": "Example Player A",
        "rankExp": 12345,
        "profileId": "9007199254740993"
      },
      "rank": 1,
      "point": 1234567
    },
    {
      "profile": {
        "id": "example-player-id-b",
        "name": "Example Player B",
        "profileId": "9007199254740994"
      },
      "rank": 10,
      "point": 987654
    }
  ]
}
```

The example intentionally has no row for requested rank 100. It illustrates how
to handle a partial response, not a verified upstream rule for that rank.

| Field | JSON type | Meaning |
| --- | --- | --- |
| `ranking` | Array of objects, optional | Returned event ranking rows, in upstream order |
| `ranking[].rank` | Number, int32, optional | Upstream rank; use this to match requested positions |
| `ranking[].point` | Number, int32, optional | Event points at the returned rank; not a song score |
| `ranking[].profile` | Object, optional | Player's simple profile; not a full player snapshot |

The body does not echo `eventId` or requested ranks and has no total count,
`maxRank`, own rank, own points, event title, update timestamp or historical series.
Keep request context in your application. The event response has the same current
known fields in public and raw modes; raw mode does not add a separate own-rank
query. The [public policy](api-stability.md) is the downstream contract.

### Profile fields

All fields below are optional because protobuf defaults and absent nested
messages are omitted. All listed fields are allowed in public mode.

| Field inside `profile` | JSON type | Meaning |
| --- | --- | --- |
| `id` | String | Upstream player identifier; preserve it exactly |
| `name` | String | Player name |
| `rankExp` | Number, int32 | Player rank experience, not the displayed player level or leaderboard rank |
| `lastUpdatedAt` | Decimal string, int64 | Profile update time; native profile consumers interpret Unix seconds, not the ranking fetch time |
| `profileId` | Decimal string, int64 | Profile ID for `/v1/profile?playerProfileId=...` |
| `favoriteMemberCardMasterId` | Decimal string, int64 | Favorite member card's master ID |
| `favoriteMemberCard` | Object | Card details described below |
| `profileCard` | Object | Profile card described below |

`favoriteMemberCard` may contain `cardId` (int64 decimal string) and `exp`,
`awakeCount`, `cardRank`, `liveSkillLevel`, `performanceSkillLevel` (int32 numbers).
`profileCard` may contain `slot` (int32 number), `name` (string), and
`thumbnailUrl` (**array** of strings).

For an event deck, use `/v1/event/deck?playerId=...&eventId=...`, taking `playerId`
from this row's `profile.id`. Do not substitute `profileId`, an account ID or a
login credential's player ID. A decimal-looking string does not establish that
two ID domains are interchangeable. The gateway does not add card names, image
URLs from master data, player levels or computed deck power.

### Empty results, defaults and rank matching

An empty repeated `ranking` field is omitted, so a successful empty body is:

```json
{}
```

After checking HTTP success, normalize `body.ranking ?? []`. Empty strings, zero
scalar values and empty arrays are normally omitted too. A returned row without
`point` has the protobuf value `0`; a requested rank with **no returned row** has
no observed point value. Do not convert that missing row into a zero cutoff.
A missing or zero `rank` cannot identify a positive requested position. Absent
`profile` differs from a present empty `profile: {}`.

Match by each row's `rank`, never by array index or by zipping request and response
arrays. Keep response order and duplicate-rank rows; do not silently pick the
first/last duplicate or infer tie rules. A missing position means only “not
returned in this response,” not “no player exists at this rank.” An HTTP failure
is not an empty successful snapshot.

## Fetch time and cache

| Header | Meaning |
| --- | --- |
| `X-Moenotes-Fetched-At` | Original successful upstream fetch time, Unix **milliseconds**, encoded as header text |
| `X-Moenotes-Cache` | `MISS`, `HIT` or `COALESCED` (shared in-flight request) |
| `X-Request-Id` | Gateway request correlation ID |
| `Cache-Control` | `no-store` |

The default internal cache TTL is 15 seconds and is operator-configurable. A cache
hit keeps the original fetch time; the header is not an upstream aggregation
timestamp or proof that points changed at that time. Parameter order/duplicates
within `ranks` participate in cache identity. Errors are not cached, and stale
successes are not substituted on failure. No polling cadence is guaranteed safe
by the upstream; the gateway has no background ranking history collector.

## TypeScript types and a request example

Keep int64 values as strings. Do not pass IDs through JavaScript `Number`, which
cannot represent all int64 values exactly. These types describe parsed JSON;
they do not perform runtime validation.

```ts
type Int64String = string;

interface DeckMemberCardDetail {
  cardId?: Int64String;
  exp?: number;
  awakeCount?: number;
  cardRank?: number;
  liveSkillLevel?: number;
  performanceSkillLevel?: number;
}

interface PlayerSimpleProfile {
  id?: string;
  name?: string;
  rankExp?: number;
  lastUpdatedAt?: Int64String;
  profileId?: Int64String;
  favoriteMemberCardMasterId?: Int64String;
  favoriteMemberCard?: DeckMemberCardDetail;
  profileCard?: { slot?: number; name?: string; thumbnailUrl?: string[] };
}

interface EventRankingEntry {
  profile?: PlayerSimpleProfile;
  rank?: number;
  point?: number;
}

interface EventRankingResponse {
  ranking?: EventRankingEntry[];
}
```

The following JavaScript works in a server-side runtime with `fetch` (Node.js
18+). Supply the operator's base URL/key and a real event ID. The gateway grants
no browser CORS permission; a web frontend needs its own backend integration.

```js
async function fetchEventRanking(baseUrl, apiKey, eventId, ranks) {
  const url = new URL("/v1/event/ranking", baseUrl);
  url.searchParams.set("eventId", eventId); // Already a decimal string.
  for (const rank of ranks) url.searchParams.append("ranks", String(rank));

  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${apiKey}` },
    signal: AbortSignal.timeout(15_000),
  });
  const requestId = response.headers.get("x-request-id");
  if (!response.ok) {
    const error = await response.json().catch(() => null);
    throw new Error(
      `HTTP ${response.status}: ${error?.error?.kind ?? "unknown"}` +
      ` (request ${requestId ?? "unknown"})`,
    );
  }
  const body = await response.json();
  const rows = body.ranking ?? [];
  const byRank = new Map();
  for (const row of rows) {
    if (!Number.isInteger(row.rank) || row.rank <= 0) continue;
    const matches = byRank.get(row.rank) ?? [];
    matches.push(row); // Preserve multiple rows at the same rank.
    byRank.set(row.rank, matches);
  }
  return {
    eventId,
    rows,
    requested: ranks.map((rank) => ({
      rank,
      returned: byRank.has(rank),
      entries: byRank.get(rank) ?? [],
    })),
    fetchedAtUnixMs: response.headers.get("x-moenotes-fetched-at"),
    cache: response.headers.get("x-moenotes-cache"),
    requestId,
  };
}
```

The returned `requested`/metadata object is a **consumer-side convenience shape**,
not the gateway's response body. Handle missing points on an existing row as
`entry.point ?? 0`; keep a missing row as missing. The example makes one request
and does not start a poller or retry loop.

## Errors

Gateway query errors use a non-2xx status and a small JSON body, for example:

```json
{"error":{"kind":"maintenance"}}
```

| HTTP status | Typical kind / handling |
| --- | --- |
| 400 | `invalid_request`: fix parameters or remove the GET body |
| 401 | `unauthorized`: missing/invalid gateway bearer key |
| 429 | `queue_full`: local admission limit; avoid immediate retry loops |
| 502 | `business`, `transport`, `protocol`: upstream failure, not an empty ranking |
| 503 | `authentication_required`, `authentication`, `maintenance`, `version`, `device_conflict`, `session_changed`, `cancelled`; health-only startup uses `unconfigured` |
| 504 | `timeout`: the operation deadline expired |

Unknown/disabled paths can return 404 and unsupported methods 405 without a JSON
error body. Reverse proxies can also return non-JSON failures. Check the status
before reading ranking fields. Game authentication failures are 503, not 401.
Record `X-Request-Id` for operator diagnostics. See [HTTP API](http-api.md) for
the full shared contract.

## Other ranking routes are different schemas

The table describes default **public mode**. Every field remains subject to
protobuf default omission; sample shapes below are structural, not real responses.

| Route | Parameters | Response fields |
| --- | --- | --- |
| `/v1/event/ranking` | `eventId`, repeated `ranks` | `ranking[]`: `profile`, `rank` (int32), `point` (int32) |
| `/v1/event/challenge-ranking` | `challengeMusicId` | `players[]`: `playerData`, `score` (int32), `highScoreDeck` |
| `/v1/music/ranking` | `musicId` | `players[]`: `playerData`, `score` (int32), `highScoreDeck` |
| `/v1/arena/ranking` | `arenaSeasonId`, `rankingStart`, `rankingEnd`; optional `bandId` | `ranking[]`: `profile`, `deck`, `rank` (int32), `point` (int32); `maxRank` (int64 string) |

Music/challenge rows have no `rank` field, nor do those routes accept a requested
rank range or difficulty. List position is not a documented tie-aware server rank.
Raw mode additionally exposes music `myRank` (int32) and challenge `myRank` and
`myScore` (both int32); public mode removes them.

Arena request fields are int64 decimal strings. `arenaSeasonId` and range bounds
must be positive, `rankingEnd >= rankingStart`, and
`rankingEnd - rankingStart < 100`; `bandId`, if supplied, must be nonnegative.
These are local validation rules; upstream endpoint inclusivity remains unverified.
Omitting `bandId` differs from explicitly supplying zero. The gateway does not
apply the native UI's master-data rank filtering or renumber the returned rows.

## OpenAPI for developer tools

Import the instance's authenticated `GET /openapi.json` response into a tool that
supports **OpenAPI 3.1**, such as Swagger UI or Postman. Configure its server/base
URL and bearer authentication separately; never put the API key in the URL.
The schema reflects the instance's public/raw response mode.

To export a full public-mode document locally without credentials or upstream
requests, run from the repository root (normal Cargo dependency access may be
needed if the build cache is empty):

```sh
cargo run --locked -q -p moenotes-server --example openapi > openapi.json
```

The schema includes response nesting and repeated-key query encoding. It does
not encode every local constraint or rank-matching rule, so retain this guide's
validation and missing-row handling. Generated clients should tolerate omitted
defaults and unknown future response fields. The checked-in contract fixture is
for compatibility tests; use the endpoint/export for tool imports.

## Validation boundary

Field types, serialization, projection, request parsing and cache behavior have
local implementation/contract evidence. The recorded live batch on 2026-09-24
returned 100 song-ranking entries for one music ID. Event, challenge and arena
ranking requests were not sent in that batch because sampled master tables had
no applicable IDs. This documentation review made no live game requests.

Ongoing-event responses, deep cutoffs, ties, missing positions, upstream refresh
frequency and rate limits still need authorized live validation. See
[live validation scope](live-validation.md) and [API stability](api-stability.md).
