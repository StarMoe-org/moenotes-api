# Single-File Configuration

JP credential-only backends use `[regions.jp.accounts]` and `[regions.jp.session]`
without a login/SDK section. The common account root has `international/` and
`jp/` subdirectories; see [JP configuration](jp-accounts.md).

Since `0.1.0-alpha.4`, all manually entered server, region, device and SDK settings
can live in `config.toml`. Older images require the legacy file-based settings.
Account passwords remain in `/accounts/international/*.json`; session snapshots remain in a
persistent writable state directory. No passwords are accepted in the config.

## Find and Create the File (alpha.5)

Run these commands inside the container or on the native host:

```sh
moenotes-server config-path
moenotes-server init-config
moenotes-server check-config
```

`config-path` prints the resolved absolute path, never its contents. `init-config`
creates a commented template with mode 0600 and private parent directories, and
never overwrites an existing file. `serve` also attempts this on first startup and
logs `configuration_path`, including a permission error if creation is unavailable.

Path selection is: command argument, `MOENOTES_CONFIG`, an existing legacy
`/etc/moenotes/config.toml`, then the container default
`/var/lib/moenotes/config.toml` (native default: `./config.toml`). Set
`MOENOTES_CONFIG=/var/lib/moenotes/config.toml` to explicitly migrate off a legacy
mount. An explicit CLI path takes precedence, so give the same path to diagnostic
commands when the service was started with one.

For Zeabur, attach a persistent volume at `/var/lib/moenotes`, then edit
`config.toml` in that volume using the file manager or container terminal. New
deployments create the template there automatically when the volume is writable
by UID/GID 65532. Keep the same volume mounted across redeployments. Restart after
editing. `/accounts` remains a separate optional private account-password mount.
For a Docker host, see [compose.yaml](../compose.yaml); `./data/config.toml` is the
editable host file. Do not edit a temporary `/etc/moenotes` copy generated from a
Kubernetes Secret: update the Secret source, or migrate to the persistent path.

The generated [template](../config.example.toml) is intentionally blank:
without required values, `serve` exposes `/health` and `/healthz` with HTTP 200
`{"status":"ok"}`, logs missing key names and keeps business routes at 503.
No SDK/game clients, login or upstream checks run in health-only mode. An absent
file or an empty file behaves the same way. Supply the config and restart to
activate queries; SIGHUP does not reread configuration.

## Settings

| Section/key | Purpose |
| --- | --- |
| `listen` | HTTP bind address; use `0.0.0.0:8080` inside a container. |
| `api_key` | Your HTTP bearer key, 32-4096 ASCII characters without whitespace. |
| `response_mode` | Keep `public` for shared callers; `raw` is trusted-operator-only. |
| `[session]` | Region, approved game HTTPS origin, platform and client/data versions. |
| `[accounts]` | Account directory, single/round_robin strategy, optional selected file, role creation and SDK readiness. |
| `[login]` | Persistent `state_dir`, writable by the service user. |
| `[login.context]` | Observed Android device model, OS, identifier and channel numbers. |
| `[login.sdk_http]` | SDK HTTPS base/allowlist, AppKey, country and optional SDK header. |
| `[login.sdk_http.common]` | SDK common request fields; values are strings. |
| `[recovery]` | Opt-in game-session recovery using saved SDK authorization. |
| `[version_sync]` | Anonymous master/resource version discovery; enabled by default, every 60 seconds. Optionally follows patch client releases. |

## Region and HTTP Paths

The default region is configured in `[session]`. `/v1/profile/20000000001`
automatically selects TW/HK/MO by its 11-digit ID prefix (2=tw, 3=en, 4=kr).
`/v1/tw/profile/20000000001` explicitly selects the same region. Other path routes
use the default region unless prefixed with `/v1/tw`, `/v1/en` or `/v1/kr`.
See [path routes](path-routes.md) for examples covering all query operations.
The old `/v1/profile?playerProfileId=...` keeps default-region semantics.

## Multiple Regions

Keep the existing top-level configuration for the default backend. Add only the
other regions to the same private file; do not repeat the default under `[regions]`.
Example for an existing HK default and separate saved EN/KR game credentials:

```toml
[regions.en]
credentials_file = "en/game-session.json"
[regions.en.session]
region = "en"
origin = "https://en.example.invalid"
allowed_origins = ["https://en.example.invalid"]
platform = "android"
client_version = "1.0.1"

[regions.kr]
credentials_file = "kr/game-session.json"
[regions.kr.session]
region = "kr"
origin = "https://kr.example.invalid"
allowed_origins = ["https://kr.example.invalid"]
platform = "android"
client_version = "1.0.1"
```

Replace placeholder origins with approved current regional origins. Each credential
file must already match its session region and origin. Static game credentials
do not enable SDK recovery. A regional `[login]`/`[accounts]` setup may instead be
provided as `[regions.en.login]`, `[regions.en.login.context]`,
`[regions.en.login.sdk_http]`, `[regions.en.accounts]`, `[regions.en.recovery]` and
`[regions.en.version_sync]`, using the same fields as their top-level equivalents.
Never combine `credentials_file` with `login` for the same backend.

Authentication sources are explicit per region and are not inherited. Device/SDK
settings can point to the same authorized source files, and account directories
may share an authorized SDK account, but each login state directory must be a
different private directory. With the default single strategy, the loader initializes only the selected target
region on a protected request; startup does not create roles across all regions.
Create state directories as UID/GID 65532, mode 0700. Paths resolve relative to
the single config file. Duplicate canonical regions, duplicate origins and shared
state directories are rejected. Changing `region` alone cannot migrate a saved token.

Version polling defaults to enabled/60 seconds separately in each configured
region; regional recovery defaults to disabled. Set `[regions.en.version_sync]`
`enabled=false` to disable it there. Request/cache limits inherit the top-level
settings, with separate per-region caches/queues, so total cache memory grows with
the number of configured regions. The HTTP bearer key and response mode are global.

`check-config` and `auth-status` validate all region state without networking or
reading passwords. Missing/invalid secondary credential files fail startup rather
than silently using default-region credentials. SIGHUP reloads each backend locally,
logging success separately; a backend in login/recovery or version update may reject
reload without affecting the others. Config changes still require a restart.
Manual `sdk-login`/`game-login` act on the top-level session; use a separate private
operator config with the target regional settings when provisioning that state.

## Automatic Data Versions

```toml
[version_sync]
enabled = true
interval_seconds = 60
```

For a fully configured `serve`, one anonymous Version RPC runs at startup and
then after each interval (30–86400 seconds). No account or SDK token is sent.
`check-config`, `auth-status`, `init-config`, health-only startup and diagnostic
HTTP routes never initiate checks. SDK/password login is independent and is never
triggered by version polling.

The complete master/resource pair is validated and installed together in memory;
the config file is not rewritten. Static `[session]` version fields are optional
startup fallbacks. Restart discovers current values again. Failed checks retain
the last known pair and retry only on the next interval. A changed pair invalidates
old cached responses and in-flight work, while preserving the account credentials.
Clients may see a transient 503 around a version change and should retry normally.

Only an explicit `MASTER_VERSION_MISMATCH` is cleared by discovery of a changed
pair. Required client upgrades need operator action unless the option below adopts
an advertised or probed release. Rejected tokens and device conflicts are not
cleared by version discovery. No failed business request is automatically replayed. Readiness
is re-established by a successful authenticated query, not by Version success.
See authenticated `/v1/status` → `session.version_sync` for effective versions,
check/update counts, last check time and safe error category. Disable this section
with `enabled=false` for manually pinned or completely offline deployments.

### Following Client Releases

```toml
[version_sync]
follow_client_updates = true
```

After a client release the game refuses the configured `client_version` (the
Version call returns `CLIENT_UPDATE_REQUIRED`, categorized as `version`), and
every query of that region fails until the configuration is updated. With
`follow_client_updates = true` the poller first reads
`https://metadata.bdon.moe/current_version.json` and selects that region's newer
`client_version`. TW/HK/MO (`hk`, `tw`, `hk-tw-mo`) maps to `regions.hk-tw-mo`;
EN, KR and JP map to their respective keys. The candidate is validated with the
same anonymous Version call: no account, SDK token or login is involved.

The metadata request is HTTPS-only, sends no credentials, disables redirects and
proxies, has a five-second timeout and accepts at most 64 KiB of schema-version-1
JSON. A process-wide 60-second cache shares successful and failed fetches across
all regions and pool members. Missing, malformed, equal or older candidates are
ignored. Only `client_version` is used: metadata master/resource values, URLs and
endpoints never replace game-returned versions or configured origins.

If metadata is unavailable or unusable, or its candidate is explicitly rejected
with `CLIENT_UPDATE_REQUIRED`, the poller falls back to the next three patch
releases in order (`1.0.3` → `1.0.4`, `1.0.5`, `1.0.6`). A metadata candidate
already tried is not probed twice in the same check.

- A candidate also rejected with `CLIENT_UPDATE_REQUIRED` moves on to the next one.
  `MASTER_VERSION_MISMATCH` shares the `version` error category but never starts or
  advances this search.
- The first candidate it accepts is installed together with the versions that call
  returned, and the version block is cleared. Queries continue under the new
  version; old cached responses are invalidated as for any version change.
- Any other answer stops the search until the next check. During a rollout the new
  release is typically announced with maintenance while the old one is already
  refused; the poller keeps the old version and retries every interval.
- Metadata can advertise a newer minor or major release (`1.0.x` → `1.1.0`),
  which is adopted only after the game accepts it. Minor and major releases are
  never guessed by the fallback. Versions not in `MAJOR.MINOR.PATCH` format
  still require operator action.

The adopted version is kept in memory only; `config.toml` is not rewritten, so a
restart starts from the configured value and follows again on its first check.
Update `client_version` in the file to make it permanent. `version_sync` in
`/v1/status` shows the presented `client_version` and a `client_updates` count, and
each followed release logs a `client_version_update` event with both versions
and `source` (`metadata` or `patch_probe`).

If an initialization/recovery worker was explicitly rejected with
`CLIENT_UPDATE_REQUIRED`, adopting a release rearms that attempt for the next
protected request. Recovery cooldowns remain in force. Other worker failures stay
exhausted, and the poller itself never logs in or replays the failed query.

The request descriptors stay those of the configured release. Acceptance by Version
does not establish compatibility of other methods: protobuf can decode changed
messages without an error, including unknown fields or changed meanings. Validate
the queries you rely on after a followed update. Each pool member follows
independently. Additional regions require their own opt-in, for example
`[regions.jp.version_sync] follow_client_updates = true`; they do not inherit the
top-level setting.

Use actual authorized device/SDK values, not arbitrary IDs. The repository and
image do not embed a service AppKey or operator device values. SDK common fields
are separate from game session metadata. Keep the existing request interval and
cache defaults unless there is a measured reason to change them.

## SDK Null Values

TOML has no `null`. Put every SDK common parameter either in the `common` table or
in `omit_common`, never both. The union must contain exactly the 20 known keys
listed in the template. Duplicates and unknown keys fail validation.

For example, `omit_common = ["adid"]` sends no `adid` field, matching a JSON
`"adid": null`. Conversely, `ad_ext = ""` is an explicit empty string and is sent
as an empty value. Omitting a key from both locations is incomplete configuration,
not an instruction to omit it upstream. This preserves the legacy SDK encoding.

## Files and Permissions

New deployments keep configuration and session state on one persistent volume:

- `/var/lib/moenotes`, read-write, contains `config.toml` and saved sessions.
- Account directory to `/accounts`, read-only, with `{"user":"EMAIL","password":"PASSWORD"}` files.

An existing deployment can keep a separate read-only config mount at
`/etc/moenotes/config.toml` and its writable state volume. With a read-only config
mount, create and edit the file at its host/Secret source; automatic template
creation requires a writable parent directory.

The container runs as UID/GID 65532. Private files must be mode 0600 or stricter;
account/state directories must be mode 0700 or stricter and owned/accessed by that
user. A config containing inline secrets or device values is also a secret file:
no group/world access or symlink. Keep backups private and exclude it from Git and
images. Empty templates contain no secrets and can be readable while unconfigured.
Malformed TOML, conflicting sources, invalid nonempty keys and unsafe permissions
are errors, not health-only fallbacks. The config is limited to 64 KiB.

`check-config` validates the configuration offline. It does not validate passwords
or live sessions. Health/status never trigger login, and the first protected query
still starts lazy account loading and returns 503 until the caller retries after
completion. See [account lifecycle](accounts.md) for role creation and recovery.

## Compatibility

Existing deployments can keep their file-based settings unchanged, or migrate
each source independently:

| Inline setting | Legacy alternative |
| --- | --- |
| `api_key` | `api_key_file` |
| `[login.context]` | `login.context_file` JSON |
| `[login.sdk_http]` and `.common` | `login.sdk_http_file` JSON |

Providing both sources is rejected, even if the file path is empty. Relative
legacy paths still resolve against the config directory. Static `credentials_file`
and manual operator login remain available without `[accounts]`; do not combine
static credentials with managed login. Saved SDK/game snapshots are unchanged.

For multiple independent accounts in one region, see [session pools](session-pool.md).
