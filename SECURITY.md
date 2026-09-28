# Security Boundaries

This is an experimental integration with limited live validation, not a production authentication
service or an official API. No stable release or security audit certification is
claimed. Use only accounts and upstream services you are authorized to access.

## Deployment

- Inline configuration makes `config.toml` a secret file: use owner-only
  Unix permissions. The persistent-volume setup creates it with mode 0600;
  a separately provisioned config may instead use a read-only mount. Automatic
  creation needs a writable parent and never overwrites an existing file.
  HTTP keys and SDK AppKeys must not enter
  images, logs or Git. Account passwords remain exclusively in `/accounts`.

- Use default public projection for shared callers; raw mode is trusted-operator-only.
  All HTTP key holders share the configured game account. Public player data remains
  personal data even after caller-specific fields are removed.
- Bind to loopback by default. Remote deployments need TLS termination, request
  timeouts, connection limits, rate limiting and operator-key rotation at a trusted
  reverse proxy. The in-process admission limit is not an internet-facing DoS shield.
- Never commit credentials, SDK callbacks, AppKeys, private configuration, account
  captures or device identifiers. Keep operator secrets outside the source tree or
  in the ignored `secrets/` directory, with owner-only permissions on Unix.
- Do not log protobuf messages, raw SDK failure data or sensitive accessors.
  Redacted wrappers and best-effort zeroization do not wipe every library buffer.
- GET query strings contain identifiers and filters, not credentials. Redact them
  in proxy/access logs and keep bearer keys out of URLs and browser bookmarks.
- Login can affect upstream account/session state. Cancellation is not rollback.
  SDK HTTP success is pending; it does not complete consent or authorize game access.
- Explicit SDK/session snapshots are plaintext secret files, not an encrypted vault.
  Unix writes require a private parent directory and never replace existing files;
  keep backups and their parent paths private as well. Windows ACL-based saving is
  not implemented. The program never writes passwords. Optional `/accounts` files
  are operator-provided plaintext password storage: mount them read-only and protect
  their parent directory. With `[accounts]`, a bearer-authorized query can initiate
  SDK/game login and create a missing regional role by default. Enable only with
  account-owner authorization; set `allow_create=false` to reject missing roles.
  Opt-in recovery persists rotated
  game credentials; use a dedicated account and a private writable state directory.
- Regional path routing selects only configured backends. Each uses its own bound
  game credentials, SDK/session state and cache. Profile prefix inference never
  authorizes cross-region token reuse or triggers a fan-out search. All regions
  share the gateway bearer key and response policy; callers holding that key can
  reach any configured region. Use separate gateway instances for separate callers.

## Dependency Advisory Review

Reviewed 2026-09-23 against RustSec database revision
`6477ec04375b913e13f38d966dc49eba9d178cb8` and the checked-in `Cargo.lock`.
This was a locked-package advisory inventory and manual version/applicability
review, **not a successful `cargo audit` run**. Installing cargo-audit was stopped
after stalled dependency downloads; rerun the standard tool before a public release.

`rsa 0.9.10` has unresolved
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), a private-key
timing side channel. Production code only loads public keys and encrypts passwords.
Private-key generation/decryption exists exclusively in offline synthetic tests.
The reported private-key recovery path is therefore not used by this project's
production code, but the dependency remains flagged and is not declared patched.
Reassess before adding any private-key operation; do not globally suppress the
advisory. Other inventoried non-withdrawn advisories were covered by the locked
versions' patched or unaffected ranges at review time.

Recovered MD5 signing and RSA PKCS#1 v1.5 encryption reproduce a legacy protocol;
they are not recommendations for new cryptographic designs. Successful offline
tests do not establish the upstream service's security or acceptance policy.

## Reporting

Use a private maintainer channel or GitHub private vulnerability reporting when
available. Share a minimal synthetic reproducer, not real account secrets or game
captures. Do not post credential-bearing requests or responses in public issues.
