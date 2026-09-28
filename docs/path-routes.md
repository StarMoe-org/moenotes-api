# Path Routes and Automatic Profile Regions

JP is supported through explicit `/v1/jp/...` routes. Its profile path accepts a
positive int64 without international prefix inference. The unscoped automatic
profile path keeps the 2/3/4 rules below; it never guesses JP from a prefix.

Available in alpha.6. Use GET, an empty body and the gateway bearer key. The
response fields are identical to the corresponding existing query endpoint.

```http
GET /v1/profile/20000000001
Authorization: Bearer <HTTP_API_KEY>
```

The profile path accepts exactly 11 ASCII digits. Its first digit selects the
configured region: `2` → `tw` (TW/HK/MO), `3` → `en`, `4` → `kr`. This routing rule
is based on observed regional number ranges; it is not an official guarantee of
all future ID allocations. Unknown prefixes are rejected, never guessed or sent
to every region. Only this profile path infers a region; account IDs, player UUIDs,
music/event/circle IDs do not.

`/v1/tw/profile/20000000001` is also supported. The explicit region must agree
with the profile prefix. Successful automatic/explicit-region responses include
`X-Moenotes-Region: tw|en|kr`.

## Calls Without Query Parameters

The following synthetic examples use the default configured region except for
profile lookup. Put `/tw`, `/en` or `/kr` after `/v1` to select another configured
region, e.g. `/v1/en/event/123/ranking/1,10,100`.

| Operation | Example path |
| --- | --- |
| Profile (automatic region) | `/v1/profile/20000000001` |
| Batch player briefs | `/v1/profiles/20000000001,20000000002` |
| Favorite status (player ID) | `/v1/player/example-player-id/favorites` |
| Announcements, all | `/v1/announcements` |
| Announcements, selected tab | `/v1/announcements/BUG` |
| Announcement detail | `/v1/announcement/12` |
| Gacha rates | `/v1/gacha/1/rates` |
| Gacha rates with selected pickups | `/v1/gacha/1/rates/2,3` |
| Music ranking | `/v1/music/69/ranking` |
| Event point cutoffs | `/v1/event/123/ranking/1,10,100` |
| Challenge ranking | `/v1/event/challenge/8/ranking` |
| Event deck (player ID) | `/v1/event/123/deck/example-player-id` |
| Arena ranking range | `/v1/arena/7/ranking/1/100` |
| Arena ranking with band filter | `/v1/arena/7/ranking/1/100/band/2` |
| Arena deck trend | `/v1/arena/7/music/69/deck-trend` |
| Circle detail | `/v1/circle/123` |
| Circle search by name | `/v1/circles/search/Example%20Circle` |
| Circle search, no filters | `/v1/circles/search` |
| Personalized circle recommendations (raw mode only) | `/v1/circles/recommended` |

Lists in a path use commas and retain order and duplicates. IDs remain decimal
strings, without floating-point conversion. Empty list items, more than 100 items,
out-of-range numbers and invalid request combinations fail the existing validators.
Path components must be URL-encoded; a literal `+` in a path remains `+`.
For circle names, a name can equal a reserved word because it follows `/search/`.
`/v1/profile/favorites` retains its existing query endpoint and is not a profile ID.

Optional filters can still use `?`, e.g.
`/v1/circles/search/Example?options.memberRange=0`. Do not repeat a path-bound
field in the query string, even for lists: `/event/123/ranking/1,10?ranks=100`
is rejected. Existing query endpoints remain compatible, including repeated query
keys instead of comma lists. Explicit-region versions of those endpoints also
exist, e.g. `/v1/en/profile?playerProfileId=30000000001`.

## Region Configuration and Failures

The default `[session]` is registered automatically when its region is `hk`, `tw`,
`hk-tw-mo`, `en` or `kr`. The three TW/HK/MO names all route through `/tw`; preserve
the existing session-region label to match stored credential bindings. Configure
additional backends with `[regions.en]`, `[regions.kr]` or `[regions.tw]`, as shown
in [configuration](configuration.md#multiple-regions). Each backend has independent
credentials, version polling, recovery, request scheduling and cache. HTTP key,
response policy and admission limits remain global. No credentials are copied
from the default region. A failed or unconfigured region never falls back elsewhere.

| HTTP status | Error kind | Meaning |
| --- | --- | --- |
| 400 | `invalid_profile_id` | Profile path ID is not exactly 11 ASCII digits |
| 400 | `unsupported_profile_region` | Profile prefix is outside 2/3/4 |
| 400 | `profile_region_mismatch` | Explicit profile region differs from its ID prefix |
| 400 | `invalid_request` | Invalid field, duplicate path/query input, body or range |
| 401 | `unauthorized` | Gateway bearer key missing or invalid |
| 503 | `region_unconfigured` | Recognized target region has no configured backend |

Other upstream/cache/error behavior matches [HTTP API](http-api.md). POST/HEAD do
not execute queries. Public mode also rejects regional recommendations. Authentication
is checked before revealing region availability. Logs use route names, not ID paths.
Authenticated `/v1/status` includes per-region `ready` and `session` diagnostics;
`/readyz` continues to describe the default backend, so a disabled/expired secondary
account does not make the default backend unavailable.

The old `/v1/profile?playerProfileId=...` deliberately keeps its default-region
semantics and int64 validation. Use the path form for automatic selection. The
new route does not add arbitrary-player private data: it returns public profile
data under the same response policy as before.
