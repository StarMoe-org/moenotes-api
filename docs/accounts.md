# Lazy Account Directory

The shared account root now separates `international/` SDK password inputs and
`jp/` game credential imports. New international defaults use
`/accounts/international`; explicit legacy directory settings still work.
See [JP accounts and migration](jp-accounts.md). The SDK creation/recovery behavior
below applies only to international sessions; JP never registers automatically.

Available since `0.1.0-alpha.3`, `AccountDirectoryClient` adds lazy account loading
to the server. Older images do not include it. Use a dedicated, authorized
account: an authenticated HTTP query can now trigger login and, by default,
create a missing regional role. No new HTTP login endpoint is exposed.

## Configuration

Since `0.1.0-alpha.4`, place device/SDK settings and the HTTP key directly in
`config.toml`; see [single-file configuration](configuration.md). Account passwords
still stay exclusively in `/accounts`. The following file-source form remains
supported, including in older alpha.3 images.

Add these sections to a complete server configuration. The device context, SDK
HTTP configuration and state directory are described in [operations](operations.md).
The project does not distribute service AppKeys or device identifiers.

```toml
[login]
context_file = "secrets/device-context.json"
sdk_http_file = "secrets/sdk-http.json"
state_dir = "secrets/state"

[accounts]
directory = "/accounts/international"
# selected = "worker.json"
allow_create = true
sdk_ready = true
```

Each account is one UTF-8 JSON file, with exactly these fields:

```json
{"user":"worker@example.invalid","password":"OPERATOR_PASSWORD"}
```

`user` is the SDK email login, not a game nickname, player ID or SDK UID. These are
placeholders, not usable credentials. Create files with a private editor or secret
manager; do not place real passwords in shell arguments/history, images or Git.
The loader does not write or modify account files. It zeroizes owned password
buffers on drop on a best-effort basis.

- Enable directory login explicitly with `[accounts]`; its default path is
  `/accounts/international`. Without this section, existing static/managed login is unchanged.
- Each regional backend uses **one selected account by default**.
  A process can host additional [regional backends](configuration.md#multiple-regions).
  One
  visible `.json` file is selected automatically. Multiple files require `selected`
  to name one basename. To use every account through independent clients and
  round robin, set `strategy = "round_robin"` and omit `selected`; see
  [session pools](session-pool.md) for limits, lifecycle and cache isolation.
- Account files must be regular, non-symlink files, at most 64 KiB, mode 0600 or
  stricter. Unknown/duplicate JSON fields, empty values and unsafe names fail closed.
  Username/password limits are 1024/4096 UTF-8 bytes. Directory mode is 0700 or
  stricter. Protect parent paths too; login/persistence requires Unix.
- Auto-selection scans at most 128 directory entries, including non-JSON files.
  Explicit selection reads only the selected file. A missing directory/file or
  empty directory leaves authentication unloaded; it does not stop the process.
- `allow_create` defaults to **true**. Set it to false to reject missing roles.
  `sdk_ready` defaults to false; true asserts that required SDK consent and
  post-login checks have been completed. This does not bypass challenges, OTP,
  consent, account recovery or other official-client requirements.

Mount `/accounts` read-only, with owner-only access for container UID/GID 65532.
Mount `login.state_dir` read-write with the same owner and mode 0700. In addition
to the configuration mount, a source-built container can use:

```sh
--mount type=bind,src=/absolute/accounts,dst=/accounts,readonly \
--mount type=bind,src=/absolute/state,dst=/var/lib/moenotes
```

Set `login.state_dir = "/var/lib/moenotes"` in that deployment. Sessions for
different region/origin/account combinations get separate `account-<digest>`
subdirectories. This digest is a namespace, not encryption. Root-level manual
CLI snapshots are not silently imported into an account directory's state.

## Lifecycle

1. Startup, `check-config`, `auth-status`, `/health`, `/healthz`, `/readyz`,
   `/v1/status` and anonymous queries do not read account passwords or log in.
   `check-config` validates server/login settings, not account-file contents.
   Health always returns 200 for a running process; readiness remains 503 until
   an authenticated query succeeds or game-session recovery completes.
2. The first valid, bearer-authorized query requiring game authentication starts
   one background load. The triggering request returns 503, as do protected
   queries while loading; it is **not replayed**. Retry after loading completes.
3. A saved session in the selected scope is loaded locally, without SDK calls,
   pre-login or role creation. This does not prove the saved token is still valid.
4. Without saved state, after `sdk_ready=true`, send one SDK RSA request and at
   most one password request. Persist the returned SDK authorization privately,
   then call `PlayerPreLogin` for the configured region.
5. If a role exists, log in normally. If none exists, proceed only when
   `allow_create=true`; `PlayerLogin` performs first-role creation. Errors or
   maintenance responses are never interpreted as missing roles. Pre-login and
   login are separate calls, not a transactional no-creation guarantee.
6. Persist game/SDK snapshots and atomically publish `current.json`. Future
   requests use that session. Cache generations change; stale results cannot
   cross the session boundary. No naming/tutorial/reward/gameplay calls are made.

There is no login on startup, health probing, timer or directory watcher. Adding a
file enables loading on the next protected query. Failed initialization is limited
to one attempt per input revision in a process; unchanged inputs do not loop.
Fixing malformed files or changing credentials permits another attempt. An
initialized/pinned username cannot silently switch when a file is replaced.
Upstream authentication, version or device blocks are not cleared by file changes.
The independent anonymous version poller can clear an explicit master mismatch
when it discovers a changed data-version pair. Opt-in
[client release following](configuration.md#following-client-releases) can also
clear a client-version block. If the initialization/recovery worker was explicitly
rejected with `CLIENT_UPDATE_REQUIRED`, an adopted release permits another attempt
on the next protected request, retaining the recovery cooldown. Other exhausted
attempts need operator action. The poller never reads account files or logs in and
pauses while account initialization/recovery runs.

The initialization deadline is 120 seconds. Cancellation cannot undo a login
already sent upstream. A failure after in-memory session rotation blocks protected
queries until operator reload; an unpersisted login is not treated as ready.

## Reload and Recovery

In accounts mode, **SIGHUP clears the in-memory session and rearms lazy loading**.
The signal itself performs no network call. A subsequent protected query selects
the account again and prefers its saved session. SIGHUP is refused while login or
recovery runs. Configuration changes, including `selected`, need a process restart.
State must be kept on a persistent volume to avoid re-login after container removal.

With `recovery.enabled=true`, rejected game credentials use the selected scope's
approved SDK snapshot for existing-role recovery, retaining the normal cooldown.
This does not automatically resubmit passwords if SDK authorization expires, and
does not create a replacement role. Resolve SDK challenges in the official client.

To deliberately force a fresh SDK/password attempt after SDK expiry, stop the
service, privately back up and remove the selected scope's `current.json` and
`initial-sdk.json`, then restart and issue a protected query. Removing only
`current.json` reuses `initial-sdk.json`; removing only the latter still loads the
saved game session. Never remove an entire shared state root or another account's
scope. A restart or SIGHUP explicitly rearms attempts; do not automate restarts
as a password-retry loop. Earlier snapshots are retained for operator management.

Manual `sdk-login`/`game-login` reject configurations with `[accounts]` to prevent
writing unrelated root-level state. Use a separate configuration without that
section for manual CLI operations. A changed password does not revoke a saved
game/SDK token or force password login by itself.

Structured logs contain safe events (`account_source`, `account_load`,
`account_sdk_login`, `account_initialization`) and error categories, never account
filenames, usernames, passwords or tokens. Status counters include both initial
loads and recovery attempts. `no_account` and `invalid_account_selection` distinguish
missing files from malformed/ambiguous sources. No list of accounts is exposed
over HTTP. See [security](../SECURITY.md) for deployment boundaries.
