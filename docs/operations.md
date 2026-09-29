# Authentication and Operations

JP credential import and explicit operator commands are documented separately in
[JP accounts](jp-accounts.md). SDK login/recovery below applies to international
sessions. The common mount separates `international/` passwords and `jp/` game
credentials; JP does not use OneSDK or automatic registration.

Use a dedicated account: login and recovery may invalidate another device's session.
There is no HTTP login endpoint. Passwords never enter command-line arguments,
environment variables, logs or saved session state. The optional
[account directory](accounts.md) reads operator-provided plaintext password files.

## Configuration

Use `moenotes-server config-path` to find the editable configuration and
`init-config` to create a private template. New container deployments use the
persistent `/var/lib/moenotes/config.toml`; legacy mounts still work. See
[configuration and automatic data versions](configuration.md).

Since `0.1.0-alpha.4`, [single-file configuration](configuration.md) consolidates
the HTTP key, device context and SDK parameters into `config.toml`. The file-based
examples below remain compatible. Account passwords still belong in `/accounts`.

### Missing Configuration

`serve` starts in **health-only mode** when the config file, required fields or
explicitly referenced files are missing. It creates no game/SDK client and performs
no authentication, recovery or upstream self-check. `/health` and `/healthz` both
return HTTP 200 with `{"status":"ok"}`. `/readyz` returns 503 with
`{"ready":false,"configured":false}`; all other URLs return 503 with the generic
error `unconfigured`, without exposing configuration details or business data.

A structured `configuration_missing` startup warning lists missing field names,
not values. After template creation, these include `api_key`, session fields and
the template's login/device/SDK fields. If creation is unavailable, the warning
also reports `config_file`; `configuration_path` reports the creation error.
Blank fields/allowlists and empty
API-key files count as missing. Optional settings are not reported unless they are
explicitly configured or needed by an enabled login section.

The image uses `0.0.0.0:8080` for this mode so container health probes can reach it.
Without an explicit setting or generated template, native binaries default to
`127.0.0.1:8080`. `MOENOTES_BOOTSTRAP_LISTEN` takes precedence in health-only mode,
including over a partial config's `listen`. Without that variable, `listen` takes
precedence over the native fallback. The generated template uses `0.0.0.0:8080`. Complete
configs retain their normal listen settings, independent of the bootstrap address.
There is no image `HEALTHCHECK` or game-availability probe; configure the deployment
platform's liveness probe to `/health` or `/healthz`, not `/readyz`.

Supply the missing configuration and **restart** to activate business routes.
SIGHUP in health-only mode only logs `restart_required`. `check-config`, login and
auth commands remain strict and exit nonzero when required configuration is absent.
Malformed TOML, invalid field types and unsafe/invalid nonempty API-key files are
errors, not a reason to weaken authentication. An absent optional game credential
source is still a valid anonymous configuration, not a missing-config error.

For file-based deployment, use the example below. Choose one credential source: `credentials_file`
for static game credentials, or `[login]` for managed state. Paths are relative to
the config file. Managed example:

```toml
listen = "127.0.0.1:8080"
api_key_file = "secrets/http-api-key"
response_mode = "public"
access_log = true

[login]
context_file = "secrets/device-context.json"
sdk_http_file = "secrets/sdk-http.json"
state_dir = "secrets/state"

[recovery]
enabled = true
cooldown_seconds = 300

[session]
region = "operator-selected-region"
origin = "https://game.example.invalid"
allowed_origins = ["https://game.example.invalid"]
platform = "android"
client_version = "operator-client-version"
# Supply master_version/resource_version together when required.
```

Create `secrets/state` with mode 0700 and secret files with mode 0600. Protect their
parent paths. Login/persistence is Unix-only; use WSL/Linux on Windows. State is
plaintext, not an encrypted vault. In containers the writable state directory must
belong to UID 65532; do not loosen permissions to work around ownership.

`device-context.json` contains observed, not invented, device/channel parameters:

```json
{
  "device_model": "OPERATOR_DEVICE_MODEL",
  "operating_system": "OPERATOR_OS_STRING",
  "device_identifier": "OPERATOR_UNITY_DEVICE_IDENTIFIER",
  "global_channel_id": 1,
  "brand_id": 1,
  "area_id": 1
}
```

`sdk-http.json` uses this structure; all values below are placeholders:

```json
{
  "base_url": "https://sdk.example.invalid",
  "allowed_base_urls": ["https://sdk.example.invalid"],
  "app_key": "OPERATOR_APP_KEY",
  "country_id": 1,
  "one_sdk_version": null,
  "common": {
    "game_id": "OPERATOR_GAME_ID",
    "server_id": "OPERATOR_SERVER_ID",
    "merchant_id": "OPERATOR_MERCHANT_ID",
    "app_ver": "OPERATOR_APP_VERSION",
    "sdk_ver": "OPERATOR_SDK_VERSION",
    "channel_id": "OPERATOR_CHANNEL_ID",
    "platform": "OPERATOR_SDK_PLATFORM",
    "platform_type": "OPERATOR_PLATFORM_TYPE",
    "net": "OPERATOR_NETWORK_TYPE",
    "operators": "OPERATOR_NETWORK_OPERATOR",
    "model": "OPERATOR_MODEL",
    "pf_ver": "OPERATOR_OS_VERSION",
    "udid": "OPERATOR_SDK_UDID",
    "dp": "OPERATOR_DISPLAY_SIZE",
    "adid": null,
    "lang": "OPERATOR_LANGUAGE",
    "sdk_log_type": "OPERATOR_LOG_TYPE",
    "ad_ext": "",
    "time_zone": "OPERATOR_TIME_ZONE",
    "isRoot": "OPERATOR_ROOT_STATUS"
  }
}
```

All twenty common keys are explicit; null means SDK omission. The program does not
extract APK/device configuration or ship an AppKey. See [SDK HTTP](sdk-http.md).

## Login

This section describes manual CLI login without `[accounts]`. To load private
`/accounts/international/*.json` files lazily, see [Lazy Account Directory](accounts.md). That
mode defaults to automatic missing-role creation, subject to pre-login, whereas
manual CLI login still requires `--allow-create`.

```sh
moenotes-server check-config config.toml
moenotes-server sdk-login config.toml
# Complete required SDK consent/checks before confirming readiness.
moenotes-server game-login config.toml --confirm-sdk-ready
moenotes-server auth-status config.toml
moenotes-server serve config.toml
```

`sdk-login` prompts without echo, then sends one RSA and at most one email/password
request. `--password-stdin` reads bounded JSON with `email` and `password` from a
pipe, not a terminal. Never put literal passwords in shell history. Rejections
print only SDK codes and challenge/restore flags. CAPTCHA, OTP, account recovery
and consent must be handled by the operator using the official client as needed.

SDK success saves a **pending** authorization, not a game session. The flag
`--confirm-sdk-ready` asserts required SDK steps are complete. Game login checks
role existence and refuses creation without `--allow-create`. Pre-login is not a
transactional no-creation guarantee. No forced-device header is sent.

Success saves versioned SDK/game snapshots and atomically publishes `current.json`.
`operator.lock` serializes CLI/recovery operations sharing a state directory. Old
snapshots are retained; manually prune only when not in use. Protect backups as
secrets. No password is stored. Failures never automatically repeat a login that
may already have been processed upstream.

## Recovery

Recovery requires `recovery.enabled=true`. A query with an explicit token-rejection
category triggers one background attempt for that session generation. Other queries
fail while recovering; **the original query is not replayed**. The worker uses the
approved SDK snapshot, checks the same account/region/origin and existing role, does
one game login, persists its result, then permits new queries. It never sends an SDK
password/cache login, creates a role, forces a device override, changes region or
refreshes master versions automatically.

The total worker deadline is 120 seconds. Cooldown is 60-86400 seconds across
generations; failure exhausts that generation's attempt without a periodic retry
loop. An explicitly client-version-rejected worker may be retried by the next
protected request after opt-in client release following adopts a new version;
the cooldown still applies. SDK expiry, network uncertainty or an absent role
requires operator action.
Persistence failure blocks queries even if upstream login succeeded. Session
rotation invalidates cached results, and locally rejected credentials cannot serve
old authenticated cache entries.

If SDK authorization expires: run `sdk-login`, complete required steps, run
`game-login --confirm-sdk-ready`, then send SIGHUP to the server. SIGHUP reloads
the saved session locally, never logs in, preserves the HTTP key and changes cache
generation. Reload is rejected while recovery runs. Static credential files can
also be replaced by the operator and reloaded this way. `auth-status` is offline;
credential presence does not establish upstream validity.

The preceding manual CLI instructions do not apply to `[accounts]`: SIGHUP in
that mode unloads the session and rearms the first protected query, without
logging in on the signal. See [accounts recovery](accounts.md#reload-and-recovery).

## Diagnostics

- `GET /health` and `GET /healthz`: unauthenticated liveness only; neither queries upstream.
- `GET /readyz`: authenticated 200 after observed authenticated success or completed
  recovery, absent local blocks; otherwise 503. Not a periodic health/expiry probe.
- `GET /v1/status`: authenticated version, response mode, phase, recovery counts,
  safe error category, uptime and HTTP counters; no tokens, player IDs or raw errors.
  In alpha.6, `regions.tw|en|kr` includes each configured backend's readiness and
  session/version-sync state; the original top-level session and `/readyz` still
  describe the default backend. SIGHUP reloads region state independently.
- `X-Request-Id`: random server correlation ID on every response. JSON stderr logs
  contain only this ID, an allowlisted route name, status and duration. No URL/query,
  headers or bodies. Set `access_log=false` to disable per-request logs.

Phases: unverified, ready, recovering, recovered, reauthentication_required,
version_blocked, device_conflict, persistence_failed. The independent version
poller clears an explicit master mismatch only after discovering a changed pair.
Device blocks still require operator action, and so do client upgrades unless
`[version_sync] follow_client_updates` adopts a patch release (see
[configuration](configuration.md#following-client-releases)). `session.version_sync`
reports checks, effective versions and failures; Version success does not establish
authenticated readiness. Whoami is not used as the sole validity probe.
SIGTERM/Ctrl-C cancels queries and recovery. Cancellation cannot undo a sent login.
