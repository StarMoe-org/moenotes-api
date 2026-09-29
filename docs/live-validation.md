# Live Validation Scope

Authorized checks on 2026-09-24 used the Android 1.0.1 protocol and the TW/HK/MO
region. These are bounded samples, not a service-level or cross-region guarantee.
No credentials, account fixtures or device identifiers are distributed here.

## Regional Path Routing (2026-09-28)

An authorized alpha.6 Linux amd64 container loaded previously saved TW/HK/MO,
EN and KR game sessions, without another SDK/game login. Independent Version
checks updated each region's data pair. The automatic profile path returned the
expected profile from all three regions; equivalent explicit-region profile paths
returned cache HIT. Regional circle searches also succeeded, and another TW/HK/MO
profile query returned the requested profile. All three backends became ready.

Missing bearer authentication returned 401 and an explicit profile-region
mismatch returned 400. Recovery attempts remained zero. Other path aliases have
offline request-equivalence, policy, validation and cache-isolation coverage;
these checks do not claim all live ranking/deck/gacha endpoints were exercised.
The prefix rule remains an observed regional allocation pattern, not a guarantee
of future upstream ID assignments.

## Data Version Synchronization (2026-09-28)

An authorized alpha.5 Linux amd64 check reused a saved TW/HK/MO game session.
Starting with stale master/resource settings, the server's anonymous Version poll
installed the current pair and a protected HTTP circle search returned 200;
`/readyz` then returned 200. Subsequent periodic checks succeeded both in an
isolated container and in the deployed instance. Requests without the gateway key
returned 401. Session files were unchanged, with no SDK/password/game login.

This verifies one current upstream/session sample and periodic discovery, not
all business routes or future authentication validity. Offline tests cover master
mismatch recovery, retained authentication/device blocks, malformed responses,
generation isolation and coexistence with account recovery.

## JP Client Release (2026-09-29)

JP client 1.0.4 was released on the App Store on 2026-09-29 around 11:38 UTC,
during a maintenance. Anonymous Version calls observed from a development machine:
presenting 1.0.3 answered `version` (update required), while 1.0.4 and higher
answered `maintenance`; after the maintenance, 1.0.4 was accepted with
`x-asset-version` present. The deployed gateway, still configured with 1.0.3,
reported `version` from its version poll and its JP queries. This is the rollout
behavior `follow_client_updates` relies on: a refused version moves to the next
patch, and maintenance waits. The option itself was verified with offline tests,
not yet by a deployed rollout.

## Authentication and Persistence

- Rust SDK RSA and email/password login returned success after fixing public-key
  Base64 wrapping and integer UID decoding. Cached-key login was not tested live.
- Explicit game login created a role only after the account owner approved it;
  the server returned `is_new_user = 1`. No tutorial, nickname, reward or gameplay
  mutation was performed afterward.
- SDK authorization was saved and imported with the bound-file API. Game-session
  export was loaded in separate processes through `StaticCredentials::from_file`;
  authenticated queries succeeded without another login.
- One observed SDK post-login configuration did not request an agreement prompt.
  Complete consent, CAPTCHA, account recovery and renewal workflows remain outside
  this validation. The integration's device-identifier value was not checked
  byte-for-byte against Unity runtime output.

## Queries

| HTTP query | Result with the new persisted session |
| --- | --- |
| Announcement list | Success, five announcements and seven external entries |
| Announcement detail | Success, nonempty body |
| Public profile | Success for another player and the new account |
| Batch profiles | Success for one observed account ID |
| Favorite status | Success; default zero/false fields omitted from JSON |
| Gacha probability | Success, two probability groups; both lot and prize totals were 100 |
| Song ranking | Success, 100 entries for one music ID |
| Circle recommendations | Success, empty list |
| Circle search | Success, empty list for one empty-options-name request |
| Circle detail | Not sent: no circle ID returned by the sampled queries |
| Event ranking, challenge ranking, event deck | Not sent: current sampled event/challenge tables were empty |
| Arena ranking, deck trend | Not sent: current sampled season table was empty |

Nine of the fifteen HTTP-eligible query types have successful samples in this
batch. These checks invoked the same Rust client used by the gateway, not all
fifteen HTTP handlers against the live upstream. Earlier gateway checks separately
verified authenticated HTTP profile lookup and API-key rejection.

After the GET migration, a separate gateway smoke confirmed `/v1/profile` and
`/v1/announcements` with a saved session, plus cache HIT, API-key rejection and
invalid-method/removed-route behavior. Other GET handlers have offline contract
coverage; their individual live HTTP roundtrips were not repeated.

Additional research calls confirmed `GetPlayerData` and `GetAllActivities`;
the latter returned an empty list. They are not new HTTP routes or `Query` variants.
`Whoami` returned gRPC `PermissionDenied` without a business code, while other
authenticated calls using the same saved session succeeded. Its precise rejection
cause remains unknown; do not use it as the only credential-validity probe.

The sampled public profile `id` and brief `playerId` were decimal profile-ID
strings, while the login credential's player ID was a UUID. Preserve their source
and do not interchange them. Favorite-status samples using the credential UUID
and observed public IDs all returned default zero/false values; this does not
establish which ID domain is required for a nonzero result.

## Remaining Limits

Intermittent transport failures occurred earlier and remain unexplained. Empty
results do not establish feature availability or absence for every account.
Ranking depth, tie rules, high-volume limits, ongoing event queries, public field
filtering and multi-account concurrency still need targeted validation. A successful
snapshot does not imply credentials remain valid after another login.

## Independent JP sessions — 2026-09-29

A temporary localhost server using the account-pool implementation loaded ten
existing authorized JP credential files with `strategy = "round_robin"`.
The first protected request returned 503 and triggered local import; no JP
registration, password write, device override or transfer was performed.

- Twenty profile queries returned 200 with the requested profile identity.
- Every session received exactly two assigned queries: ten initial MISS results,
  followed by ten HIT results from the corresponding per-session caches.
- All ten sessions reported ready, zero query errors, and one successful local
  import each. `/readyz` returned 200.
- SIGHUP rearmed all members. Following the initial 503/import, ten queries
  returned 200/MISS, verifying cache invalidation and reuse of saved credentials.
- Account files were byte-for-byte unchanged. The temporary server shut down
  successfully; the production deployment was not changed.

This validates sequential live rotation, import, caching and reload. It does not
measure live concurrent throughput or service rate limits. Synthetic offline
tests separately verify ten independent Client queues and concurrent fairness.

## Four-region session pools — 2026-09-29

Four additional authorized international SDK accounts were tested against TW,
EN and KR alongside the ten existing JP sessions in one localhost server.
Four SDK password logins succeeded, including literal-plus email addresses.
After explicit role-creation authorization, eleven missing regional roles were
created; one TW role already existed. The approved SDK authorizations were reused
for EN/KR with explicitly scoped snapshots, without another password login.

- TW/EN/KR each retained four distinct game sessions; JP retained ten.
  All twelve international player IDs and game credentials were distinct.
- Forty-four interleaved authenticated queries returned 200: eight favorite-status
  queries per international region and twenty JP profile queries. Every session
  received exactly two queries, first MISS then HIT. All twenty-two sessions were
  ready; region headers and JP profile identities matched their requests.
- After stopping and restarting the process, all twenty-two sessions loaded
  from private disk state. Twenty-two further authenticated queries returned
  200/MISS. The saved international state files were byte-for-byte unchanged;
  no repeat SDK password login or role creation occurred.
- Input account files were unchanged; temporary processes exited successfully.
  The production deployment was not changed. This is a bounded sequential live
  test, not a concurrent throughput or long-term credential-lifetime guarantee.

## JP profile-card image proxy (2026-09-30)

A local build fetched an authorized public JP profile through a saved JP credential
import, then proxied two profile-card pages through anonymous Version/CDN auth.
Both responses were PNG, 1224×688 (37,088 and 94,595 bytes), byte-identical to the
independent authenticated CDN downloads. Repeated API requests were cache hits;
missing page returned 404 and missing HTTP key returned 401. No SDK login,
registration, device transfer or game write occurred.

The companion ranking service was tested end to end against that local API:
both pages matched, ETag revalidation returned 304, HEAD preserved image length,
and a nonexistent third page returned 404/no-store. Temporary listeners were
stopped afterward. This validates the local implementation; it does not claim
that the new image route has been released or deployed to production.
