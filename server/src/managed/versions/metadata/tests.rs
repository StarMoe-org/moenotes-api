use super::*;
use axum::{Router, http::HeaderMap, routing::get};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub(in crate::managed::versions) struct Fixture {
    pub source: MetadataSource,
    pub calls: Arc<AtomicUsize>,
    pub response: Arc<std::sync::Mutex<(reqwest::StatusCode, String)>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(in crate::managed::versions) async fn fixture(snapshot: serde_json::Value) -> Fixture {
    let calls = Arc::new(AtomicUsize::new(0));
    let response = Arc::new(std::sync::Mutex::new((
        reqwest::StatusCode::OK,
        snapshot.to_string(),
    )));
    let handler_calls = calls.clone();
    let handler_response = response.clone();
    let router = Router::new().route(
        "/current_version.json",
        get(move |headers: HeaderMap| {
            let calls = handler_calls.clone();
            let response = handler_response.clone();
            async move {
                assert_eq!(headers.get("accept").unwrap(), "application/json");
                assert!(!headers.contains_key("authorization"));
                assert!(!headers.contains_key("cookie"));
                assert!(!headers.contains_key("x-client-version"));
                calls.fetch_add(1, Ordering::SeqCst);
                let (status, body) = response.lock().unwrap().clone();
                (
                    status,
                    [
                        ("location", "/unexpected"),
                        ("content-type", "application/json"),
                    ],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/current_version.json",
        listener.local_addr().unwrap()
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    Fixture {
        source: MetadataSource::new(url, Some(http)),
        calls,
        response,
        task,
    }
}

#[tokio::test]
async fn maps_all_regions_and_shares_one_snapshot() {
    let fixture = fixture(serde_json::json!({"schema_version":1,"regions":{
        "hk-tw-mo":{"client_version":"1.2.3"},
        "en":{"client_version":"1.0.2"},
        "kr":{"client_version":"1.0.3"},
        "jp":{"client_version":"2.0.0"}
    }}))
    .await;
    for (region, expected) in [
        ("hk", "1.2.3"),
        ("tw", "1.2.3"),
        ("hk-tw-mo", "1.2.3"),
        ("en", "1.0.2"),
        ("kr", "1.0.3"),
        ("jp", "2.0.0"),
    ] {
        assert_eq!(
            fixture
                .source
                .candidate(region, "1.0.1", CancellationToken::new())
                .await
                .as_deref(),
            Some(expected)
        );
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_missing_malformed_equal_or_older_versions() {
    for version in [
        "1.0.1",
        "1.0.0",
        "0.9.99",
        "1.0",
        "1.0.2-beta",
        "1.0.2\n",
        "1.0.1000000000",
    ] {
        let fixture = fixture(serde_json::json!({"schema_version":1,"regions":{
            "en":{"client_version":version},"jp":{"client_version":"2.0.0"}
        }}))
        .await;
        assert_eq!(
            fixture
                .source
                .candidate("en", "1.0.1", CancellationToken::new())
                .await,
            None,
            "{version}"
        );
        assert_eq!(
            fixture
                .source
                .candidate("kr", "1.0.1", CancellationToken::new())
                .await,
            None
        );
    }
    let fixture = fixture(serde_json::json!({"schema_version":1,"regions":{}})).await;
    assert_eq!(
        fixture
            .source
            .candidate("unknown", "1.0.1", CancellationToken::new())
            .await,
        None
    );
    assert_eq!(
        fixture
            .source
            .candidate("en", "1", CancellationToken::new())
            .await,
        None
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bounds_and_caches_failed_fetches_without_following_redirects() {
    for (status, body) in [
        (reqwest::StatusCode::SERVICE_UNAVAILABLE, "{}".to_owned()),
        (reqwest::StatusCode::FOUND, "{}".to_owned()),
        (reqwest::StatusCode::OK, "not-json".to_owned()),
        (reqwest::StatusCode::OK, " ".repeat(MAX_BYTES + 1)),
        (
            reqwest::StatusCode::OK,
            "{\"schema_version\":2,\"regions\":{}}".to_owned(),
        ),
        (reqwest::StatusCode::OK, "{\"schema_version\":1}".to_owned()),
    ] {
        let fixture = fixture(serde_json::json!({})).await;
        *fixture.response.lock().unwrap() = (status, body);
        for _ in 0..3 {
            assert_eq!(
                fixture
                    .source
                    .candidate("en", "1.0.1", CancellationToken::new())
                    .await,
                None
            );
        }
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn refreshes_a_failed_snapshot_after_the_cache_interval() {
    let fixture = fixture(serde_json::json!({})).await;
    assert_eq!(
        fixture
            .source
            .candidate("en", "1.0.1", CancellationToken::new())
            .await,
        None
    );
    *fixture.response.lock().unwrap() = (
        reqwest::StatusCode::OK,
        serde_json::json!({"schema_version":1,"regions":{"en":{"client_version":"1.0.2"}}})
            .to_string(),
    );
    fixture.source.cache.lock().await.checked_at = Some(Instant::now() - TTL);
    assert_eq!(
        fixture
            .source
            .candidate("en", "1.0.1", CancellationToken::new())
            .await
            .as_deref(),
        Some("1.0.2")
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancellation_interrupts_waiting_for_the_shared_cache() {
    let fixture = fixture(serde_json::json!({})).await;
    let _guard = fixture.source.cache.lock().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(100),
            fixture.source.candidate("en", "1.0.1", cancel)
        )
        .await
        .unwrap(),
        None
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_pool_members_share_one_http_fetch() {
    let fixture = fixture(
        serde_json::json!({"schema_version":1,"regions":{"en":{"client_version":"1.0.2"}}}),
    )
    .await;
    let (first, second, third) = tokio::join!(
        fixture
            .source
            .candidate("en", "1.0.1", CancellationToken::new()),
        fixture
            .source
            .candidate("en", "1.0.1", CancellationToken::new()),
        fixture
            .source
            .candidate("en", "1.0.1", CancellationToken::new())
    );
    for candidate in [first, second, third] {
        assert_eq!(candidate.as_deref(), Some("1.0.2"));
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
}
