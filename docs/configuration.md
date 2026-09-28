# Single-File Configuration

Since `0.1.0-alpha.4`, all manually entered server, region, device and SDK settings
can live in `config.toml`. Older images require the legacy file-based settings.
Account passwords remain in `/accounts/*.json`; session snapshots remain in a
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
| `[accounts]` | Account directory, optional selected file, role creation and SDK readiness. |
| `[login]` | Persistent `state_dir`, writable by the service user. |
| `[login.context]` | Observed Android device model, OS, identifier and channel numbers. |
| `[login.sdk_http]` | SDK HTTPS base/allowlist, AppKey, country and optional SDK header. |
| `[login.sdk_http.common]` | SDK common request fields; values are strings. |
| `[recovery]` | Opt-in game-session recovery using saved SDK authorization. |
| `[version_sync]` | Anonymous master/resource version discovery; enabled by default, every 60 seconds. |

## Region and HTTP Paths

The region is configured in `[session]`, not in the HTTP URL. One instance uses
one region and its matching approved `origin`/saved session. Profile requests use
`/v1/profile?playerProfileId=12345678901`; paths such as
`/v1/tw/profile/12345678901` are not supported. Changing a region label alone does
not select another upstream or migrate credentials. Use separate instances and
region-bound state for different regions.

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
pair. Required client upgrades, rejected tokens and device conflicts still require
operator action. No failed business request is automatically replayed. Readiness
is re-established by a successful authenticated query, not by Version success.
See authenticated `/v1/status` → `session.version_sync` for effective versions,
check/update counts, last check time and safe error category. Disable this section
with `enabled=false` for manually pinned or completely offline deployments.

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
