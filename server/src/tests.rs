use super::*;
use crate::cache::QueryCache;
use async_trait::async_trait;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use moenotes_client::{Generation, Query, QueryResponse};
use prost_reflect::DynamicMessage;
use std::{
    sync::{
        RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;

const KEY: &str = "synthetic-http-key-not-for-production-123456789";

#[test]
fn v1_public_openapi_contract_is_frozen() {
    assert_eq!(
        ROUTES.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        vec![
            "/v1/announcement",
            "/v1/announcements",
            "/v1/arena/ranking",
            "/v1/arena/deck-trend",
            "/v1/circle",
            "/v1/circles/recommended",
            "/v1/circles/search",
            "/v1/event/challenge-ranking",
            "/v1/event/deck",
            "/v1/event/ranking",
            "/v1/profile",
            "/v1/gacha/rates",
            "/v1/music/ranking",
            "/v1/profile/favorites",
            "/v1/profiles"
        ]
    );
    let mut actual = openapi::document_for(projection::ResponseMode::Public);
    actual.as_object_mut().unwrap().remove("info");
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/v1-openapi.json")).unwrap();
    // Additive path/region aliases must not alter any established endpoint/schema.
    actual["paths"]
        .as_object_mut()
        .unwrap()
        .retain(|path, _| expected["paths"].get(path).is_some());
    assert_eq!(
        actual, expected,
        "Public v1 changes require explicit compatibility review"
    );
}
struct Fake {
    generation: RwLock<Generation>,
    calls: AtomicUsize,
    failure: AtomicBool,
    blocked: AtomicBool,
    delay: Duration,
}
impl Fake {
    fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            generation: RwLock::new(Generation::new_v4()),
            calls: AtomicUsize::new(0),
            failure: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
            delay,
        })
    }
    fn replace(&self) {
        *self.generation.write().unwrap() = Generation::new_v4();
    }
}
#[async_trait]
impl QueryClient for Fake {
    fn query_error(&self, anonymous: bool) -> Option<ClientError> {
        if !anonymous && self.blocked.load(Ordering::SeqCst) {
            Some(ClientError::new(ErrorKind::Authentication))
        } else {
            None
        }
    }
    fn generation(&self) -> Generation {
        *self.generation.read().unwrap()
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        cancel: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::select! { _ = tokio::time::sleep(self.delay) => {}, _ = cancel.cancelled() => return Err(ClientError::new(ErrorKind::Cancelled)) }
        if self.failure.load(Ordering::SeqCst) {
            return Err(ClientError::new(ErrorKind::Maintenance));
        }
        let descriptor = moenotes_proto::pool()
            .get_message_by_name(query.method().output)
            .unwrap();
        let message = if query.method().name == "favorite-status" {
            DynamicMessage::deserialize(
                descriptor,
                serde_json::json!({"totalFavorite":"9007199254740993","isSentFavorite":true}),
            )
            .unwrap()
        } else {
            DynamicMessage::new(descriptor)
        };
        Ok(QueryResponse {
            message,
            fetched_at: SystemTime::now(),
            generation,
        })
    }
}

#[tokio::test]
async fn blocked_session_never_serves_cached_private_response() {
    let fake = Fake::new(Duration::ZERO);
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions::default(),
        CancellationToken::new(),
    );
    let query = Query::from_json(
        "favorite-status",
        serde_json::json!({"playerId":"synthetic"}),
    )
    .unwrap();
    cache.query(query.clone()).await.unwrap();
    fake.blocked.store(true, Ordering::SeqCst);
    assert!(matches!(
        cache.query(query).await,
        Err(ClientError {
            kind: ErrorKind::Authentication,
            ..
        })
    ));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn public_policy_and_diagnostics_are_authenticated_and_redacted() {
    let fake = Fake::new(Duration::ZERO);
    let app = router_with_options(
        fake.clone(),
        Zeroizing::new(KEY.into()),
        RouterOptions {
            mode: projection::ResponseMode::Public,
            managed: None,
            access_log: false,
        },
        CacheOptions::default(),
        CancellationToken::new(),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(
            "/v1/profile/favorites",
            true,
            serde_json::json!({"playerId":"synthetic"}),
        ))
        .await
        .unwrap();
    assert!(response.headers().contains_key("x-request-id"));
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"totalFavorite":"9007199254740993"})
    );
    let denied = app
        .clone()
        .oneshot(request(
            "/v1/circles/recommended",
            true,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.clone()
            .oneshot(request("/v1/status", false, serde_json::json!({})))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let status = app
        .clone()
        .oneshot(request("/v1/status", true, serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(status.headers()["cache-control"], "no-store");
    let raw = status.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(raw.to_vec()).unwrap();
    assert!(!text.contains(KEY) && !text.contains("playerId"));
    assert_eq!(
        app.oneshot(request("/readyz", true, serde_json::json!({})))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let doc = openapi::document_for(projection::ResponseMode::Public);
    assert!(doc["paths"].get("/v1/circles/recommended").is_none());
    assert!(
        doc["components"]["schemas"]["app.player.GetPlayerFavoriteStatusResponse"]["properties"]
            .get("isSentFavorite")
            .is_none()
    );
}
fn q() -> Query {
    Query::from_json("announcement", serde_json::json!({"id":"1"})).unwrap()
}

#[tokio::test]
async fn overflowing_cache_ttl_is_rejected_without_upstream_work() {
    let fake = Fake::new(Duration::ZERO);
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            ttl: Duration::MAX,
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let result = tokio::time::timeout(Duration::from_secs(1), cache.query(q()))
        .await
        .unwrap();
    assert!(matches!(
        result,
        Err(ClientError {
            kind: ErrorKind::InvalidConfig,
            ..
        })
    ));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}
fn request(path: &str, key: bool, body: serde_json::Value) -> Request<Body> {
    fn pairs(prefix: &str, value: &serde_json::Value, out: &mut Vec<(String, String)>) {
        match value {
            serde_json::Value::Object(values) => {
                for (name, value) in values {
                    pairs(
                        &if prefix.is_empty() {
                            name.clone()
                        } else {
                            format!("{prefix}.{name}")
                        },
                        value,
                        out,
                    );
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    pairs(prefix, value, out);
                }
            }
            serde_json::Value::String(value) => out.push((prefix.to_owned(), value.clone())),
            other => out.push((prefix.to_owned(), other.to_string())),
        }
    }
    let mut parameters = Vec::new();
    pairs("", &body, &mut parameters);
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(parameters)
        .finish();
    let mut request = Request::builder().method("GET").uri(if encoded.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{encoded}")
    });
    if key {
        request = request.header("authorization", format!("Bearer {KEY}"));
    }
    request.body(Body::empty()).unwrap()
}
fn app(fake: Arc<Fake>, enabled: bool) -> Router {
    router(
        fake,
        Zeroizing::new(KEY.into()),
        enabled,
        CacheOptions::default(),
        CancellationToken::new(),
    )
    .unwrap()
}

#[tokio::test]
async fn routes_default_off_auth_health_and_json() {
    let fake = Fake::new(Duration::ZERO);
    let disabled = app(fake.clone(), false);
    let response = disabled
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        disabled
            .oneshot(request(ROUTES[0].0, true, serde_json::json!({"id":"1"})))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    let enabled = app(fake.clone(), true);
    assert_eq!(
        enabled
            .clone()
            .oneshot(request(ROUTES[0].0, false, serde_json::json!({"id":"1"})))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut invalid_key = request(ROUTES[0].0, true, serde_json::json!({"id":"1"}));
    invalid_key
        .headers_mut()
        .append("authorization", "Bearer wrong".parse().unwrap());
    assert_eq!(
        enabled.clone().oneshot(invalid_key).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let response = enabled
        .clone()
        .oneshot(request(
            "/v1/profile/favorites",
            true,
            serde_json::json!({"playerId":"synthetic"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-moenotes-cache"], "MISS");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers().contains_key("x-moenotes-fetched-at"));
    let value: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"totalFavorite":"9007199254740993","isSentFavorite":true})
    );
    let response = enabled
        .clone()
        .oneshot(request(
            "/v1/profile/favorites",
            true,
            serde_json::json!({"playerId":"synthetic"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.headers()["x-moenotes-cache"], "HIT");
    for body in [
        serde_json::json!({"id":"0"}),
        serde_json::json!({"id":"1","unknown":1}),
        serde_json::json!({"id":"not-an-id"}),
    ] {
        assert_eq!(
            enabled
                .clone()
                .oneshot(request(ROUTES[0].0, true, body))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn all_http_routes_and_openapi() {
    let fake = Fake::new(Duration::ZERO);
    let enabled = app(fake.clone(), true);
    let inputs = [
        serde_json::json!({"id":"1"}),
        serde_json::json!({}),
        serde_json::json!({"arenaSeasonId":"1","rankingStart":"1","rankingEnd":"100"}),
        serde_json::json!({"musicId":"1","arenaSeasonId":"1"}),
        serde_json::json!({"circleId":"1"}),
        serde_json::json!({}),
        serde_json::json!({"options":{}}),
        serde_json::json!({"challengeMusicId":"1"}),
        serde_json::json!({"playerId":"synthetic","eventId":"1"}),
        serde_json::json!({"eventId":"1","ranks":[1,10]}),
        serde_json::json!({"playerProfileId":"1"}),
        serde_json::json!({"gachaId":"1"}),
        serde_json::json!({"musicId":"1"}),
        serde_json::json!({"playerId":"synthetic"}),
        serde_json::json!({"accountIds":["1"]}),
    ];
    for ((path, _), input) in ROUTES.iter().zip(inputs) {
        assert_eq!(
            enabled
                .clone()
                .oneshot(request(path, true, input))
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
            "{path}"
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 15);
    let response = enabled
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .header("authorization", format!("Bearer {KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["paths"].as_object().unwrap().len(), 161);
    for (path, name) in ROUTES {
        let method = moenotes_client::METHODS
            .iter()
            .find(|m| m.name == *name)
            .unwrap();
        let operation = &json["paths"][path]["get"];
        assert!(operation.is_object());
        assert!(operation.get("requestBody").is_none());
        assert!(json["paths"][path].get("post").is_none());
        let declared: Vec<_> = operation["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        let expected = crate::query_params::fields(
            moenotes_proto::pool()
                .get_message_by_name(method.input)
                .unwrap(),
        );
        assert_eq!(
            declared,
            expected
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
        );
    }
    let ranks = json["paths"]["/v1/event/ranking"]["get"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "ranks")
        .unwrap();
    assert_eq!(ranks["explode"], true);
    assert_eq!(ranks["required"], true);
    assert_eq!(ranks["schema"]["type"], "array");
    assert_eq!(
        json["components"]["schemas"]["app.announcement.GetRequest"]["properties"]["id"]["type"],
        "string"
    );
}

#[tokio::test]
async fn coalescing_failures_expiry_and_session_isolation() {
    let fake = Fake::new(Duration::from_millis(15));
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            ttl: Duration::from_millis(30),
            ..CacheOptions::default()
        },
        CancellationToken::new(),
    );
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let cache = cache.clone();
        tasks.push(tokio::spawn(
            async move { cache.query(q()).await.unwrap().1 },
        ));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.query(q()).await.unwrap().1, "HIT");
    tokio::time::sleep(Duration::from_millis(40)).await;
    cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    fake.replace();
    cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 3);
    fake.replace();
    fake.failure.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        assert_eq!(
            cache.query(q()).await.err().unwrap().kind,
            ErrorKind::Maintenance
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn stale_flight_rejected_and_caller_drop_does_not_poison_cache() {
    let fake = Fake::new(Duration::from_millis(30));
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions::default(),
        CancellationToken::new(),
    );
    let worker = cache.clone();
    let pending = tokio::spawn(async move { worker.query(q()).await });
    while fake.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    fake.replace();
    assert_eq!(
        pending.await.unwrap().err().unwrap().kind,
        ErrorKind::SessionChanged
    );
    assert_eq!(cache.query(q()).await.unwrap().1, "MISS");
    fake.replace();
    let worker = cache.clone();
    let pending = tokio::spawn(async move { worker.query(q()).await });
    while fake.calls.load(Ordering::SeqCst) < 3 {
        tokio::task::yield_now().await;
    }
    pending.abort();
    let _ = pending.await;
    cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn capacity_backpressure_shutdown_and_safe_errors() {
    let fake = Fake::new(Duration::from_millis(30));
    let stop = CancellationToken::new();
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            capacity: 1,
            max_inflight: 1,
            ..CacheOptions::default()
        },
        stop.clone(),
    );
    let worker = cache.clone();
    let pending = tokio::spawn(async move { worker.query(q()).await });
    while fake.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let other = Query::from_json("announcement", serde_json::json!({"id":"2"})).unwrap();
    assert_eq!(
        cache.query(other.clone()).await.err().unwrap().kind,
        ErrorKind::QueueFull
    );
    pending.await.unwrap().unwrap();
    cache.query(other).await.unwrap();
    assert_eq!(cache.query(q()).await.unwrap().1, "MISS");
    stop.cancel();
    assert_eq!(
        cache.query(q()).await.err().unwrap().kind,
        ErrorKind::Cancelled
    );
    fake.failure.store(true, Ordering::SeqCst);
    let response = app(fake, true)
        .oneshot(request(ROUTES[0].0, true, serde_json::json!({"id":"1"})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &response.into_body().collect().await.unwrap().to_bytes()
        )
        .unwrap(),
        serde_json::json!({"error":{"kind":"maintenance"}})
    );
}

#[tokio::test]
async fn bounded_bytes_and_full_request_cache_keys() {
    let fake = Fake::new(Duration::ZERO);
    let no_cache = QueryCache::new(
        fake.clone(),
        CacheOptions {
            max_bytes: 1,
            ..CacheOptions::default()
        },
        CancellationToken::new(),
    );
    no_cache.query(q()).await.unwrap();
    no_cache.query(q()).await.unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    let cache = QueryCache::new(
        fake.clone(),
        CacheOptions::default(),
        CancellationToken::new(),
    );
    for ranks in [vec![1, 10], vec![10, 1], vec![1, 10, 1], vec![1, 10]] {
        let query = Query::from_json(
            "event-ranking",
            serde_json::json!({"eventId":"1","ranks":ranks}),
        )
        .unwrap();
        cache.query(query).await.unwrap();
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn malformed_oversized_input_and_weak_key_rejected() {
    let fake = Fake::new(Duration::ZERO);
    assert!(
        router(
            fake.clone(),
            Zeroizing::new("short".into()),
            true,
            CacheOptions::default(),
            CancellationToken::new()
        )
        .is_err()
    );
    let app = app(fake.clone(), true);
    for body in ["{".to_owned(), "x".repeat(65537)] {
        let request = Request::builder()
            .method("GET")
            .uri(format!("{}?id=1", ROUTES[0].0))
            .header("authorization", format!("Bearer {KEY}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn get_only_routes_reject_old_paths_bodies_and_ambiguous_query_strings() {
    let fake = Fake::new(Duration::ZERO);
    let app = app(fake.clone(), true);
    for method in ["POST", "PUT", "DELETE", "HEAD"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/v1/profile?playerProfileId=1")
                    .header("authorization", format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method}"
        );
    }
    for method in ["POST", "GET"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/experimental/v1/profiles/find")
                    .header("authorization", format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    for query in [
        "playerProfileId=1&playerProfileId=2".into(),
        "playerProfileId=%GG".into(),
        "playerProfileId=1&unknown=x".into(),
        format!("playerProfileId={}", "1".repeat(8192)),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/profile?{query}"))
                    .header("authorization", format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn get_query_cache_uses_parsed_request_and_preserves_array_order() {
    let fake = Fake::new(Duration::ZERO);
    let app = app(fake.clone(), true);
    for (raw, expected) in [
        ("eventId=1&ranks=10&ranks=1", "MISS"),
        ("ranks=10&eventId=1&ranks=1", "HIT"),
        ("eventId=1&ranks=1&ranks=10", "MISS"),
        ("eventId=1&ranks=10&ranks=1&ranks=10", "MISS"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/event/ranking?{raw}"))
                    .header("authorization", format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-moenotes-cache"], expected);
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 3);
}
