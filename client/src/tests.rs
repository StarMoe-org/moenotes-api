use super::*;
use crate::transport::{DynamicCodec, mock_channel};
use axum::{
    Router,
    body::Body,
    extract::{Request as HttpRequest, State},
};
use http_body_util::StreamBody;
use prost::Message;
use prost_reflect::ReflectMessage;
use std::sync::atomic::{AtomicUsize, Ordering};
use tonic::{Request, Response, Status, metadata::MetadataMap};

mod auth_tests;
mod version_tests;

fn config() -> SessionConfig {
    SessionConfig {
        region: "test".into(),
        origin: "https://game.invalid".into(),
        allowed_origins: vec!["https://game.invalid".into()],
        platform: "android".into(),
        client_version: "1.0.1".into(),
        master_version: Some("master-test".into()),
        resource_version: Some("resource-test".into()),
    }
}
fn credentials() -> StaticCredentials {
    StaticCredentials::new(
        "test".into(),
        "https://game.invalid".into(),
        Credentials {
            player_id: "synthetic-player".into(),
            credential: "synthetic-private-value".into(),
            device_id: None,
            bid: Some("synthetic-bid".into()),
        },
    )
}
fn options() -> ClientOptions {
    ClientOptions {
        minimum_interval: Duration::ZERO,
        ..ClientOptions::default()
    }
}

fn fixtures() -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::json;
    vec![
        ("announcement", json!({"id":"9007199254740993"})),
        ("announcements", json!({"selectedTab":"ALL"})),
        (
            "arena-ranking",
            json!({"arenaSeasonId":"5","bandId":"0","rankingStart":"1","rankingEnd":"100"}),
        ),
        ("deck-trend", json!({"musicId":"7","arenaSeasonId":"5"})),
        ("circle", json!({"circleId":"18446744073709551615"})),
        ("circle-recommendations", json!({})),
        ("circle-search", json!({"options":{"name":" synthetic "}})),
        ("challenge-ranking", json!({"challengeMusicId":"5"})),
        (
            "event-deck",
            json!({"playerId":"synthetic-player","eventId":"5"}),
        ),
        ("event-ranking", json!({"eventId":"5","ranks":[1,10,1]})),
        ("profile", json!({"playerProfileId":"9007199254740993"})),
        (
            "probability",
            json!({"gachaId":"5","selectedPickUp":["9","1","9"]}),
        ),
        ("music-ranking", json!({"musicId":"7"})),
        ("favorite-status", json!({"playerId":"synthetic-player"})),
        ("profiles", json!({"accountIds":["9007199254740993","1"]})),
        ("server-list", json!({})),
        ("version", json!({})),
        ("whoami", json!({})),
    ]
}

type Calls = Arc<std::sync::Mutex<Vec<(Method, Vec<u8>, MetadataMap)>>>;
#[derive(Clone)]
struct MockUnary {
    method: Method,
    calls: Calls,
}
impl tonic::server::UnaryService<DynamicMessage> for MockUnary {
    type Response = DynamicMessage;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Response<Self::Response>, Status>> + Send>,
    >;
    fn call(&mut self, request: Request<DynamicMessage>) -> Self::Future {
        let method = self.method;
        self.calls.lock().unwrap().push((
            method,
            request.get_ref().encode_to_vec(),
            request.metadata().clone(),
        ));
        Box::pin(async move {
            let desc = pool().get_message_by_name(method.output).unwrap();
            let message = if method.name == "favorite-status" {
                DynamicMessage::deserialize(
                    desc,
                    serde_json::json!({"totalFavorite":"9007199254740993","isSentFavorite":true}),
                )
                .unwrap()
            } else if method == crate::auth::LOGIN {
                DynamicMessage::deserialize(desc, serde_json::json!({
                    "credential": {"id":"synthetic-new-player", "credential":"synthetic-new-secret",
                    "deviceId":"synthetic-device-must-not-forward", "profileId":"9007199254740993"},
                    "isNewUser":7
                })).unwrap()
            } else if method == crate::auth::PRE_LOGIN {
                DynamicMessage::deserialize(desc, serde_json::json!({"isAccountCreated":true}))
                    .unwrap()
            } else {
                DynamicMessage::new(desc)
            };
            Ok(Response::new(message))
        })
    }
}

async fn mock_grpc(State(calls): State<Calls>, request: HttpRequest) -> http::Response<Body> {
    let method = *METHODS
        .iter()
        .chain([&crate::auth::LOGIN, &crate::auth::PRE_LOGIN])
        .find(|m| m.path == request.uri().path())
        .unwrap();
    let codec = DynamicCodec(pool().get_message_by_name(method.input).unwrap());
    tonic::server::Grpc::new(codec)
        .unary(MockUnary { method, calls }, request)
        .await
        .map(Body::new)
}

#[tokio::test]
async fn all_queries_over_local_grpc_and_conditional_headers() {
    let calls: Calls = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let server_stop = stop.clone();
    let router = Router::new().fallback(mock_grpc).with_state(calls.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_stop.cancelled_owned())
            .await
            .unwrap()
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let client = Client::with_transport(
        config(),
        Some(&credentials()),
        options(),
        Arc::new(mock_channel(channel)),
    )
    .unwrap();
    for (name, json) in fixtures() {
        let query = Query::from_json(name, json).unwrap();
        let expected = query.encode();
        let response = client.query(query).await.unwrap();
        let calls = calls.lock().unwrap();
        let (method, bytes, headers) = calls.last().unwrap();
        assert_eq!(bytes, &expected, "{name}");
        assert_eq!(response.message.descriptor().full_name(), method.output);
        assert_eq!(headers.get("x-platform").unwrap(), "android");
        assert!(headers.get("grpc-timeout").is_some());
        assert_eq!(
            headers.get("x-player-credential").is_none(),
            method.anonymous
        );
        assert_eq!(headers.get("x-master-version").is_none(), method.anonymous);
        assert!(headers.get("x-device-id").is_none());
        assert!(
            !headers
                .keys()
                .any(|key| format!("{key:?}").contains("override"))
        );
        if name == "favorite-status" {
            assert_eq!(
                moenotes_proto::to_json(&response.message).unwrap()["totalFavorite"],
                "9007199254740993"
            );
        }
    }
    assert_eq!(calls.lock().unwrap().len(), 18);
    let ids = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, _, m)| m.get("x-request-id").unwrap().to_str().unwrap().to_owned())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 18);
    drop(client);
    stop.cancel();
    server.await.unwrap();
}

#[tokio::test]
async fn initial_and_trailing_errors_are_preserved_but_redacted() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let router = Router::new().fallback(|| async {
        let mut trailers = http::HeaderMap::new();
        trailers.insert("grpc-status", "2".parse().unwrap());
        trailers.insert("grpc-message", "synthetic-private-value".parse().unwrap());
        trailers.append("x-sirius-error-code", "TOKEN_ILLEGAL".parse().unwrap());
        trailers.append("x-sirius-error-code", "UNDER_MAINTENANCE".parse().unwrap());
        let frame: http_body::Frame<bytes::Bytes> = http_body::Frame::trailers(trailers);
        let body = Body::new(StreamBody::new(futures_util::stream::iter([Ok::<
            _,
            std::convert::Infallible,
        >(
            frame
        )])));
        http::Response::builder()
            .header("content-type", "application/grpc")
            .header("x-sirius-error-code", "UNKNOWN_SYNTHETIC_CODE")
            .body(body)
            .unwrap()
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap()
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let client =
        Client::with_transport(config(), None, options(), Arc::new(mock_channel(channel))).unwrap();
    let error = client
        .query(Query::Version(Default::default()))
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Maintenance);
    assert_eq!(error.business_codes().0, &["UNKNOWN_SYNTHETIC_CODE"]);
    assert_eq!(
        error.business_codes().1,
        &["TOKEN_ILLEGAL", "UNDER_MAINTENANCE"]
    );
    assert!(!format!("{error:?}").contains("SYNTHETIC"));
    assert!(!format!("{error:?}").contains("private"));
    drop(client);
    stop.cancel();
    server.await.unwrap();
}

struct Slow {
    calls: AtomicUsize,
    delay: Duration,
}

#[test]
fn oversized_rate_interval_is_rejected_before_requests() {
    let transport = Arc::new(Slow {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
    });
    let result = Client::with_transport(
        config(),
        None,
        ClientOptions {
            minimum_interval: Duration::MAX,
            ..options()
        },
        transport.clone(),
    );
    assert!(matches!(
        result,
        Err(ClientError {
            kind: ErrorKind::InvalidConfig,
            ..
        })
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}
#[async_trait]
impl Transport for Slow {
    async fn call(
        &self,
        method: Method,
        _: DynamicMessage,
        _: MetadataMap,
        _: Duration,
    ) -> Result<DynamicMessage, ClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        Ok(DynamicMessage::new(
            pool().get_message_by_name(method.output).unwrap(),
        ))
    }
}

#[tokio::test]
async fn deadlines_cancellation_queue_and_session_replacement() {
    let slow = Arc::new(Slow {
        calls: AtomicUsize::new(0),
        delay: Duration::from_secs(60),
    });
    let opts = ClientOptions {
        timeout: Duration::from_millis(100),
        queue_capacity: 0,
        ..options()
    };
    let client = Arc::new(Client::with_transport(config(), None, opts, slow.clone()).unwrap());
    let c = client.clone();
    let pending = tokio::spawn(async move { c.query(Query::Version(Default::default())).await });
    while slow.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        client
            .query(Query::Version(Default::default()))
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::QueueFull
    );
    assert_eq!(
        pending.await.unwrap().err().unwrap().kind,
        ErrorKind::Timeout
    );
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        client
            .execute(
                client.generation(),
                Query::Version(Default::default()),
                token
            )
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::Cancelled
    );
    let old_generation = client.generation();
    let c = client.clone();
    let pending = tokio::spawn(async move { c.query(Query::Version(Default::default())).await });
    while slow.calls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    client.replace_session(config(), None).unwrap();
    assert_eq!(
        pending.await.unwrap().err().unwrap().kind,
        ErrorKind::SessionChanged
    );
    assert_eq!(
        client
            .execute(
                old_generation,
                Query::Version(Default::default()),
                CancellationToken::new()
            )
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::SessionChanged
    );
    // No call is made through the replacement's production HTTPS transport.
}

#[tokio::test]
async fn serialization_rate_limit_and_missing_credentials() {
    let slow = Arc::new(Slow {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
    });
    let client = Client::with_transport(
        config(),
        None,
        ClientOptions {
            minimum_interval: Duration::from_millis(30),
            ..options()
        },
        slow.clone(),
    )
    .unwrap();
    assert_eq!(
        client
            .query(Query::Whoami(Default::default()))
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::AuthenticationRequired
    );
    let start = Instant::now();
    client
        .query(Query::Version(Default::default()))
        .await
        .unwrap();
    client
        .query(Query::Version(Default::default()))
        .await
        .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(30));
    assert_eq!(slow.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn origins_credentials_and_strict_requests() {
    for bad in [
        "http://game.invalid",
        "https://user@game.invalid",
        "https://game.invalid/path",
        "https://game.invalid?x=1",
        "https://other.invalid",
    ] {
        let mut c = config();
        c.origin = bad.into();
        assert!(c.validate().is_err());
    }
    let mut c = config();
    c.region = "different".into();
    assert!(credentials().credentials(&c).is_err());
    assert!(!format!("{:?}", credentials().credentials(&config()).unwrap()).contains("synthetic"));
    assert!(
        Query::from_json(
            "announcement",
            serde_json::json!({"id":"1","extra":"value"})
        )
        .is_err()
    );
    assert!(Query::from_json("arbitrary-rpc", serde_json::json!({})).is_err());
    assert!(Query::from_json("announcements", serde_json::json!({"selectedTab":3})).is_err());
    assert!(
        Query::from_json(
            "probability",
            serde_json::json!({"gachaId":"1","productId":"2"})
        )
        .is_err()
    );
    assert!(Query::from_json("circle-search", serde_json::json!({})).is_err());
    assert_eq!(METHODS.iter().filter(|m| m.http).count(), 15);
}

#[test]
fn error_classification_and_unknown_codes() {
    for (code, kind) in [
        ("TOKEN_MISSING", ErrorKind::Authentication),
        ("CONCURRENT_DEVICE", ErrorKind::DeviceConflict),
        ("MASTER_VERSION_MISMATCH", ErrorKind::Version),
        ("CLIENT_UPDATE_REQUIRED", ErrorKind::Version),
        ("FUTURE_CODE", ErrorKind::Business),
    ] {
        let mut metadata = MetadataMap::new();
        metadata.insert("x-sirius-error-code", code.parse().unwrap());
        let error =
            ClientError::from_metadata(tonic::Code::Unknown, &MetadataMap::new(), &metadata)
                .unwrap();
        assert_eq!(error.kind, kind);
        assert_eq!(error.business_codes().1, [code]);
    }
}

struct Rejected {
    calls: AtomicUsize,
}
#[async_trait]
impl Transport for Rejected {
    async fn call(
        &self,
        _: Method,
        _: DynamicMessage,
        _: MetadataMap,
        _: Duration,
    ) -> Result<DynamicMessage, ClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ClientError::new(ErrorKind::Authentication))
    }
}

#[tokio::test]
async fn rejected_session_blocks_subsequent_authenticated_calls() {
    let transport = Arc::new(Rejected {
        calls: AtomicUsize::new(0),
    });
    let client =
        Client::with_transport(config(), Some(&credentials()), options(), transport.clone())
            .unwrap();
    assert_eq!(
        client.session_status(),
        SessionStatus::CredentialsUnverified
    );
    for _ in 0..2 {
        assert_eq!(
            client
                .query(Query::Whoami(Default::default()))
                .await
                .err()
                .unwrap()
                .kind,
            ErrorKind::Authentication
        );
    }
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        client.session_status(),
        SessionStatus::AuthenticationRejected
    );
    client.replace_session(config(), None).unwrap();
    assert_eq!(client.session_status(), SessionStatus::Anonymous);
}

#[test]
fn registry_matches_descriptor_input_output_and_authentication_options() {
    for method in METHODS
        .iter()
        .chain([&crate::auth::LOGIN, &crate::auth::PRE_LOGIN])
    {
        let (service, name) = method.path[1..].split_once('/').unwrap();
        let service = pool().get_service_by_name(service).unwrap();
        let rpc = service.methods().find(|m| m.name() == name).unwrap();
        assert_eq!(rpc.input().full_name(), method.input);
        assert_eq!(rpc.output().full_name(), method.output);
        let options = rpc.options();
        let extension = pool()
            .get_extension_by_name("entity.method_options.skip_authentication")
            .unwrap();
        assert_eq!(
            *options.get_extension(&extension),
            prost_reflect::Value::Bool(method.anonymous)
        );
        assert!(!rpc.is_client_streaming());
        assert!(!rpc.is_server_streaming());
    }
}

#[cfg(unix)]
#[test]
fn credentials_file_permissions_and_origin_binding() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.json");
    let contents = serde_json::json!({"region":"test","origin":"https://game.invalid","credentials":{
        "player_id":"synthetic-player","credential":"synthetic-private-value","device_id":null,"bid":null
    }}).to_string();
    std::fs::write(&path, contents).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(StaticCredentials::from_file(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let provider = StaticCredentials::from_file(&path).unwrap();
    assert!(provider.credentials(&config()).unwrap().is_some());
    let mut config = config();
    config.origin = "https://other.invalid".into();
    assert!(provider.credentials(&config).is_err());
}
