# Independent sessions and round robin

Enable an account pool within a region:

```toml
[regions.jp.accounts]
directory = "/accounts/jp"
strategy = "round_robin"
```

Use `[accounts]` for the default region. Omit `selected` in pool mode. The
default `strategy = "single"` preserves the existing selection rules: one JSON
file is selected automatically, or `selected` names one file. Combining
`round_robin` with `selected` fails configuration validation.

The directory must contain distinct authorized accounts. On startup the server
lists filenames in sorted order and creates an independent Client, ManagedClient,
session generation, transport, serial queue, rate limit, version poller and cache
for each file. It does not read credentials or log in during discovery,
`check-config`, health or status checks. Credential contents and permissions are
validated by the existing lazy loader on a protected query. Malformed account
files affect their own slots. Never duplicate one account under several filenames;
filename discovery does not verify that the upstream identities are distinct.

Pools support up to 32 files per region. Directory scanning retains the 128-entry
limit, including non-JSON entries. A missing/empty directory produces an empty
pool: health stays live, readiness is false, and queries return 503. Add/remove
account files and restart to change membership. SIGHUP reloads the fixed members
independently, retaining the old session if a member is busy recovering or syncing
versions; other members can reload successfully. It does not register accounts,
transfer devices, or force SDK login. JP imports only saved game credentials.
International accounts retain their SDK readiness, role creation and bounded
recovery settings; use `allow_create = false` to require existing roles.

Each valid HTTP query advances the shared regional cursor to the next available
session, including queries served from cache. Legacy/default and explicit-region
routes share that cursor. Different regions never share accounts or cursors.
Preflight-rejected sessions are skipped; their lazy loader/recovery may start
under the existing rules. Initial loading can return 503; there is no automatic
replay of the triggering query. Authentication/version/device/persistence blocks
exclude that session from protected queries until its lifecycle resolves the block.
Anonymous calls follow the existing anonymous-method policy.

Once assigned, a query executes only on that session. A failed RPC is returned
without replay on another account. In multi-session pools, transport errors,
timeouts and maintenance responses cool down that member for five seconds;
generation replacement clears its cooldown. A later query can select it again.
If all sessions are unavailable, the caller receives the first preflight error
in rotation order. Queue-full errors retain their existing response; queues do
not automatically spill into other sessions.

The single-client defaults (one active RPC, 1000ms minimum interval, 32 queued
requests) apply per account. The HTTP gateway's 64-request admission limit remains
global. Cache capacity and memory bounds apply per session, so total potential
resource use grows with the configured number of accounts. This controls local
concurrency; it does not establish the game server's permitted aggregate rate.

Responses and in-flight coalescing are cached per session and generation, before
the HTTP public projection. No raw or account-dependent fields cross session
caches. Reloading one session invalidates only its old work/cache. Rotating raw
responses can reflect different operator accounts; pool mode is opt-in and does
not imply that account-dependent results are identical. Public field filtering
remains unchanged.

`/v1/status` includes `pool` for the default region and `regions.<region>.pool`:

- `sessions` and `ready_sessions`;
- `members[].slot` (zero-based, sorted filename order; no filename or identity);
- `requests` (assigned HTTP queries, including cache hits), `errors`,
  `cooling_down` and the existing sanitized per-session lifecycle status.

With multiple sessions, the legacy `session` field is null; use the per-member
status. A region is ready when at least one member has verified readiness and is
not cooling down. `/readyz` continues to represent the default region. Status
inspection does not advance round robin or initialize accounts. `auth-status`
also reports pool size without upstream checks.

Offline tests cover concurrent fairness, credential/queue isolation across ten
real Client objects with a synthetic transport, per-session caching, stale
in-flight rejection, cooldown, partial failure, lazy import, reload, configuration
bounds, regional routes and public projection. Live throughput is a separate
operational measurement.
