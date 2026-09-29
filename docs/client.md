# Client and Session Model

## Protocol Boundary

The checked-in descriptor is the source of truth for wire field numbers, types,
presence and RPC paths. `moenotes-proto::generated` contains prost message types.
`Query` enumerates the 15 HTTP-eligible operations and three support operations;
its variants accept the corresponding generated request type. The client exposes
`Client::query` and cancellable `QueryClient::execute`, not gameplay write APIs.
Explicit authentication methods are separate; see [SDK login](sdk-login.md).
The independent [SDK HTTP module](sdk-http.md) has its own configuration, errors
and admission limit, and never installs a game session from HTTP success alone.

Responses retain `prost_reflect::DynamicMessage`, including unknown wire fields.
`moenotes_proto::to_json` follows protobuf JSON (lowerCamelCase names, decimal
strings for int64/uint64, names for known enums and numbers for unknown enums).
Absent scalar defaults are omitted. Unknown wire fields have no known JSON name
and are not represented in JSON. Null nested messages differ from present empty
messages. No master-data enrichment or client UI transformations are applied.

Request validation includes local defensive limits. These are NOT inferred server
limits: positive IDs/ranks, at most 100 ranks/account IDs/pickup selections, at most
100 rows per arena request, player/name strings at most 256 UTF-8 bytes. Circle
search requires the options message (an empty options object is allowed).
Deprecated probability product_id must remain zero. Pickup order and duplicates
are preserved. Arena band presence is preserved, including explicitly supplied 0;
the library does not silently reproduce UI normalization/filtering.

## Credentials and Origins

`StaticCredentials` implements the synchronous `CredentialProvider` configuration
boundary. It is consulted only at construction or replacement, never automatically
for every query. Separately, `login_with_sdk` exchanges an authorized Android SDK
callback for game credentials and installs a new generation. It is never invoked
by a query, transport reconnect, HTTP request or authentication error.

An operator-supplied credential file has the following structure (values below
are synthetic placeholders, not usable credentials):

```json
{
  "region": "operator-selected-region",
  "origin": "https://game.example.invalid",
  "credentials": {
    "player_id": "OPERATOR_PLAYER_ID",
    "credential": "OPERATOR_GAME_CREDENTIAL",
    "device_id": null,
    "bid": null
  }
}
```

Only HTTPS root origins without userinfo, query, fragment or custom path are
accepted. The configured target must be in `allowed_origins`. Static credentials
must match that origin and region. Discovery results are data, never authorization
to forward existing credentials to a newly discovered host. Production transport
validates TLS with webpki roots, does not follow redirects and never downgrades TLS.

Credential files are explicitly supplied, read-only and limited to 64 KiB. Unix
group/other permissions are rejected. Credentials have redacted Debug output and
zeroize their owned fields on drop; this is best-effort memory hygiene, not a claim
that every HTTP/2/library allocation is wiped. No session is automatically saved.
The managed server layer can explicitly opt into recovery/persistence; that policy
is separate from this low-level client. See [operations](operations.md).
An operator can explicitly call `Client::save_session(generation, path)` to create
a private Unix snapshot for later static loading; see [persistence](sdk-login.md#explicit-persistence).

Every request has a new UUID request ID. There is no automatic retry or forced
device override. Authenticated calls add player credentials and optional device/BID
fields; version headers are added only when the configured pair is present.
Anonymous methods deliberately omit all player credentials. The native game may
attach an existing identity even to anonymous methods; this is a conservative
client policy, not a proven server requirement. Explicit login/pre-login adds the
SDK uid as BID, but never inherits player/device credentials. Pre-login omits the
client-version header, matching the native direct probe. No versions are fabricated.

## Session State and Scheduling

Each session has a random generation ID, immutable config and credentials, a
transport, and a cancellation token. `replace_session` builds and validates a new
session before replacing the old one. Old queued/inflight requests are cancelled,
and their results cannot be labeled or cached with the new generation.

`session_status()` reports local observations: anonymous, credentials unverified,
authentication rejected, version blocked or device conflict. Credential presence
is never treated as proof of validity. Authentication/version/device errors block
subsequent authenticated upstream work for that generation; device blocks require
explicit session replacement. `refresh_versions`
performs one anonymous Version call and atomically installs a validated pair,
retaining credentials and invalidating the old generation. It clears only an
explicit master mismatch when versions change. `adopt_client_version` presents an
explicit candidate with the same anonymous RPC; on success it installs the client
and data versions together and can clear a client-upgrade block. Authentication
and device blocks remain. Server-side automatic adoption is opt-in; see
[following client releases](configuration.md#following-client-releases). Explicit
SDK login can recover an authentication rejection, but cannot bypass a
version/device block.
Anonymous support remains callable.
Maintenance is returned as an error without an automatic retry or login loop.

Requests are serialized per client, with a default minimum one-second start
interval and 32 waiting slots plus the executing slot. The default 10-second
operation deadline includes queue/rate waiting, connection and response reading;
the gRPC request also carries a deadline. A queue can therefore time out before it
is drained. These controls bound load, not claim compatibility with official limits.
Cancellation drops local work; it cannot prove the server did not receive a request.
Construction rejects timeouts above 120 seconds, minimum intervals above 60 seconds,
and more than 4,096 waiting slots, including through the direct Rust API.

## Errors

`ClientError` preserves a typed category and optional gRPC code. Error text and
Debug never include upstream status text, full metadata, SDK values or credentials.
Business codes are retained separately from initial and trailing metadata through
an explicit `business_codes()` diagnostic accessor. Treat those values as untrusted
and potentially sensitive; they are not emitted in HTTP errors or logs.

Duplicate business codes are retained in arrival order. The last trailing code
takes precedence, otherwise the last initial code. Unknown business codes remain
`Business`. `UNDER_MAINTENANCE` is not mistaken for a generic UNKNOWN transport
failure. Transport cancellation is not automatically equated to caller cancellation.
The HTTP gateway returns only the classified category, never the raw gRPC message.

The transport consumes unary results as an explicitly bounded stream to capture
headers and trailers; zero or multiple response messages are protocol failures.
Maximum response size is 8 MiB and maximum request size is 1 MiB. `with_transport`
and the transport trait support controlled mocks; supplying a custom transport
transfers responsibility for TLS and network policy to the caller.

## Not Implemented

Complete SDK authorization workflows, SDK refresh, automatic credential validation,
daily-reset UI, notifications and local
game state, cross-origin failover, JP support, and gameplay writes. A successful
offline test or `check-config` does not establish live service availability.
