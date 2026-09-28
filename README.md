# moenotes-api

An experimental Rust client and HTTP query gateway for Our Notes, based on
static analysis of the Android `com.bilibili.sirius` 1.0.1 protocol.

**Experimental, with limited authorized live validation.** SDK password login,
game login, session persistence and selected queries have passed live checks;
see [validation scope](docs/live-validation.md). Complete SDK consent/renewal flows,
upstream limits and exhaustive account-visibility validation remain incomplete.
Version `0.1.0-alpha.2` establishes a [versioned HTTP contract](docs/api-stability.md). This is
an independent project, not an official or endorsed API.

## What It Supports

- `moenotes-proto`: reproducible message generation and protobuf JSON reflection
  from a checked-in descriptor snapshot. No `protoc` installation required.
- `moenotes-client`: 15 audited query operations plus server discovery, version
  lookup and `Whoami`; HTTPS gRPC, explicit credential injection, cancellation,
  session isolation, bounded serial scheduling and classified errors. Explicit
  Android SDK-callback import, pre-login and game-login exchange have explicit operator commands.
  Separate SDK HTTP RSA, password and cached-key primitives are available;
  their results remain pending until required SDK post-login checks are completed.
  Explicit private Unix SDK/session snapshots support restart without re-login.
  Opt-in bounded recovery renews rejected game sessions from approved SDK caches;
  expired SDK tokens require operator reauthentication.
- `moenotes-server`: API-key-protected HTTP queries, short-lived bounded memory
  cache, duplicate-request coalescing and OpenAPI documentation. Read routes use
  GET with short `/v1` resource paths and URL query parameters.

Queries cover profiles, favorites, event PT rankings and decks, song rankings,
arena rankings and card trends, circles, gacha probabilities and announcements.
There are no gameplay write operations, arbitrary RPC proxy, complete SDK/social
login workflow, database, historical collector, master-data enrichment or Japanese-release
compatibility claims. Explicit game login can create an account or affect an
existing session; it is not a read-only query and has no HTTP route.

HTTP defaults to a recursive public-field whitelist, omitting `myRank`, `myScore`
and `isSentFavorite`. Personalized recommendations are disabled in public mode.
`response_mode="raw"` retains original responses for trusted operators only.
All query modes require a bearer key. Public mode is not anonymization.

## Install

Release images are published to the public GitHub Container Registry package
`ghcr.io/luoxiadesu/moenotes-api` for Linux `amd64` and `arm64`. No registry login
is required to pull public images:

```sh
docker pull ghcr.io/luoxiadesu/moenotes-api:0.1.0-alpha.4
docker run --rm --network none ghcr.io/luoxiadesu/moenotes-api:0.1.0-alpha.4 --version
```

Use an exact version or the immutable digest listed in the GitHub Release.
There is no `latest` tag. Images contain no configuration or game credentials.
See [Run the HTTP Server](#run-the-http-server) and [Docker](#docker) before deployment.

### Build From Source

Install rustup and build with the pinned Rust 1.98.1 toolchain:

```sh
cargo build --locked --workspace
cargo test --locked --workspace
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Dependency downloads are needed for a first build. Protocol generation and all
tests are offline with respect to the game; gRPC tests use loopback mock servers.

To try HTTP routing without credentials or any upstream connection:

```sh
cargo run --locked -p moenotes-server --example offline -- 8081
```

This loopback-only example uses the fixed key
`offline-demo-key-not-for-production-123456789` and returns empty **synthetic**
responses. It is not a live-data mode and is not included in the Docker image.

## Rust Client

```rust,no_run
use moenotes_client::{Client, ClientOptions, Query, SessionConfig};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let config = SessionConfig {
    region: "operator-selected-region".into(),
    origin: "https://game.example.invalid".into(),
    allowed_origins: vec!["https://game.example.invalid".into()],
    platform: "android".into(),
    client_version: "1.0.1".into(),
    master_version: None,
    resource_version: None,
};
let client = Client::new(config, None, ClientOptions::default())?;
let response = client.query(Query::Announcements(Default::default())).await?;
let json = moenotes_proto::to_json(&response.message)?;
# let _ = json;
# Ok(()) }
```

The host above is intentionally nonfunctional. Configure an explicitly approved
origin and matching region before making authorized live requests. Authentication
is optional only for descriptor-marked anonymous methods, not a maintenance bypass.
For authenticated queries, supply a `StaticCredentials` provider to `Client::new`.
See [client and session behavior](docs/client.md).
For an already authorized OneSDK callback, see [SDK-to-game login](docs/sdk-login.md).
The separate [SDK HTTP primitives](docs/sdk-http.md) return pending login results;
they do not bypass SDK checks or implement complete authorization or renewal.

## Run the HTTP Server

Since `0.1.0-alpha.4`, one `config.toml` holds the HTTP key, game session,
device context and SDK HTTP settings. Account passwords remain in `/accounts`.
The blank [template](config.example.toml) starts health-only until filled in;
see [single-file configuration](docs/configuration.md).

Fill in `config.example.toml` and set `api_key` to a random ASCII key of at least
32 characters. Protect the populated config and account files with mode `0600`,
without group/other permissions. Never put keys on a command line or commit them.
Legacy file sources remain supported; see [operations](docs/operations.md).
Relative file paths resolve against the configuration file.

```sh
cargo run --locked -p moenotes-server -- check-config config.toml
cargo run --locked -p moenotes-server -- serve config.toml
```

`check-config` does not make upstream calls or verify credential validity.
The binary's default bind address is `127.0.0.1:8080`; the container-oriented
template sets `0.0.0.0:8080`. `/healthz` reports process liveness
only. `/openapi.json` requires `Authorization: Bearer <HTTP_API_KEY>`.

Since `0.1.0-alpha.3`, missing configuration starts a
health-only listener instead of exiting. `/health` and `/healthz` return 200 without
upstream checks; logs list missing configuration keys, and business routes remain
unavailable (503). The image's bootstrap listener is `0.0.0.0:8080`. Fill in the
configuration and restart. `check-config` remains strict;
see [operations](docs/operations.md#missing-configuration).

The server also supports optional lazy `/accounts` loading from private
`{"user":"EMAIL","password":"PASSWORD"}` JSON files. The first protected query
loads a persisted session or logs in, creating a regional role only if pre-login
reports none. Health/status never trigger login; missing accounts leave health at
200. One account/region is selected per instance. See [account directory setup](docs/accounts.md)
for permissions, SDK-readiness confirmation, selection and retry behavior.

Query routes accept
GET requests with query parameters and no body. Example request:

```http
GET /v1/event/ranking?eventId=123&ranks=1&ranks=10&ranks=100
Authorization: Bearer <HTTP_API_KEY>
```

For a profile, use `/v1/profile?playerProfileId=12345678901`. Arrays use repeated
parameter names; IDs are decimal text. The response is un-enriched protobuf JSON, with 64-bit
integers represented as decimal strings. Fetch time and cache status are headers.
See [HTTP API](docs/http-api.md) for all routes, limits and error meanings.

Since alpha.2, the old POST `/experimental/v1/...` routes are removed. The upstream game
protocol remains gRPC, not HTTP GET. See [operations](docs/operations.md) for login,
recovery, SIGHUP reload and authenticated `/readyz` and `/v1/status` diagnostics.

## Docker

```sh
docker run --rm --cap-drop ALL --security-opt no-new-privileges \
  -p 127.0.0.1:8080:8080 \
  --mount type=bind,src=/absolute/operator-config,dst=/etc/moenotes,readonly \
  --mount type=bind,src=/absolute/accounts,dst=/accounts,readonly \
  --mount type=bind,src=/absolute/state,dst=/var/lib/moenotes \
  ghcr.io/luoxiadesu/moenotes-api:0.1.0-alpha.4
```

Set the mounted config's `listen` to `0.0.0.0:8080` inside the container. The image
runs as UID/GID 65532; grant that user access to the config and private secret files
without making secrets group/world-readable. Mount only the necessary directory.
The example uses the template's `login.state_dir=/var/lib/moenotes`; initialize
the host state/account directories with mode 0700 and UID 65532 ownership, and
config/account files with mode 0600. Keep configuration and accounts read-only.
Use a TLS reverse proxy before exposing HTTP remotely; TLS termination, firewalling
and operator key rotation are deployment responsibilities. No permissive CORS or
remote credential-management endpoint is included.

To build locally, run `docker build -t moenotes-api:dev .`. The multi-stage build
uses pinned Rust and cargo-chef versions; dependency layers survive application-only
edits. CI exports these layers to architecture-specific GitHub Actions caches and
smoke-tests each native Linux image before publishing. See [releasing](docs/releasing.md).

## Documentation

- [Single-file configuration and legacy compatibility](docs/configuration.md)
- [Client, authentication and error model](docs/client.md)
- [Operator login, recovery and diagnostics](docs/operations.md)
- [Lazy account directory](docs/accounts.md)
- [API stability and migration](docs/api-stability.md)
- [SDK-to-game login and analysis boundaries](docs/sdk-login.md)
- [SDK HTTP login primitives and encoding](docs/sdk-http.md)
- [Experimental HTTP API](docs/http-api.md)
- [Ranking responses and downstream integration](docs/rankings.md)
- [Protocol provenance and third-party notice](proto/NOTICE.md)
- Rust API reference: `cargo doc --locked --workspace --no-deps`
- [Changelog](CHANGELOG.md)
- [Local validation results and remaining gaps](docs/validation.md)
- [Security boundaries and dependency advisory review](SECURITY.md)
- [Build caches, versioning and release procedure](docs/releasing.md)

## License

Original project code is [MIT licensed](LICENSE). Recovered third-party protocol content and
generated definitions are **not** claimed as original MIT work; see
[proto/NOTICE.md](proto/NOTICE.md). Review redistribution requirements before
publishing protocol artifacts. No credentials or real account fixtures are included.
