use async_trait::async_trait;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use moenotes_client::{
    CancellationToken, Client, ClientError, ClientOptions, Credentials, ErrorKind, Generation,
    Method, Query, QueryClient, QueryResponse, SessionConfig, StaticCredentials,
    transport::Transport,
};
use moenotes_server::{
    RouterOptions,
    accounts::JpAccountDirectoryClient,
    cache::CacheOptions,
    config::Config,
    managed::{ManagedClient, Recovery},
    pool::{SessionMember, SessionPool},
    projection::ResponseMode,
    regions::{Region, RegionClients},
    runtime::Runtime,
};
use prost_reflect::DynamicMessage;
use std::{
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tonic::metadata::MetadataMap;
use tower::ServiceExt;

struct Fake {
    id: i64,
    generation: Mutex<Generation>,
    blocked: Mutex<Option<ErrorKind>>,
    fail: Mutex<Option<ErrorKind>>,
    calls: AtomicUsize,
    gate: Mutex<Option<Arc<tokio::sync::Barrier>>>,
}
impl Fake {
    fn new(id: i64) -> Arc<Self> {
        Arc::new(Self {
            id,
            generation: Mutex::new(Generation::new_v4()),
            blocked: Mutex::new(None),
            fail: Mutex::new(None),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }
    fn replace(&self) {
        *self.generation.lock().unwrap() = Generation::new_v4();
    }
}
fn query() -> Query {
    Query::FavoriteStatus(
        moenotes_client::generated::app::player::GetPlayerFavoriteStatusRequest {
            player_id: "synthetic-target".into(),
        },
    )
}
fn message(method: Method, id: i64) -> DynamicMessage {
    DynamicMessage::deserialize(
        moenotes_proto::pool()
            .get_message_by_name(method.output)
            .unwrap(),
        serde_json::json!({"totalFavorite":id.to_string(),"isSentFavorite":id % 2 == 0}),
    )
    .unwrap()
}
#[async_trait]
impl QueryClient for Fake {
    fn generation(&self) -> Generation {
        *self.generation.lock().unwrap()
    }
    fn query_error(&self, anonymous: bool) -> Option<ClientError> {
        if anonymous {
            None
        } else {
            self.blocked.lock().unwrap().map(ClientError::new)
        }
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        _: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.wait().await;
            gate.wait().await;
        }
        if let Some(kind) = *self.fail.lock().unwrap() {
            return Err(ClientError::new(kind));
        }
        Ok(QueryResponse {
            generation,
            message: message(*query.method(), self.id),
            fetched_at: SystemTime::now(),
        })
    }
}
fn pool(clients: &[Arc<Fake>], options: CacheOptions, stop: CancellationToken) -> Arc<SessionPool> {
    SessionPool::new(
        clients
            .iter()
            .map(|c| SessionMember {
                client: c.clone(),
                managed: None,
            })
            .collect(),
        options,
        stop,
    )
    .unwrap()
}
#[tokio::test]
async fn round_robin_keeps_cached_raw_responses_and_generations_separate() {
    let clients = [Fake::new(1), Fake::new(2), Fake::new(3)];
    let pool = pool(&clients, CacheOptions::default(), CancellationToken::new());
    for i in 0..9 {
        let (r, cache) = pool.query(query()).await.unwrap();
        assert_eq!(r.json["totalFavorite"], ((i % 3) + 1).to_string());
        assert_eq!(cache, if i < 3 { "MISS" } else { "HIT" });
    }
    clients[0].replace();
    assert_eq!(pool.query(query()).await.unwrap().1, "MISS");
    assert_eq!(pool.query(query()).await.unwrap().1, "HIT");
    assert_eq!(
        clients
            .iter()
            .map(|c| c.calls.load(Ordering::SeqCst))
            .collect::<Vec<_>>(),
        vec![2, 1, 1]
    );
}
#[tokio::test]
async fn concurrent_requests_are_evenly_dispatched_and_coalesced_per_member() {
    let clients: Vec<_> = (1..=10).map(Fake::new).collect();
    let pool = pool(&clients, CacheOptions::default(), CancellationToken::new());
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..100 {
        let p = pool.clone();
        tasks.spawn(async move { p.query(query()).await.unwrap().0.json.clone() });
    }
    let mut counts = [0; 10];
    while let Some(r) = tasks.join_next().await {
        let json = r.unwrap();
        let id: usize = json["totalFavorite"].as_str().unwrap().parse().unwrap();
        counts[id - 1] += 1;
    }
    assert_eq!(counts, [10; 10]);
    assert!(clients.iter().all(|c| c.calls.load(Ordering::SeqCst) == 1));
    assert!(
        pool.status()["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["requests"] == 10)
    );
}
#[tokio::test(start_paused = true)]
async fn failed_rpc_is_not_replayed_and_cooldown_expires_or_clears_on_reload() {
    let a = Fake::new(1);
    let b = Fake::new(2);
    *a.fail.lock().unwrap() = Some(ErrorKind::Transport);
    let p = pool(
        &[a.clone(), b.clone()],
        CacheOptions::default(),
        CancellationToken::new(),
    );
    assert_eq!(
        p.query(query()).await.err().unwrap().kind,
        ErrorKind::Transport
    );
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    for _ in 0..3 {
        assert_eq!(p.query(query()).await.unwrap().0.json["totalFavorite"], "2");
    }
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(6)).await;
    *a.fail.lock().unwrap() = None;
    assert_eq!(p.query(query()).await.unwrap().0.json["totalFavorite"], "1");
    a.replace();
    *a.fail.lock().unwrap() = Some(ErrorKind::Timeout);
    p.query(query()).await.unwrap();
    assert_eq!(
        p.query(query()).await.err().unwrap().kind,
        ErrorKind::Timeout
    );
    a.replace();
    *a.fail.lock().unwrap() = None;
    p.query(query()).await.unwrap();
    assert_eq!(p.query(query()).await.unwrap().0.json["totalFavorite"], "1");
}
#[tokio::test]
async fn blocked_empty_canceled_and_invalid_queries_never_dispatch() {
    let a = Fake::new(1);
    let b = Fake::new(2);
    *a.blocked.lock().unwrap() = Some(ErrorKind::Authentication);
    let stop = CancellationToken::new();
    let p = pool(
        &[a.clone(), b.clone()],
        CacheOptions::default(),
        stop.clone(),
    );
    assert_eq!(p.query(query()).await.unwrap().0.json["totalFavorite"], "2");
    *b.blocked.lock().unwrap() = Some(ErrorKind::DeviceConflict);
    assert!(p.query(query()).await.is_err());
    assert_eq!(a.calls.load(Ordering::SeqCst), 0);
    let empty = pool(&[], CacheOptions::default(), CancellationToken::new());
    assert_eq!(
        empty.query(query()).await.err().unwrap().kind,
        ErrorKind::AuthenticationRequired
    );
    assert!(!empty.ready());
    stop.cancel();
    assert_eq!(
        p.query(query()).await.err().unwrap().kind,
        ErrorKind::Cancelled
    );
    let p = pool(
        &[Fake::new(3)],
        CacheOptions::default(),
        CancellationToken::new(),
    );
    assert_eq!(
        p.query(Query::FavoriteStatus(Default::default()))
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::InvalidRequest
    );
    assert_eq!(p.status()["members"][0]["requests"], 0);
}
#[tokio::test]
async fn stale_inflight_response_is_rejected_without_affecting_other_session() {
    let a = Fake::new(1);
    let b = Fake::new(2);
    let gate = Arc::new(tokio::sync::Barrier::new(2));
    *a.gate.lock().unwrap() = Some(gate.clone());
    let p = pool(
        &[a.clone(), b],
        CacheOptions::default(),
        CancellationToken::new(),
    );
    let task = tokio::spawn({
        let p = p.clone();
        async move { p.query(query()).await }
    });
    gate.wait().await;
    a.replace();
    assert_eq!(p.query(query()).await.unwrap().0.json["totalFavorite"], "2");
    gate.wait().await;
    assert_eq!(
        task.await.unwrap().err().unwrap().kind,
        ErrorKind::SessionChanged
    );
}

#[tokio::test]
async fn readiness_accepts_one_good_session_and_diagnostics_do_not_advance_rotation() {
    let bad = Fake::new(1);
    *bad.blocked.lock().unwrap() = Some(ErrorKind::DeviceConflict);
    let good = Fake::new(2);
    let managed = Arc::new(ManagedClient::new(
        good,
        None,
        Duration::from_secs(60),
        CancellationToken::new(),
    ));
    let p = SessionPool::new(
        vec![
            SessionMember {
                client: bad,
                managed: None,
            },
            SessionMember {
                client: managed.clone(),
                managed: Some(managed),
            },
        ],
        CacheOptions::default(),
        CancellationToken::new(),
    )
    .unwrap();
    assert!(!p.ready());
    assert_eq!(p.status()["members"][1]["requests"], 0);
    assert_eq!(p.query(query()).await.unwrap().0.json["totalFavorite"], "2");
    assert!(p.ready());
    assert_eq!(p.status()["ready_sessions"], 1);
    assert_eq!(p.status()["members"][0]["requests"], 0);
    assert_eq!(p.status()["members"][1]["requests"], 1);
}

fn private(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
fn settings() -> &'static str {
    r#"api_key="synthetic-session-pool-key-at-least-32-chars"
[session]
region="jp"
origin="https://jp.invalid"
allowed_origins=["https://jp.invalid"]
platform="android"
client_version="1.0.3"
[version_sync]
enabled=false
[accounts]
directory="accounts/jp"
strategy="round_robin"
"#
}
#[tokio::test]
async fn directory_builds_independent_lazy_clients_and_reloads_only_target_session() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    private(&cfg, settings());
    moenotes_server::jp_operator::init_accounts(&dir.path().join("accounts")).unwrap();
    let config = Config::read(&cfg).unwrap();
    assert!(
        Runtime::for_accounts(config.clone(), CancellationToken::new())
            .unwrap()
            .is_empty()
    );
    for name in ["b.json", "a.json"] {
        private(
            &dir.path().join("accounts/jp").join(name),
            "not yet valid credentials",
        );
    }
    let runtimes = Runtime::for_accounts(config.clone(), CancellationToken::new()).unwrap();
    assert_eq!(runtimes.len(), 2);
    assert_eq!(
        runtimes[0]
            .config
            .accounts
            .as_ref()
            .unwrap()
            .selected
            .as_deref(),
        Some("a.json")
    );
    assert_ne!(
        runtimes[0].client.generation(),
        runtimes[1].client.generation()
    );
    for r in &runtimes {
        assert_eq!(
            r.client.session_status(),
            moenotes_client::SessionStatus::Anonymous
        );
    }
    let pool = SessionPool::new(
        runtimes
            .iter()
            .map(|r| SessionMember {
                client: r.managed.clone(),
                managed: Some(r.managed.clone()),
            })
            .collect(),
        CacheOptions::default(),
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(pool.status()["sessions"], 2);
    assert!(!pool.ready());
    assert!(
        runtimes
            .iter()
            .all(|r| r.managed.status().recovery_attempts == 0)
    );
    for (i, r) in runtimes.iter().enumerate() {
        let a = r.config.accounts.as_ref().unwrap();
        private(&a.directory.join(a.selected.as_ref().unwrap()),&serde_json::json!({"region":"jp","origin":"https://jp.invalid","credentials":{"player_id":format!("synthetic-{i}"),"credential":format!("synthetic-secret-{i}"),"device_id":null,"bid":null}}).to_string());
    }
    assert_eq!(
        pool.query(query()).await.err().unwrap().kind,
        ErrorKind::AuthenticationRequired
    );
    for r in &runtimes {
        r.managed.wait_idle().await;
        assert_eq!(
            r.client.session_status(),
            moenotes_client::SessionStatus::CredentialsUnverified
        );
    }
    let b = runtimes[1].client.generation();
    runtimes[0].reload().unwrap();
    assert_eq!(
        runtimes[0].client.session_status(),
        moenotes_client::SessionStatus::Anonymous
    );
    assert_eq!(runtimes[1].client.generation(), b);
    let loader = JpAccountDirectoryClient::new(
        runtimes[0].client.clone(),
        runtimes[0].config.accounts.clone().unwrap(),
        config.session,
    )
    .unwrap();
    loader
        .recover(runtimes[0].client.generation(), CancellationToken::new())
        .await
        .unwrap();
    private(&cfg, &format!("{}selected=\"a.json\"\n", settings()));
    assert!(Config::read(&cfg).is_err());
    private(&cfg, &settings().replace("round_robin", "unknown"));
    assert!(Config::read(&cfg).is_err());
    private(&cfg, settings());
    for n in 0..31 {
        private(&dir.path().join(format!("accounts/jp/{n}.json")), "{}");
    }
    assert!(Runtime::for_accounts(Config::read(&cfg).unwrap(), CancellationToken::new()).is_err());
}

struct HeaderPeer {
    calls: Mutex<Vec<(String, String)>>,
    barrier: tokio::sync::Barrier,
}
#[async_trait]
impl Transport for HeaderPeer {
    async fn call(
        &self,
        m: Method,
        _: DynamicMessage,
        h: MetadataMap,
        _: Duration,
    ) -> Result<DynamicMessage, ClientError> {
        let id = h.get("x-player-id").unwrap().to_str().unwrap().to_owned();
        let credential = h
            .get("x-player-credential")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        self.calls.lock().unwrap().push((id, credential));
        self.barrier.wait().await;
        Ok(message(m, 1))
    }
}
#[tokio::test]
async fn ten_real_clients_have_independent_serial_queues_and_auth_headers() {
    let peer = Arc::new(HeaderPeer {
        calls: Mutex::new(vec![]),
        barrier: tokio::sync::Barrier::new(10),
    });
    let mut members = vec![];
    for i in 0..10 {
        let config = SessionConfig {
            region: "jp".into(),
            origin: "https://jp.invalid".into(),
            allowed_origins: vec!["https://jp.invalid".into()],
            platform: "android".into(),
            client_version: "1.0.3".into(),
            master_version: None,
            resource_version: None,
        };
        let provider = StaticCredentials::new(
            config.region.clone(),
            config.origin.clone(),
            Credentials {
                player_id: format!("synthetic-player-{i}"),
                credential: format!("synthetic-secret-{i}"),
                device_id: None,
                bid: None,
            },
        );
        let client = Arc::new(
            Client::with_transport(
                config,
                Some(&provider),
                ClientOptions {
                    minimum_interval: Duration::ZERO,
                    ..Default::default()
                },
                peer.clone(),
            )
            .unwrap(),
        );
        members.push(SessionMember {
            client,
            managed: None,
        });
    }
    let p = SessionPool::new(members, CacheOptions::default(), CancellationToken::new()).unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let p = p.clone();
        tasks.spawn(async move { p.query(query()).await });
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(r) = tasks.join_next().await {
            assert!(r.unwrap().is_ok());
        }
    })
    .await
    .unwrap();
    let mut headers = peer.calls.lock().unwrap().clone();
    headers.sort();
    assert_eq!(
        headers,
        (0..10)
            .map(|i| (
                format!("synthetic-player-{i}"),
                format!("synthetic-secret-{i}")
            ))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn http_aliases_share_pool_cursor_and_region_isolation_with_public_projection() {
    let a = Fake::new(1);
    let b = Fake::new(2);
    let c = Fake::new(3);
    let jp = pool(
        &[a.clone(), b.clone()],
        CacheOptions::default(),
        CancellationToken::new(),
    );
    let en = pool(
        std::slice::from_ref(&c),
        CacheOptions::default(),
        CancellationToken::new(),
    );
    let regions = RegionClients::with_pools(
        Some(Region::Jp),
        vec![(Region::Jp, jp.clone()), (Region::En, en)],
    )
    .unwrap();
    let app = moenotes_server::router_with_regions(
        a.clone(),
        zeroize::Zeroizing::new("synthetic-http-pool-key-at-least-32-chars".into()),
        RouterOptions {
            mode: ResponseMode::Public,
            managed: None,
            access_log: false,
        },
        CacheOptions::default(),
        CancellationToken::new(),
        regions,
    )
    .unwrap();
    for (path, id) in [
        ("/v1/profile/favorites?playerId=synthetic-target", 1),
        ("/v1/jp/player/synthetic-target/favorites", 2),
        ("/v1/en/player/synthetic-target/favorites", 3),
        ("/v1/jp/profile/favorites?playerId=synthetic-target", 1),
    ] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(
                        "authorization",
                        "Bearer synthetic-http-pool-key-at-least-32-chars",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let json: serde_json::Value =
            serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(json["totalFavorite"], id.to_string());
        assert!(json.get("isSentFavorite").is_none());
    }
    assert_eq!(jp.status()["members"][0]["requests"], 2);
    assert_eq!(jp.status()["members"][1]["requests"], 1);
}
