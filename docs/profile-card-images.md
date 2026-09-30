# Profile-card images

The image route for each explicitly selected region is:

```http
GET /v1/{region}/profile/{profileId}/card/1
Authorization: Bearer <HTTP_API_KEY>
```

`region` is `tw`, `en`, `kr` or `jp`. `profileId` is a positive int64 decimal ID.
International IDs must be 11 digits with the matching prefix (2=TW, 3=EN, 4=KR);
JP IDs are not inferred from a prefix. `page` is **1-based**, in the original
`playerProfile.profileCard.thumbnailUrl` array order. Empty entries retain their
indices and return 404. The route accepts no body or query parameters, and only
GET is supported. The region must be selected explicitly, even on a single-region server.
The existing JP path is unchanged.

The response is `image/png`, with `X-Content-Type-Options: nosniff`,
`X-Moenotes-Region`, `X-Moenotes-Cache: HIT|MISS`, `X-Moenotes-Card-File`
and the normal request ID. The card-file header identifies the image actually
returned, so a downstream cache can detect a changed image before writing bytes
under the file name from an older profile snapshot.
`Cache-Control: no-store` retains the authenticated gateway's response policy.
The original profile JSON, including its upstream URLs, is unchanged.

Each request first resolves the profile through the selected regional session pool
and its normal profile cache. It then fetches the selected current card image.
Multiple array entries are separate pages, not layers to combine or size variants.
For a small preview, select the first nonempty entry and preserve its original
index. A historical ranking/profile may refer to an older card; this endpoint
resolves the current profile, rather than promising the historical image.

## Operation

The CLI enables a separate downloader and bounded cache for each configured
region. JP requires a runtime using `https://api.bang-dream-on.jp` and uses its
effective client version, including accepted automatic client updates.
International CDN downloads do not send gateway, player or JP CDN credentials.
No extra secret configuration is needed. Library embedders call
`RegionClients::enable_profile_images` once per region with its `Arc<Client>`
and shutdown token before constructing the router.

JP CDN URLs require Basic authentication. The downloader obtains the CDN
credential from an **anonymous** Version RPC and retains it only in memory for
five minutes. This RPC does not send player credentials or log in. A CDN 401/403
allows one credential refresh and one retry; game/profile queries are never
replayed. Existing lazy account initialization can still return an initial 503,
just as for the profile JSON route.

The downloader accepts only HTTPS URLs in the requested profile's
`/operation/profilecard/{profileId}/` directory. JP uses exactly
`static.bang-dream-on.jp`; international URLs have a validated `/prod/<release>/`
prefix and a region-approved domain: `gamerfusiontech.com` for TW,
`bilibiligame.net` for EN, either for KR (including their subdomains).
Query strings, fragments, unexpected ports and normalized/escaped paths are rejected. URLs come
from the profile response, never a caller-supplied URL argument. Redirects and
environment HTTP proxies are disabled for CDN downloads. CDN authorization,
upstream error bodies, cookies and redirect locations are never forwarded.

Only bounded PNG responses are served: maximum 8 MiB, PNG signature/IHDR/IEND
checks, nonzero dimensions at most 8192 per axis. Bytes are not re-encoded or
fully decoded. Successful images are cached by the **complete upstream URL** for
five minutes, up to 128 entries and 64 MiB per region. At most four image downloads per region run at
once; requests for the same URL share the download. Invalid/error responses are
not cached as images. Credential discovery failures have a five-second backoff.
The entire handler has a 30-second deadline and observes server shutdown.

| Status | Meaning |
|---|---|
| 200 | PNG image |
| 400 | Invalid ID/page, query parameters or body |
| 401 | Missing/invalid gateway bearer key |
| 404 | Missing card/page, empty URL entry, or disabled route |
| 405 | Method other than GET |
| 429 | Local request/download limit |
| 502 | Upstream profile business error, rejected URL, invalid/oversized image or CDN failure |
| 503 | Unconfigured region/proxy, unavailable game session or version/maintenance block |
| 504 | Deadline exceeded |

The route is unavailable when `response_mode="disabled"`. Health, status and
configuration checks do not fetch profiles, CDN credentials or images.

## Website integration

Keep the gateway bearer key on the website's server. `starmoe-api` enforces
player-page visibility and forwards its zero-based `/cards/{index}` routes to
this gateway with `page = index + 1` for every region. Its image cache validates
`X-Moenotes-Card-File` before persisting bytes, and does not persist responses
from older gateways without that header. The ranking service has no profile-card
route; profile images are outside its responsibility.

A browser can display the website endpoint in an `img` element. A trusted client
using the gateway directly must fetch with its Authorization header and turn the
successful PNG into a Blob URL; an `img src` cannot attach that header. Do not put
bearer/CDN credentials in image URLs or expose the JP CDN Basic credential in JSON.

Upgrade moenotes-api before switching international website image downloads to
the new routes. Remove any ranking image-route consumers before deploying a
ranking version that removes those routes.

Offline checks cover routing/auth/response policy, page selection, changed URLs,
cache coalescing/expiry, untrusted URLs, MIME/size bounds, redirect refusal,
anonymous gRPC metadata, CDN credential rotation and the single retry budget.
