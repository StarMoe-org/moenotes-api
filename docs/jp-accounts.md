# Japanese accounts and credential import

Account files share a private root, split by release:

```text
accounts/
  international/worker.json   # {"user":"EMAIL","password":"PASSWORD"}
  jp/worker.json              # origin-bound game credentials below
```

Create it with `moenotes-server init-accounts ./accounts`. Directories must be
0700, files 0600. Mount the root at `/accounts`, read-only for the HTTP server.
Existing explicit paths such as `directory="/accounts"` remain compatible.
When migrating, move international password files into `international/` and
update that setting; the application never moves/deletes existing files itself.

JP credential file (synthetic values):

```json
{
  "region": "jp",
  "origin": "https://api.bang-dream-on.jp",
  "credentials": {
    "player_id": "AUTHORIZED_INTERNAL_PLAYER_ID",
    "credential": "AUTHORIZED_GAME_CREDENTIAL",
    "device_id": null,
    "bid": null
  }
}
```

The internal player_id differs from the public numeric profile ID. Register may
omit device_id; null is valid. JP rejects BID and omits international
`x-player-bid` / `x-resource-version` headers. File region/origin must match the
JP session; private-file checks reject symbolic links and public permissions.

Add an independent JP backend to the existing private config:

```toml
[regions.jp.accounts]
directory = "/accounts/jp"
selected = "worker.json"

[regions.jp.session]
region = "jp"
origin = "https://api.bang-dream-on.jp"
allowed_origins = ["https://api.bang-dream-on.jp"]
platform = "android"
client_version = "1.0.3"
```

To distribute requests across multiple JP accounts, replace `selected` with
`strategy = "round_robin"`. Each JSON gets its own client, session and cache;
see [session pools](session-pool.md). The HTTP service still never creates JP
accounts. Use a single-account configuration for `jp-check` operator validation.

For a JP-only config use `[accounts]` / `[session]`, plus normal HTTP key/listen
settings. JP has no SDK `[login]` section and rejects automatic recovery.
The first authenticated query triggers local import and returns the existing
authentication_required response without replay; the next query uses the saved
session. Health/status/anonymous calls never register or load credentials.
SIGHUP clears the session and rearms import. Account replacement requires reload
or restart, which invalidates the old generation/cache.

Use `/v1/jp/...`, including `/v1/jp/profile/{profileId}`. JP accepts a positive
int64 on its explicit route; it does not infer JP from an ID prefix. International
automatic profile routing retains 2/3/4. Bulk profiles call
`app.player.PlayerService/GetPlayerList` with the same public JSON projection.

JP Version uses its own descriptor pool. Master is the full release/hash path;
resource version comes from selected `x-asset-version` JSON and may be absent.
CDN credentials are not retained or exposed by the API transport.

## Explicit registration and recovery

Use a separate JP-only config with an **empty private account directory**, mounted
writable for this operator command:

```bash
moenotes-server jp-register jp-config.toml --allow-create --name worker.json
```

It queries Version, writes `registration.attempt`, sends one Register, saves the
private raw `worker.response`, then writes normalized `worker.json`. No automatic
retry, overwrite, SDK login, transfer-password change or existing-account transfer.
Retain the attempt marker after a timeout; do not delete it to register again.

If the response was saved but normalization failed, recover without networking:

```bash
moenotes-server jp-import-registration jp-config.toml --name worker.json
```

The response remains sensitive account material. Its extension excludes it from
JSON account selection; keep it and the marker private.

Verify a saved/imported session with:

```bash
moenotes-server jp-check jp-config.toml
```

This refreshes Version, matches Whoami against the imported ID, loads self-data,
and queries bulk profiles using its account ID. Output contains success indicators
and public master version only. It does not register, initialize tutorial state,
or send a device override.

Credential import never sets `x-override-device-id`. An explicit successful
Register session retains the native one-shot override behavior in memory;
only its successful authenticated call clears the flag. Account transfer and
device takeover are not implemented as automatic recovery operations.
