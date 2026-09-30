use super::*;
use crate::{
    cache::CacheOptions,
    projection::ResponseMode,
    regions::{RegionBackend, RegionClients},
};
use axum::{body::Body, http::Request as HttpRequest};
use http_body_util::BodyExt;
use moenotes_client::{Generation, QueryClient, QueryResponse};
use std::{
    sync::{
        RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};
use tower::ServiceExt;

const KEY: &str = "synthetic-profile-image-key-0123456789";
const ID: i64 = 50000000001;
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==";
fn image() -> Bytes {
    Bytes::from(STANDARD.decode(PNG).unwrap())
}
fn url(page: u8) -> String {
    format!("{CDN_ORIGIN}/operation/profilecard/{ID}/{ID}_1_{page}_synthetic")
}

struct Source {
    calls: AtomicUsize,
    body: Bytes,
}
#[async_trait]
impl ImageSource for Source {
    async fn fetch(&self, _: &str) -> Result<Bytes, ClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(self.body.clone())
    }
}
struct Profile {
    region: Region,
    generation: Generation,
    json: RwLock<serde_json::Value>,
    calls: AtomicUsize,
}
#[async_trait]
impl QueryClient for Profile {
    fn generation(&self) -> Generation {
        self.generation
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        _: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        assert_eq!(query.method().name, "profile");
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(QueryResponse {
            generation,
            fetched_at: SystemTime::now(),
            message: DynamicMessage::deserialize(
                moenotes_proto::pool_for_region(self.region.as_str())
                    .get_message_by_name(query.method().output)
                    .unwrap(),
                self.json.read().unwrap().clone(),
            )
            .unwrap(),
        })
    }
}
fn app(profile: Arc<Profile>, images: Arc<ProfileImages>, mode: ResponseMode) -> Router {
    let stop = CancellationToken::new();
    let options = CacheOptions {
        ttl: Duration::ZERO,
        ..Default::default()
    };
    let mut regions = RegionClients::new(
        Some(Region::Jp),
        vec![RegionBackend {
            region: Region::Jp,
            client: profile.clone(),
            managed: None,
        }],
        options.clone(),
        stop.clone(),
    )
    .unwrap();
    regions.set_profile_images(Region::Jp, images);
    crate::router_with_regions(
        profile,
        Zeroizing::new(KEY.into()),
        crate::RouterOptions {
            mode,
            managed: None,
            access_log: false,
        },
        options,
        stop,
        regions,
    )
    .unwrap()
}
fn profile() -> Arc<Profile> {
    Arc::new(Profile {
        region: Region::Jp,
        generation: Generation::new_v4(),
        json: RwLock::new(
            serde_json::json!({"playerProfile":{"profileId":ID.to_string(),"profileCard":{"thumbnailUrl":[url(1),"",url(3)]}}}),
        ),
        calls: AtomicUsize::new(0),
    })
}
fn request(path: &str, method: &str, auth: bool, body: &str) -> HttpRequest<Body> {
    let mut builder = HttpRequest::builder().uri(path).method(method);
    if auth {
        builder = builder.header("authorization", format!("Bearer {KEY}"));
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

#[test]
fn urls_are_exact_profile_bound_and_png_is_bounded() {
    assert!(validate_url(&url(1), ID, Region::Jp).is_ok());
    for bad in [
        url(1).replace("https:", "http:"),
        url(1).replace("static.", "evil."),
        url(1).replace(&format!("/{ID}/"), "/50000000002/"),
        format!("{}?x=1", url(1)),
        format!("{}#x", url(1)),
        url(1).replace("_synthetic", "/../x"),
        url(1).replace("_synthetic", "%2fsecret"),
        url(1).replace("_synthetic", "\\secret"),
        format!("{CDN_ORIGIN}/operation/profilecard/{ID}/bad"),
        url(1).replace(".jp/", ".jp.evil/"),
        url(1).replace(".jp/", ".jp@evil/"),
    ] {
        assert!(validate_url(&bad, ID, Region::Jp).is_err(), "{bad}");
    }
    assert!(png(&image()));
    assert!(!png(b"<html>not an image</html>"));
    let mut bytes = image().to_vec();
    bytes[16..20].copy_from_slice(&9000u32.to_be_bytes());
    assert!(!png(&bytes));
    assert!(!png(&image()[..40]));
}

#[tokio::test]
async fn auth_pages_changed_urls_disabled_and_errors() {
    let profile = profile();
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        body: image(),
    });
    let images = ProfileImages::with_source(source.clone(), CancellationToken::new());
    let app = app(profile.clone(), images.clone(), ResponseMode::Public);
    let path = format!("/v1/jp/profile/{ID}/card/1");
    for (method, auth, body, suffix, expected) in [
        ("GET", false, "", "", 401),
        ("HEAD", true, "", "", 405),
        ("POST", true, "", "", 405),
        ("GET", true, "x", "", 400),
        ("GET", true, "", "?url=https://evil", 400),
    ] {
        let r = app
            .clone()
            .oneshot(request(&(path.clone() + suffix), method, auth, body))
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), expected);
    }
    assert_eq!(profile.calls.load(Ordering::SeqCst), 0);
    for bad in [
        "0/card/1",
        "-1/card/1",
        "50000000001/card/0",
        "50000000001/card/99999999999999999999",
        "1%2f2/card/1",
    ] {
        let r = app
            .clone()
            .oneshot(request(&format!("/v1/jp/profile/{bad}"), "GET", true, ""))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }
    for status in ["MISS", "HIT"] {
        let r = app
            .clone()
            .oneshot(request(&path, "GET", true, ""))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["content-type"], "image/png");
        assert_eq!(r.headers()["x-content-type-options"], "nosniff");
        assert_eq!(r.headers()["cache-control"], "no-store");
        assert_eq!(r.headers()["x-moenotes-cache"], status);
        assert_eq!(
            r.headers()["x-moenotes-card-file"],
            format!("{ID}_1_1_synthetic")
        );
        assert!(!r.headers().contains_key("authorization"));
        assert_eq!(r.into_body().collect().await.unwrap().to_bytes(), image());
    }
    for page in [2, 4] {
        let r = app
            .clone()
            .oneshot(request(
                &format!("/v1/jp/profile/{ID}/card/{page}"),
                "GET",
                true,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
    // Existing URL bytes are cached, but changing the profile's URL must fetch a new image.
    profile.json.write().unwrap()["playerProfile"]["profileCard"]["thumbnailUrl"][0] =
        serde_json::json!(url(9));
    assert_eq!(
        app.clone()
            .oneshot(request(&path, "GET", true, ""))
            .await
            .unwrap()
            .headers()["x-moenotes-cache"],
        "MISS"
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    profile.json.write().unwrap()["playerProfile"]["profileCard"]["thumbnailUrl"][0] =
        serde_json::json!("https://evil.invalid/image");
    let r = app
        .clone()
        .oneshot(request(&path, "GET", true, ""))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    let disabled = self::app(profile, images, ResponseMode::Disabled);
    assert_eq!(
        disabled
            .oneshot(request(&path, "GET", true, ""))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn coalescing_expiry_and_bad_images_are_not_cached() {
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        body: image(),
    });
    let images = ProfileImages::with_source(source.clone(), CancellationToken::new());
    let (one, two) = tokio::join!(images.get("one"), images.get("one"));
    assert!(one.is_ok() && two.is_ok());
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    images
        .cache
        .lock()
        .await
        .entries
        .get_mut("one")
        .unwrap()
        .expires = Instant::now() - Duration::from_secs(1);
    images.get("one").await.unwrap();
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        body: Bytes::from_static(b"private upstream error"),
    });
    let images = ProfileImages::with_source(source.clone(), CancellationToken::new());
    for _ in 0..2 {
        assert_eq!(
            images.get("one").await.unwrap_err().kind,
            ErrorKind::Protocol
        );
    }
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    assert!(images.cache.lock().await.entries.is_empty());
}

#[tokio::test]
async fn cdn_metadata_validation_trailer_precedence_and_secret_hygiene() {
    let mut initial = MetadataMap::new();
    initial.insert("x-sirius-env", CDN_ORIGIN.parse().unwrap());
    initial.insert("x-sirius-cred", "old-synthetic-secret".parse().unwrap());
    let mut trailing = MetadataMap::new();
    trailing.insert("x-sirius-cred", "new-synthetic-secret".parse().unwrap());
    let c = credential_from_metadata(&initial, &trailing, "1.0.4").unwrap();
    assert_eq!(
        &*c.authorization,
        &format!("Basic {}", STANDARD.encode("sirius:new-synthetic-secret"))
    );
    trailing.insert("x-sirius-env", "https://evil.invalid".parse().unwrap());
    let e = credential_from_metadata(&initial, &trailing, "1.0.4")
        .err()
        .unwrap();
    assert_eq!(e.kind, ErrorKind::Protocol);
    assert!(!format!("{e:?}").contains("secret"));
    trailing.clear();
    initial.remove("x-sirius-cred");
    assert!(credential_from_metadata(&initial, &trailing, "1.0.4").is_err());
}

#[derive(Clone, Default)]
struct Wire {
    versions: Arc<AtomicUsize>,
    gets: Arc<AtomicUsize>,
    mode: Arc<AtomicUsize>,
}
struct VersionRpc(Wire);
impl tonic::server::UnaryService<DynamicMessage> for VersionRpc {
    type Response = DynamicMessage;
    type Future = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<tonic::Response<DynamicMessage>, tonic::Status>>
                + Send,
        >,
    >;
    fn call(&mut self, request: tonic::Request<DynamicMessage>) -> Self::Future {
        let wire = self.0.clone();
        Box::pin(async move {
            assert_eq!(request.metadata().get("x-platform").unwrap(), "android");
            assert_eq!(request.metadata().get("x-client-version").unwrap(), "1.0.4");
            for name in [
                "authorization",
                "x-player-id",
                "x-player-credential",
                "x-device-id",
            ] {
                assert!(request.metadata().get(name).is_none());
            }
            let number = wire.versions.fetch_add(1, Ordering::SeqCst);
            let message = DynamicMessage::new(
                moenotes_proto::pool_for_region("jp")
                    .get_message_by_name("app.masterdata.VersionResponse")
                    .unwrap(),
            );
            let mut response = tonic::Response::new(message);
            response
                .metadata_mut()
                .insert("x-sirius-env", CDN_ORIGIN.parse().unwrap());
            response.metadata_mut().insert(
                "x-sirius-cred",
                if number == 0 {
                    "synthetic-old"
                } else {
                    "synthetic-new"
                }
                .parse()
                .unwrap(),
            );
            Ok(response)
        })
    }
}
async fn wire_handler(State(wire): State<Wire>, request: axum::extract::Request) -> Response {
    if request.uri().path() == "/app.masterdata.MasterdataService/Version" {
        return tonic::server::Grpc::new(DynamicCodec(
            moenotes_proto::pool_for_region("jp")
                .get_message_by_name("app.masterdata.VersionRequest")
                .unwrap(),
        ))
        .unary(VersionRpc(wire), request)
        .await
        .map(Body::new);
    }
    assert_eq!(request.headers()["user-agent"], "OurNotes/1.0.4");
    assert!(request.headers().get("x-player-credential").is_none());
    let calls = wire.gets.fetch_add(1, Ordering::SeqCst);
    let expected = if calls == 0 {
        "synthetic-old"
    } else {
        "synthetic-new"
    };
    assert_eq!(
        request.headers()["authorization"],
        format!("Basic {}", STANDARD.encode(format!("sirius:{expected}")))
    );
    match wire.mode.load(Ordering::SeqCst) {
        1 => (StatusCode::FOUND, [("location", "/redirect-target")]).into_response(),
        2 => (
            StatusCode::OK,
            [("content-type", "image/png"), ("content-length", "8388609")],
            Body::from(vec![0u8; MAX_IMAGE + 1]),
        )
            .into_response(),
        3 => (
            StatusCode::OK,
            [("content-type", "text/html")],
            "private upstream text",
        )
            .into_response(),
        4 => StatusCode::UNAUTHORIZED.into_response(),
        _ if calls == 0 => StatusCode::UNAUTHORIZED.into_response(),
        _ => ([("content-type", "image/png")], image()).into_response(),
    }
}
fn jp_client() -> Arc<Client> {
    Arc::new(
        Client::new(
            moenotes_client::SessionConfig {
                region: "jp".into(),
                origin: API_ORIGIN.into(),
                allowed_origins: vec![API_ORIGIN.into()],
                platform: "android".into(),
                client_version: "1.0.4".into(),
                master_version: None,
                resource_version: None,
            },
            None,
            Default::default(),
        )
        .unwrap(),
    )
}
#[tokio::test]
async fn wire_version_auth_refresh_redirects_limits_and_no_player_secrets() {
    let wire = Wire::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .fallback(wire_handler)
                .with_state(wire.clone()),
        )
        .into_future(),
    );
    let mut source = LiveSource::new(jp_client()).unwrap();
    source.channel = Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    // Only tests permit loopback HTTP; production uses https_only and fixed origins.
    source.http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let url = format!("http://{address}/image");
    assert_eq!(source.fetch(&url).await.unwrap(), image());
    assert_eq!(wire.versions.load(Ordering::SeqCst), 2);
    assert_eq!(wire.gets.load(Ordering::SeqCst), 2);
    assert_eq!(source.fetch(&url).await.unwrap(), image());
    assert_eq!(wire.versions.load(Ordering::SeqCst), 2);
    for mode in [1, 2, 3] {
        wire.mode.store(mode, Ordering::SeqCst);
        let before = wire.gets.load(Ordering::SeqCst);
        assert!(source.fetch(&url).await.is_err());
        assert_eq!(
            wire.gets.load(Ordering::SeqCst),
            before + 1,
            "redirect/oversize/bad mime must not retry"
        );
    }
    wire.mode.store(4, Ordering::SeqCst);
    let before = wire.gets.load(Ordering::SeqCst);
    let versions = wire.versions.load(Ordering::SeqCst);
    assert!(source.fetch(&url).await.is_err());
    assert_eq!(
        wire.gets.load(Ordering::SeqCst),
        before + 2,
        "at most one CDN retry"
    );
    assert_eq!(wire.versions.load(Ordering::SeqCst), versions + 1);
    server.abort();
}

#[tokio::test]
async fn credential_expiry_and_concurrent_rejection_share_refresh() {
    let wire = Wire::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .fallback(wire_handler)
                .with_state(wire.clone()),
        )
        .into_future(),
    );
    let mut source = LiveSource::new(jp_client()).unwrap();
    source.channel = Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let old = source.credential(None).await.unwrap();
    let (a, b) = tokio::join!(source.credential(Some(&old)), source.credential(Some(&old)));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(wire.versions.load(Ordering::SeqCst), 2);
    drop(a);
    drop(b);
    {
        let mut state = source.auth.lock().await;
        Arc::get_mut(state.credential.as_mut().unwrap())
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
    }
    source.credential(None).await.unwrap();
    assert_eq!(wire.versions.load(Ordering::SeqCst), 3);
    server.abort();
}

fn regional_url(region: Region, id: i64) -> String {
    let hash = "a".repeat(64);
    let host = match region {
        Region::Tw => "l14-prod-hk-patch-sirius.gamerfusiontech.com",
        Region::En => "l14-prod-en-patch-sirius.bilibiligame.net",
        Region::Kr => "l14-prod-kr-patch-sirius.bilibiligame.net",
        Region::Jp => return format!("{CDN_ORIGIN}/operation/profilecard/{id}/{id}_1_1_{hash}"),
    };
    format!(
        "https://{host}/prod/{}_0123456789abcdef0123456789abcdef/operation/profilecard/{id}/{id}_1_1_{hash}",
        region.as_str()
    )
}

#[test]
fn international_urls_are_region_and_player_bound_before_download() {
    for (region, id) in [
        (Region::Tw, 20000000001),
        (Region::En, 30000000001),
        (Region::Kr, 40000000001),
    ] {
        let good = regional_url(region, id);
        assert!(validate_url(&good, id, region).is_ok());
        assert!(validate_url(&good, id + 1, region).is_err());
        assert!(validate_url(&good, id, Region::Jp).is_err());
        let host = url::Url::parse(&good)
            .unwrap()
            .host_str()
            .unwrap()
            .to_owned();
        for bad in [
            good.replace("https:", "http:"),
            good.replace(&host, &format!("{host}.invalid")),
            good.replace(&host, &format!("{host}@evil.invalid")),
            good.replace(&host, &format!("{host}:443")),
            good.replace("/prod/", "/x/../prod/"),
            good.replace("/prod/", "/%70rod/"),
            good.replace("/prod/", "/prod\\"),
            good.replace("0123456789abcdef0123456789abcdef", "short"),
            good.replace(&"a".repeat(64), "/../other"),
            format!("{good}?key=unexpected"),
            format!("{good}#fragment"),
        ] {
            assert!(validate_url(&bad, id, region).is_err(), "{bad}");
        }
    }
    assert!(
        validate_url(
            &regional_url(Region::Tw, 20000000001),
            20000000001,
            Region::En
        )
        .is_err()
    );
}

#[tokio::test]
async fn four_regions_keep_profiles_images_and_response_headers_isolated() {
    let stop = CancellationToken::new();
    let options = CacheOptions {
        ttl: Duration::ZERO,
        ..Default::default()
    };
    let entries = [
        (Region::Tw, 20000000001),
        (Region::En, 30000000001),
        (Region::Kr, 40000000001),
        (Region::Jp, ID),
    ];
    let profiles: Vec<_> = entries.iter().map(|&(region, id)| Arc::new(Profile {
        region,
        generation: Generation::new_v4(),
        json: RwLock::new(serde_json::json!({"playerProfile":{"profileId":id.to_string(),"profileCard":{"thumbnailUrl":[regional_url(region, id), ""]}}})),
        calls: AtomicUsize::new(0),
    })).collect();
    let mut regions = RegionClients::new(
        Some(Region::Tw),
        profiles
            .iter()
            .map(|profile| RegionBackend {
                region: profile.region,
                client: profile.clone(),
                managed: None,
            })
            .collect(),
        options.clone(),
        stop.clone(),
    )
    .unwrap();
    let mut sources = Vec::new();
    for &(region, _) in &entries {
        let source = Arc::new(Source {
            calls: AtomicUsize::new(0),
            body: image(),
        });
        regions.set_profile_images(
            region,
            ProfileImages::with_source(source.clone(), stop.clone()),
        );
        sources.push(source);
    }
    let app = crate::router_with_regions(
        profiles[0].clone(),
        Zeroizing::new(KEY.into()),
        crate::RouterOptions {
            mode: ResponseMode::Public,
            managed: None,
            access_log: false,
        },
        options,
        stop,
        regions,
    )
    .unwrap();
    for (index, &(region, id)) in entries.iter().enumerate() {
        let path = format!("/v1/{}/profile/{id}/card/1", region.as_str());
        for status in ["MISS", "HIT"] {
            let response = app
                .clone()
                .oneshot(request(&path, "GET", true, ""))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["x-moenotes-region"], region.as_str());
            assert_eq!(response.headers()["x-moenotes-cache"], status);
            assert_eq!(
                response.headers()["x-moenotes-card-file"],
                format!("{id}_1_1_{}", "a".repeat(64))
            );
            assert_eq!(
                response.into_body().collect().await.unwrap().to_bytes(),
                image()
            );
        }
        assert_eq!(sources[index].calls.load(Ordering::SeqCst), 1);
        for later in sources.iter().skip(index + 1) {
            assert_eq!(later.calls.load(Ordering::SeqCst), 0);
        }
        let response = app
            .clone()
            .oneshot(request(
                &path.replace("/card/1", "/card/2"),
                "GET",
                true,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    for path in [
        "/v1/tw/profile/30000000001/card/1",
        "/v1/en/profile/40000000001/card/1",
        "/v1/kr/profile/20000000001/card/1",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(path, "GET", true, ""))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        app.oneshot(request(
            "/v1/tw/profile/20000000001/card/1",
            "GET",
            false,
            ""
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn international_downloads_have_no_credentials_or_redirect_retries() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(move |request: axum::extract::Request| {
                let calls = handler_calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    for header in [
                        "authorization",
                        "cookie",
                        "x-player-id",
                        "x-player-credential",
                        "x-device-id",
                    ] {
                        assert!(request.headers().get(header).is_none());
                    }
                    match request.uri().path() {
                        "/image" => ([("content-type", "image/png")], image()).into_response(),
                        "/redirect" => {
                            (StatusCode::FOUND, [("location", "/image")]).into_response()
                        }
                        "/large" => ([("content-type", "image/png")], vec![0u8; MAX_IMAGE + 1])
                            .into_response(),
                        "/bad" => ([("content-type", "text/html")], "private upstream error")
                            .into_response(),
                        _ => StatusCode::UNAUTHORIZED.into_response(),
                    }
                }
            }),
        )
        .into_future(),
    );
    let source = DirectSource {
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    };
    assert_eq!(
        source
            .fetch(&format!("http://{address}/image"))
            .await
            .unwrap(),
        image()
    );
    for path in ["redirect", "large", "bad", "unauthorized"] {
        let before = calls.load(Ordering::SeqCst);
        assert!(
            source
                .fetch(&format!("http://{address}/{path}"))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), before + 1);
    }
    server.abort();
}
