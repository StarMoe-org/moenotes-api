# Live Validation Scope

Authorized checks on 2026-09-24 used the Android 1.0.1 protocol and the TW/HK/MO
region. These are bounded samples, not a service-level or cross-region guarantee.
No credentials, account fixtures or device identifiers are distributed here.

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
