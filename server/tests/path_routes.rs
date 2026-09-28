use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use moenotes_client::{
    CancellationToken, ClientError, ErrorKind, Generation, Query, QueryClient, QueryResponse,
};
use moenotes_server::{
    RouterOptions,
    cache::CacheOptions,
    projection::ResponseMode,
    regions::{Region, RegionBackend, RegionClients},
    router_with_regions,
};
use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tower::ServiceExt;
const KEY: &str = "synthetic-routing-key-not-for-production-123456";
struct Mock {
    generation: Generation,
    calls: Mutex<Vec<Query>>,
    failure: Mutex<Option<ErrorKind>>,
}
impl Mock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            generation: Generation::new_v4(),
            calls: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
        })
    }
}
#[async_trait]
impl QueryClient for Mock {
    fn generation(&self) -> Generation {
        self.generation
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        _: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        self.calls.lock().unwrap().push(query.clone());
        if let Some(kind) = *self.failure.lock().unwrap() {
            return Err(ClientError::new(kind));
        }
        let message = prost_reflect::DynamicMessage::new(
            moenotes_proto::pool()
                .get_message_by_name(query.method().output)
                .unwrap(),
        );
        Ok(QueryResponse {
            generation,
            message,
            fetched_at: SystemTime::now(),
        })
    }
}
fn app(mode: ResponseMode, include_en: bool) -> (Router, Arc<Mock>, Arc<Mock>) {
    let tw = Mock::new();
    let en = Mock::new();
    let mut backends = vec![RegionBackend {
        region: Region::Tw,
        client: tw.clone(),
        managed: None,
    }];
    if include_en {
        backends.push(RegionBackend {
            region: Region::En,
            client: en.clone(),
            managed: None,
        });
    }
    let stop = CancellationToken::new();
    let options = CacheOptions::default();
    let regions =
        RegionClients::new(Some(Region::Tw), backends, options.clone(), stop.clone()).unwrap();
    let app = router_with_regions(
        tw.clone(),
        KEY.to_owned().into(),
        RouterOptions {
            mode,
            managed: None,
            access_log: false,
        },
        options,
        stop,
        regions,
    )
    .unwrap();
    (app, tw, en)
}

#[tokio::test]
async fn jp_profile_uses_explicit_region_without_guessing_an_id_prefix() {
    let jp = Mock::new();
    let tw = Mock::new();
    let stop = CancellationToken::new();
    let options = CacheOptions::default();
    let regions = RegionClients::new(
        Some(Region::Tw),
        vec![
            RegionBackend {
                region: Region::Tw,
                client: tw.clone(),
                managed: None,
            },
            RegionBackend {
                region: Region::Jp,
                client: jp.clone(),
                managed: None,
            },
        ],
        options.clone(),
        stop.clone(),
    )
    .unwrap();
    let app = router_with_regions(
        tw.clone(),
        KEY.to_owned().into(),
        RouterOptions {
            mode: ResponseMode::Public,
            managed: None,
            access_log: false,
        },
        options,
        stop,
        regions,
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request("/v1/jp/profile/50000000001"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-moenotes-region"], "jp");
    assert_eq!(
        app.clone()
            .oneshot(request("/v1/profile/50000000001"))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.clone()
            .oneshot(request("/v1/jp/profile/-1"))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(jp.calls.lock().unwrap().len(), 1);
    assert!(tw.calls.lock().unwrap().is_empty());
}
fn request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {KEY}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn profile_auto_selection_explicit_region_cache_and_failures_are_isolated() {
    let (app, tw, en) = app(ResponseMode::Public, true);
    for (path, region, cache) in [
        ("/v1/profile/20000000001", "tw", "MISS"),
        ("/v1/tw/profile/20000000001", "tw", "HIT"),
        ("/v1/profile/30000000001", "en", "MISS"),
        ("/v1/en/profile/30000000001", "en", "HIT"),
    ] {
        let response = app.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["x-moenotes-region"], region);
        assert_eq!(response.headers()["x-moenotes-cache"], cache);
    }
    let response = app
        .clone()
        .oneshot(request("/v1/profile?playerProfileId=20000000001"))
        .await
        .unwrap();
    assert_eq!(response.headers()["x-moenotes-cache"], "HIT");
    assert_eq!(tw.calls.lock().unwrap().len(), 1);
    assert_eq!(en.calls.lock().unwrap().len(), 1);
    *en.failure.lock().unwrap() = Some(ErrorKind::Authentication);
    let response = app
        .clone()
        .oneshot(request("/v1/profile/30000000002"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        app.clone()
            .oneshot(request("/v1/profile/20000000002"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(tw.calls.lock().unwrap().len(), 2);
    assert_eq!(en.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_and_unconfigured_profiles_never_fall_back_or_query_upstream() {
    let (app, tw, en) = app(ResponseMode::Public, false);
    for (path, status, kind) in [
        ("/v1/profile/30000000001", 503, "region_unconfigured"),
        ("/v1/profile/40000000001", 503, "region_unconfigured"),
        ("/v1/profile/90000000001", 400, "unsupported_profile_region"),
        ("/v1/profile/020000000001", 400, "invalid_profile_id"),
        ("/v1/profile/2000000000x", 400, "invalid_profile_id"),
        ("/v1/profile/2", 400, "invalid_profile_id"),
        ("/v1/en/profile/20000000001", 400, "profile_region_mismatch"),
        (
            "/v1/profile/20000000001?playerProfileId=30000000001",
            400,
            "invalid_request",
        ),
    ] {
        let r = app.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(r.status().as_u16(), status, "{path}");
        assert_eq!(r.headers()["cache-control"], "no-store");
        let body: serde_json::Value =
            serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["error"]["kind"], kind, "{path}");
    }
    assert!(tw.calls.lock().unwrap().is_empty());
    assert!(en.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn path_aliases_encode_the_same_requests_as_legacy_routes() {
    let (app, tw, en) = app(ResponseMode::Raw, true);
    for (path, legacy) in [
        ("/v1/music/69/ranking", "/v1/music/ranking?musicId=69"),
        (
            "/v1/event/123/ranking/1,10,100,10",
            "/v1/event/ranking?eventId=123&ranks=1&ranks=10&ranks=100&ranks=10",
        ),
        (
            "/v1/arena/7/ranking/1/100",
            "/v1/arena/ranking?arenaSeasonId=7&rankingStart=1&rankingEnd=100",
        ),
        (
            "/v1/arena/7/ranking/1/100/band/0",
            "/v1/arena/ranking?arenaSeasonId=7&rankingStart=1&rankingEnd=100&bandId=0",
        ),
        (
            "/v1/arena/7/music/69/deck-trend",
            "/v1/arena/deck-trend?arenaSeasonId=7&musicId=69",
        ),
        (
            "/v1/event/challenge/8/ranking",
            "/v1/event/challenge-ranking?challengeMusicId=8",
        ),
        (
            "/v1/event/123/deck/player%2Bid",
            "/v1/event/deck?eventId=123&playerId=player%2Bid",
        ),
        (
            "/v1/player/player%2Bid/favorites",
            "/v1/profile/favorites?playerId=player%2Bid",
        ),
        (
            "/v1/profiles/20000000001,20000000002,20000000001",
            "/v1/profiles?accountIds=20000000001&accountIds=20000000002&accountIds=20000000001",
        ),
        ("/v1/announcements/BUG", "/v1/announcements?selectedTab=BUG"),
        ("/v1/announcement/12", "/v1/announcement?id=12"),
        ("/v1/gacha/1/rates", "/v1/gacha/rates?gachaId=1"),
        (
            "/v1/gacha/1/rates/2,3,2",
            "/v1/gacha/rates?gachaId=1&selectedPickUp=2&selectedPickUp=3&selectedPickUp=2",
        ),
        (
            "/v1/circle/18446744073709551615",
            "/v1/circle?circleId=18446744073709551615",
        ),
        (
            "/v1/circles/search/a+b%20c?options.memberRange=0",
            "/v1/circles/search?options.name=a%2Bb%20c&options.memberRange=0",
        ),
    ] {
        let response = app.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let response = app.clone().oneshot(request(legacy)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{legacy}");
        assert_eq!(response.headers()["x-moenotes-cache"], "HIT", "{legacy}");
        let regional = path.replacen("/v1/", "/v1/en/", 1);
        let response = app.clone().oneshot(request(&regional)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{regional}");
        assert_eq!(response.headers()["x-moenotes-cache"], "MISS");
    }
    let tw = tw.calls.lock().unwrap();
    let en = en.calls.lock().unwrap();
    assert_eq!(tw.len(), 15);
    assert_eq!(en.len(), 15);
    for (a, b) in tw.iter().zip(en.iter()) {
        assert_eq!(a.method().name, b.method().name);
        assert_eq!(a.encode(), b.encode());
    }
}

#[tokio::test]
async fn aliases_keep_authentication_methods_body_limits_and_public_policy() {
    let (app, tw, _) = app(ResponseMode::Public, true);
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/profile/20000000001")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    for path in [
        "/v1/music/1/ranking",
        "/v1/en/music/1/ranking",
        "/v1/profile/20000000001",
    ] {
        for method in ["POST", "HEAD"] {
            let mut r = request(path);
            *r.method_mut() = method.parse().unwrap();
            assert_eq!(
                app.clone().oneshot(r).await.unwrap().status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
        }
        let mut r = request(path);
        *r.body_mut() = Body::from("{}");
        assert_eq!(
            app.clone().oneshot(r).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    for path in [
        "/v1/event/1/ranking/1,,2",
        "/v1/music/0/ranking",
        "/v1/music/1/ranking?musicId=2",
        "/v1/event/1/ranking/1,2?ranks=3",
        "/v1/circle/18446744073709551616",
        "/v1/circles/search/a?options.name=b",
        "/v1/circles/search/%GG",
        "/v1/circles/search/%FF",
    ] {
        assert_eq!(
            app.clone().oneshot(request(path)).await.unwrap().status(),
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    assert_eq!(
        app.clone()
            .oneshot(request("/v1/en/circles/recommended"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(tw.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn third_region_disabled_mode_and_openapi_describe_the_added_contract() {
    let tw = Mock::new();
    let kr = Mock::new();
    let stop = CancellationToken::new();
    let regions = RegionClients::new(
        Some(Region::Tw),
        vec![
            RegionBackend {
                region: Region::Tw,
                client: tw.clone(),
                managed: None,
            },
            RegionBackend {
                region: Region::Kr,
                client: kr.clone(),
                managed: None,
            },
        ],
        CacheOptions::default(),
        stop.clone(),
    )
    .unwrap();
    let app = router_with_regions(
        tw.clone(),
        KEY.to_owned().into(),
        RouterOptions {
            mode: ResponseMode::Public,
            managed: None,
            access_log: false,
        },
        CacheOptions::default(),
        stop,
        regions,
    )
    .unwrap();
    let response = app
        .oneshot(request("/v1/profile/40000000001"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-moenotes-region"], "kr");
    assert_eq!(kr.calls.lock().unwrap().len(), 1);
    assert!(tw.calls.lock().unwrap().is_empty());
    let (disabled, _, _) = self::app(ResponseMode::Disabled, true);
    for path in ["/v1/profile/20000000001", "/v1/en/music/69/ranking"] {
        assert_eq!(
            disabled
                .clone()
                .oneshot(request(path))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let doc = moenotes_server::openapi_document(ResponseMode::Public);
    let paths = doc["paths"].as_object().unwrap();
    let mut operations = std::collections::HashSet::new();
    for (path, route) in paths {
        if let Some(id) = route["get"]["operationId"].as_str() {
            assert!(operations.insert(id), "duplicate operationId: {id}");
        }
        for parameter in route["get"]["parameters"].as_array().into_iter().flatten() {
            if parameter["in"] == "path" {
                assert_eq!(parameter["required"], true);
                assert!(path.contains(&format!("{{{}}}", parameter["name"].as_str().unwrap())));
            }
        }
    }
    assert!(paths.contains_key("/v1/kr/profile/{profileId}"));
    assert!(!paths.contains_key("/v1/en/circles/recommended"));
    let parameters = doc["paths"]["/v1/event/{eventId}/ranking/{ranks}"]["get"]["parameters"]
        .as_array()
        .unwrap();
    let ranks = parameters.iter().find(|p| p["name"] == "ranks").unwrap();
    assert_eq!(ranks["style"], "simple");
    assert_eq!(ranks["explode"], false);
    assert_eq!(ranks["schema"]["maxItems"], 100);
}
