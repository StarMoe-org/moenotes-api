# JP profile-card images

The additive image route is:

```http
GET /v1/jp/profile/50000000001/card/1
Authorization: Bearer <HTTP_API_KEY>
```

`profileId` is a positive int64 decimal ID. `page` is **1-based**, in the original
`playerProfile.profileCard.thumbnailUrl` array order. Empty entries retain their
indices and return 404. The route accepts no body or query parameters, and only
GET is supported. JP must be selected explicitly, including on a JP-only server.
International profile-card images are not supported by this route.

The response is `image/png`, with `X-Content-Type-Options: nosniff`,
`X-Moenotes-Region: jp`, `X-Moenotes-Cache: HIT|MISS` and the normal request ID.
`Cache-Control: no-store` retains the authenticated gateway's response policy.
The original profile JSON, including its upstream URLs, is unchanged.

Each request first resolves the profile through the configured JP session pool
and its normal profile cache. It then fetches the selected current card image.
Multiple array entries are separate pages, not layers to combine or size variants.
For a small preview, select the first nonempty entry and preserve its original
index. A historical ranking/profile may refer to an older card; this endpoint
resolves the current profile, rather than promising the historical image.

## Operation

The CLI enables this route's downloader when a JP runtime uses
`https://api.bang-dream-on.jp`. It uses that runtime's effective client version,
including accepted automatic client updates. No extra secret configuration is
needed. Library embedders call `RegionClients::enable_profile_images` with their
JP `Arc<Client>` and shutdown token before constructing the router.

CDN URLs currently require Basic authentication. The downloader obtains the CDN
credential from an **anonymous** Version RPC and retains it only in memory for
five minutes. This RPC does not send player credentials or log in. A CDN 401/403
allows one credential refresh and one retry; game/profile queries are never
replayed. Existing lazy account initialization can still return an initial 503,
just as for the profile JSON route.

The downloader accepts only HTTPS URLs on `static.bang-dream-on.jp` in the
requested profile's `/operation/profilecard/{profileId}/` directory. URLs come
from the profile response, never a caller-supplied URL argument. Redirects and
environment HTTP proxies are disabled for CDN downloads. CDN authorization,
upstream error bodies, cookies and redirect locations are never forwarded.

Only bounded PNG responses are served: maximum 8 MiB, PNG signature/IHDR/IEND
checks, nonzero dimensions at most 8192 per axis. Bytes are not re-encoded or
fully decoded. Successful images are cached by the **complete upstream URL** for
five minutes, up to 128 entries and 64 MiB. At most four image downloads run at
once; requests for the same URL share the download. Invalid/error responses are
not cached as images. Credential discovery failures have a five-second backoff.
The entire handler has a 30-second deadline and observes server shutdown.

| Status | Meaning |
|---|---|
| 200 | PNG image |
| 400 | Invalid ID/page, query parameters or body |
| 401 | Missing/invalid gateway bearer key |
| 404 | Missing profile/card/page, empty URL entry, or disabled route |
| 405 | Method other than GET |
| 429 | Local request/download limit |
| 502 | Rejected URL, invalid/oversized image or CDN failure |
| 503 | Unconfigured JP/proxy, unavailable game session or version/maintenance block |
| 504 | Deadline exceeded |

The route is unavailable when `response_mode="disabled"`. Health, status and
configuration checks do not fetch profiles, CDN credentials or images.

## Website integration

Keep the gateway bearer key on the website's server. For example, the companion
ranking service exposes `/api/v1/jp/profile/{profileId}/card/{page}` and calls this
gateway with its private key. A browser can display that website endpoint in an
`img` element. A trusted client using the gateway directly must fetch with its
Authorization header and turn the successful PNG into a Blob URL; an `img src`
cannot attach that header. Do not put bearer/CDN credentials in image URLs or
expose the official CDN Basic credential in profile JSON.

Offline checks cover routing/auth/response policy, page selection, changed URLs,
cache coalescing/expiry, untrusted URLs, MIME/size bounds, redirect refusal,
anonymous gRPC metadata, CDN credential rotation and the single retry budget.
