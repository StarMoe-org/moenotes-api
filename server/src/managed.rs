//! Bounded opt-in game-session recovery. Failed queries are never replayed.
mod versions;
use crate::operator::LoginConfig;
use async_trait::async_trait;
use moenotes_client::{
    CancellationToken, Client, ClientError, ErrorKind, Generation, Query, QueryClient,
    QueryResponse, SessionConfig, SessionStatus,
};
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Unverified,
    Ready,
    Recovering,
    Recovered,
    ReauthenticationRequired,
    VersionBlocked,
    DeviceConflict,
    PersistenceFailed,
}
struct State {
    version_updating: bool,
    version_sync: Option<VersionSyncStatus>,
    phase: Phase,
    attempts: u64,
    successes: u64,
    attempted: Option<Generation>,
    initial_attempted: Option<(Generation, Option<[u8; 32]>)>,
    last_attempt: Option<Instant>,
    last_error: Option<ErrorKind>,
}
#[derive(Serialize)]
pub struct Status {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_sync: Option<VersionSyncStatus>,
    pub phase: Phase,
    pub recovery_attempts: u64,
    pub recovery_successes: u64,
    pub last_error: Option<ErrorKind>,
}
#[derive(Clone, Serialize)]
pub struct VersionSyncStatus {
    pub interval_seconds: u64,
    pub current: Option<moenotes_client::DataVersions>,
    pub checks: u64,
    pub updates: u64,
    pub last_checked_at: Option<u64>,
    pub last_error: Option<ErrorKind>,
}
#[async_trait]
pub trait Recovery: Send + Sync {
    /// Local input change detection, only evaluated after a protected query fails.
    fn input_revision(&self) -> Option<[u8; 32]> {
        None
    }
    async fn recover(
        &self,
        generation: Generation,
        cancel: CancellationToken,
    ) -> Result<(), ClientError>;
}
pub struct GameRecovery {
    pub client: Arc<Client>,
    pub login: LoginConfig,
    pub config: SessionConfig,
}
#[async_trait]
impl Recovery for GameRecovery {
    async fn recover(
        &self,
        generation: Generation,
        cancel: CancellationToken,
    ) -> Result<(), ClientError> {
        let _lock = self.login.lock()?;
        if self.client.generation() != generation {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        if matches!(
            self.client.session_status(),
            SessionStatus::VersionBlocked | SessionStatus::DeviceConflict
        ) {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        let saved = self
            .login
            .credentials(&self.config)?
            .ok_or_else(|| ClientError::new(ErrorKind::InvalidConfig))?;
        if !self.client.matches_identity(generation, &saved)? {
            return Err(ClientError::new(ErrorKind::InvalidConfig));
        }
        let sdk = self.login.approved_sdk(&self.config)?;
        let context = self.login.context()?;
        if !self
            .client
            .pre_login_with_sdk(generation, &sdk, &context, cancel.clone())
            .await?
        {
            return Err(ClientError::new(ErrorKind::Authentication));
        }
        self.client
            .login_with_sdk(generation, &sdk, &context, cancel.clone())
            .await?;
        if !self
            .client
            .matches_identity(self.client.generation(), &saved)?
        {
            return Err(ClientError::new(ErrorKind::InvalidConfig));
        }
        if cancel.is_cancelled() {
            return Err(ClientError::new(ErrorKind::Cancelled));
        }
        self.login.persist(&self.client, &self.config, &sdk)
    }
}

pub struct ManagedClient {
    inner: Arc<dyn QueryClient>,
    recovery: Option<Arc<dyn Recovery>>,
    initial_loader: Option<Arc<dyn Recovery>>,
    state: Arc<Mutex<State>>,
    cooldown: Duration,
    stop: CancellationToken,
}
impl ManagedClient {
    pub fn new(
        inner: Arc<dyn QueryClient>,
        recovery: Option<Arc<dyn Recovery>>,
        cooldown: Duration,
        stop: CancellationToken,
    ) -> Self {
        Self {
            inner,
            recovery,
            initial_loader: None,
            state: Arc::new(Mutex::new(State {
                version_updating: false,
                version_sync: None,
                phase: Phase::Unverified,
                attempts: 0,
                successes: 0,
                attempted: None,
                initial_attempted: None,
                last_attempt: None,
                last_error: None,
            })),
            cooldown,
            stop,
        }
    }
    pub fn with_initial_loader(mut self, loader: Arc<dyn Recovery>) -> Self {
        self.initial_loader = Some(loader);
        self
    }
    pub fn status(&self) -> Status {
        let s = self.state.lock().unwrap();
        Status {
            version_sync: s.version_sync.clone(),
            phase: s.phase,
            recovery_attempts: s.attempts,
            recovery_successes: s.successes,
            last_error: s.last_error,
        }
    }
    pub fn ready(&self) -> bool {
        matches!(self.status().phase, Phase::Ready | Phase::Recovered)
            && self.inner.query_error(false).is_none()
    }
    pub async fn wait_idle(&self) {
        while self.status().phase == Phase::Recovering {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    pub fn reload(
        &self,
        replace: impl FnOnce() -> Result<(), ClientError>,
    ) -> Result<(), ClientError> {
        let mut s = self.state.lock().unwrap();
        if s.phase == Phase::Recovering || s.version_updating {
            return Err(ClientError::new(ErrorKind::QueueFull));
        }
        replace()?;
        s.phase = Phase::Unverified;
        s.attempted = None;
        s.initial_attempted = None;
        s.last_attempt = None;
        s.last_error = None;
        Ok(())
    }
    fn observed(
        &self,
        generation: Generation,
        anonymous: bool,
        result: &Result<QueryResponse, ClientError>,
    ) {
        if anonymous || self.inner.generation() != generation || self.stop.is_cancelled() {
            return;
        }
        let mut s = self.state.lock().unwrap();
        if self.inner.generation() != generation {
            return;
        }
        if matches!(s.phase, Phase::Recovering | Phase::PersistenceFailed) {
            return;
        }
        let initial = result
            .as_ref()
            .is_err_and(|e| e.kind == ErrorKind::AuthenticationRequired);
        // Keep the last worker failure visible until inputs actually change.
        let initial_revision = if initial {
            self.initial_loader
                .as_ref()
                .map(|loader| (generation, loader.input_revision()))
        } else {
            None
        };
        if initial_revision.is_some() && initial_revision == s.initial_attempted {
            return;
        }
        match result {
            Ok(_) => {
                s.phase = Phase::Ready;
                s.last_error = None;
                return;
            }
            Err(e) => {
                s.last_error = Some(e.kind);
                match e.kind {
                    ErrorKind::Version => s.phase = Phase::VersionBlocked,
                    ErrorKind::DeviceConflict => s.phase = Phase::DeviceConflict,
                    ErrorKind::Authentication | ErrorKind::AuthenticationRequired => {
                        s.phase = Phase::ReauthenticationRequired
                    }
                    _ => {
                        s.phase = Phase::Unverified;
                        return;
                    }
                }
                if !matches!(
                    e.kind,
                    ErrorKind::Authentication | ErrorKind::AuthenticationRequired
                ) {
                    return;
                }
            }
        }
        if s.version_updating {
            return;
        }
        let Some(recovery) = (if initial {
            &self.initial_loader
        } else {
            &self.recovery
        })
        .clone() else {
            return;
        };
        if initial {
            s.initial_attempted = initial_revision;
        } else {
            if s.attempted == Some(generation)
                || s.last_attempt.is_some_and(|t| t.elapsed() < self.cooldown)
            {
                return;
            }
            s.attempted = Some(generation);
            s.last_attempt = Some(Instant::now());
        }
        s.phase = Phase::Recovering;
        s.attempts += 1;
        let state = self.state.clone();
        let inner = self.inner.clone();
        let stop = self.stop.child_token();
        tokio::spawn(async move {
            let result = tokio::select! { _=stop.cancelled()=>Err(ClientError::new(ErrorKind::Cancelled)),
            result=tokio::time::timeout(Duration::from_secs(120),recovery.recover(generation,stop.clone()))=>result.unwrap_or_else(|_|Err(ClientError::new(ErrorKind::Timeout))) };
            stop.cancel();
            let mut s = state.lock().unwrap();
            match result {
                Ok(()) => {
                    s.phase = if initial {
                        Phase::Unverified
                    } else {
                        Phase::Recovered
                    };
                    s.successes += 1;
                    s.last_error = None;
                }
                Err(e) => {
                    s.phase = if inner.generation() != generation {
                        Phase::PersistenceFailed
                    } else {
                        match e.kind {
                            ErrorKind::Version => Phase::VersionBlocked,
                            ErrorKind::DeviceConflict => Phase::DeviceConflict,
                            ErrorKind::InvalidConfig if !initial => Phase::PersistenceFailed,
                            _ => Phase::ReauthenticationRequired,
                        }
                    };
                    s.last_error = Some(e.kind);
                }
            }
            eprintln!(
                "{}",
                serde_json::json!({"event":if initial {"account_initialization"} else {"session_recovery"},"phase":s.phase,"attempts":s.attempts,"successes":s.successes,"error":s.last_error})
            );
        });
    }
}
#[async_trait]
impl QueryClient for ManagedClient {
    fn generation(&self) -> Generation {
        self.inner.generation()
    }
    fn query_error(&self, anonymous: bool) -> Option<ClientError> {
        if !anonymous {
            let phase = self.state.lock().unwrap().phase;
            if matches!(phase, Phase::Recovering | Phase::PersistenceFailed) {
                return Some(ClientError::new(ErrorKind::Authentication));
            }
        }
        let error = self.inner.query_error(anonymous);
        if let Some(error) = &error {
            self.observed(self.generation(), anonymous, &Err(error.clone()));
        }
        error
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        cancel: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        let anonymous = query.method().anonymous;
        if !anonymous
            && matches!(
                self.status().phase,
                Phase::Recovering | Phase::PersistenceFailed
            )
        {
            return Err(ClientError::new(ErrorKind::Authentication));
        }
        let result = self.inner.execute(generation, query, cancel).await;
        self.observed(generation, anonymous, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Initial {
        reads: AtomicUsize,
        attempts: AtomicUsize,
        revision: AtomicUsize,
        gate: tokio::sync::Notify,
    }
    #[async_trait]
    impl Recovery for Initial {
        fn input_revision(&self) -> Option<[u8; 32]> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Some([self.revision.load(Ordering::SeqCst) as u8; 32])
        }
        async fn recover(&self, _: Generation, _: CancellationToken) -> Result<(), ClientError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            self.gate.notified().await;
            Err(ClientError::new(ErrorKind::InvalidConfig))
        }
    }
    fn initial() -> Arc<Initial> {
        Arc::new(Initial {
            reads: AtomicUsize::new(0),
            attempts: AtomicUsize::new(0),
            revision: AtomicUsize::new(0),
            gate: tokio::sync::Notify::new(),
        })
    }
    #[tokio::test]
    async fn health_status_ready_and_anonymous_errors_never_read_accounts() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let loader = initial();
        let stop = CancellationToken::new();
        let managed = Arc::new(
            ManagedClient::new(
                Arc::new(Failing {
                    generation: Generation::new_v4(),
                    kind: ErrorKind::AuthenticationRequired,
                }),
                None,
                Duration::ZERO,
                stop.clone(),
            )
            .with_initial_loader(loader.clone()),
        );
        let app = crate::router_with_options(
            managed.clone(),
            zeroize::Zeroizing::new("synthetic-account-api-key-1234567890".into()),
            crate::RouterOptions {
                mode: crate::projection::ResponseMode::Public,
                managed: Some(managed.clone()),
                access_log: false,
            },
            Default::default(),
            stop.clone(),
        )
        .unwrap();
        for (path, status) in [
            ("/health", 200),
            ("/healthz", 200),
            ("/readyz", 503),
            ("/v1/status", 200),
            ("/v1/announcements", 503),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(
                            "authorization",
                            "Bearer synthetic-account-api-key-1234567890",
                        )
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status);
        }
        assert!(!managed.ready());
        assert_eq!(loader.reads.load(Ordering::SeqCst), 0);
        assert_eq!(loader.attempts.load(Ordering::SeqCst), 0);
        stop.cancel();
    }
    #[tokio::test]
    async fn lazy_load_is_single_flight_and_rearmed_only_by_input_change_or_reload() {
        let loader = initial();
        let managed = Arc::new(
            ManagedClient::new(
                Arc::new(Failing {
                    generation: Generation::new_v4(),
                    kind: ErrorKind::AuthenticationRequired,
                }),
                None,
                Duration::from_secs(300),
                CancellationToken::new(),
            )
            .with_initial_loader(loader.clone()),
        );
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let managed = managed.clone();
            tasks.push(tokio::spawn(async move {
                managed
                    .execute(
                        managed.generation(),
                        Query::Whoami(Default::default()),
                        CancellationToken::new(),
                    )
                    .await
            }));
        }
        for task in tasks {
            assert!(task.await.unwrap().is_err());
        }
        tokio::task::yield_now().await;
        assert_eq!(loader.attempts.load(Ordering::SeqCst), 1);
        assert!(managed.reload(|| Ok(())).is_err());
        loader.gate.notify_one();
        managed.wait_idle().await;
        assert_eq!(managed.status().phase, Phase::ReauthenticationRequired);
        for _ in 0..4 {
            assert!(managed.query_error(false).is_none());
            let _ = managed
                .execute(
                    managed.generation(),
                    Query::Whoami(Default::default()),
                    CancellationToken::new(),
                )
                .await;
        }
        assert_eq!(loader.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(managed.status().last_error, Some(ErrorKind::InvalidConfig));
        loader.revision.store(1, Ordering::SeqCst);
        for expected in [2, 3] {
            let _ = managed
                .execute(
                    managed.generation(),
                    Query::Whoami(Default::default()),
                    CancellationToken::new(),
                )
                .await;
            tokio::task::yield_now().await;
            assert_eq!(loader.attempts.load(Ordering::SeqCst), expected);
            loader.gate.notify_one();
            managed.wait_idle().await;
            managed.reload(|| Ok(())).unwrap();
        }
    }
    struct Rotating {
        generation: std::sync::RwLock<Generation>,
        valid: std::sync::atomic::AtomicBool,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl QueryClient for Rotating {
        fn generation(&self) -> Generation {
            *self.generation.read().unwrap()
        }
        async fn execute(
            &self,
            g: Generation,
            q: Query,
            _: CancellationToken,
        ) -> Result<QueryResponse, ClientError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if g != self.generation() {
                return Err(ClientError::new(ErrorKind::SessionChanged));
            }
            if !self.valid.load(Ordering::SeqCst) {
                return Err(ClientError::new(ErrorKind::Authentication));
            }
            Ok(QueryResponse {
                generation: g,
                fetched_at: std::time::SystemTime::now(),
                message: prost_reflect::DynamicMessage::new(
                    moenotes_proto::pool()
                        .get_message_by_name(q.method().output)
                        .unwrap(),
                ),
            })
        }
    }
    struct Rotate(Arc<Rotating>);
    struct RotateThenFail(Arc<Rotating>);
    #[async_trait]
    impl Recovery for RotateThenFail {
        async fn recover(&self, _: Generation, _: CancellationToken) -> Result<(), ClientError> {
            *self.0.generation.write().unwrap() = Generation::new_v4();
            self.0.valid.store(true, Ordering::SeqCst);
            Err(ClientError::new(ErrorKind::Cancelled))
        }
    }
    #[tokio::test]
    async fn rotated_but_unpersisted_session_is_blocked() {
        let inner = Arc::new(Rotating {
            generation: std::sync::RwLock::new(Generation::new_v4()),
            valid: std::sync::atomic::AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        let managed = ManagedClient::new(
            inner.clone(),
            Some(Arc::new(RotateThenFail(inner.clone()))),
            Duration::ZERO,
            CancellationToken::new(),
        );
        let _ = managed
            .execute(
                managed.generation(),
                Query::Whoami(Default::default()),
                CancellationToken::new(),
            )
            .await;
        managed.wait_idle().await;
        assert_eq!(managed.status().phase, Phase::PersistenceFailed);
        assert!(!managed.ready());
        assert!(managed.query_error(false).is_some());
        assert!(
            managed
                .execute(
                    managed.generation(),
                    Query::Whoami(Default::default()),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn cooldown_applies_to_new_generations() {
        let inner = Arc::new(Rotating {
            generation: std::sync::RwLock::new(Generation::new_v4()),
            valid: std::sync::atomic::AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        let attempt = Arc::new(Attempt(AtomicUsize::new(0)));
        let managed = ManagedClient::new(
            inner.clone(),
            Some(attempt.clone()),
            Duration::from_secs(300),
            CancellationToken::new(),
        );
        let _ = managed
            .execute(
                managed.generation(),
                Query::Whoami(Default::default()),
                CancellationToken::new(),
            )
            .await;
        managed.wait_idle().await;
        *inner.generation.write().unwrap() = Generation::new_v4();
        let _ = managed
            .execute(
                managed.generation(),
                Query::Whoami(Default::default()),
                CancellationToken::new(),
            )
            .await;
        managed.wait_idle().await;
        assert_eq!(attempt.0.load(Ordering::SeqCst), 1);
    }
    #[async_trait]
    impl Recovery for Rotate {
        async fn recover(&self, _: Generation, _: CancellationToken) -> Result<(), ClientError> {
            *self.0.generation.write().unwrap() = Generation::new_v4();
            self.0.valid.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    #[tokio::test]
    async fn recovered_generation_works_without_replaying_failed_request() {
        let inner = Arc::new(Rotating {
            generation: std::sync::RwLock::new(Generation::new_v4()),
            valid: std::sync::atomic::AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        let managed = ManagedClient::new(
            inner.clone(),
            Some(Arc::new(Rotate(inner.clone()))),
            Duration::ZERO,
            CancellationToken::new(),
        );
        let old = managed.generation();
        assert!(
            managed
                .execute(
                    old,
                    Query::Whoami(Default::default()),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        managed.wait_idle().await;
        assert_ne!(managed.generation(), old);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(managed.status().phase, Phase::Recovered);
        assert!(
            managed
                .execute(
                    managed.generation(),
                    Query::Whoami(Default::default()),
                    CancellationToken::new()
                )
                .await
                .is_ok()
        );
        assert_eq!(managed.status().phase, Phase::Ready);
    }
    struct Gated {
        count: AtomicUsize,
        gate: tokio::sync::Notify,
        result: ErrorKind,
    }
    #[async_trait]
    impl Recovery for Gated {
        async fn recover(&self, _: Generation, _: CancellationToken) -> Result<(), ClientError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.gate.notified().await;
            Err(ClientError::new(self.result))
        }
    }
    #[tokio::test]
    async fn concurrent_recovery_reload_persistence_failure_and_shutdown() {
        let gate = Arc::new(Gated {
            count: AtomicUsize::new(0),
            gate: tokio::sync::Notify::new(),
            result: ErrorKind::InvalidConfig,
        });
        let stop = CancellationToken::new();
        let client = Arc::new(ManagedClient::new(
            Arc::new(Failing {
                generation: Generation::new_v4(),
                kind: ErrorKind::Authentication,
            }),
            Some(gate.clone()),
            Duration::ZERO,
            stop.clone(),
        ));
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let c = client.clone();
            tasks.push(tokio::spawn(async move {
                c.execute(
                    c.generation(),
                    Query::Whoami(Default::default()),
                    CancellationToken::new(),
                )
                .await
            }));
        }
        for t in tasks {
            assert!(t.await.unwrap().is_err());
        }
        tokio::task::yield_now().await;
        assert_eq!(gate.count.load(Ordering::SeqCst), 1);
        assert_eq!(client.status().phase, Phase::Recovering);
        assert!(client.reload(|| Ok(())).is_err());
        gate.gate.notify_one();
        client.wait_idle().await;
        assert_eq!(client.status().phase, Phase::PersistenceFailed);
        assert!(client.query_error(false).is_some());
        client.reload(|| Ok(())).unwrap();
        assert_eq!(client.status().phase, Phase::Unverified);
        let _ = client
            .execute(
                client.generation(),
                Query::Whoami(Default::default()),
                CancellationToken::new(),
            )
            .await;
        stop.cancel();
        client.wait_idle().await;
        assert_eq!(client.status().phase, Phase::ReauthenticationRequired);
    }
    struct Failing {
        generation: Generation,
        kind: ErrorKind,
    }
    struct SessionMock(AtomicUsize);
    #[async_trait]
    impl moenotes_client::transport::Transport for SessionMock {
        async fn call(
            &self,
            method: moenotes_client::Method,
            _: prost_reflect::DynamicMessage,
            _: tonic::metadata::MetadataMap,
            _: Duration,
        ) -> Result<prost_reflect::DynamicMessage, ClientError> {
            let index = self.0.fetch_add(1, Ordering::SeqCst);
            if index == 0 {
                return Err(ClientError::new(ErrorKind::Authentication));
            }
            let value = match method.name {
                "player-pre-login" => serde_json::json!({"isAccountCreated":true}),
                "player-login" => {
                    serde_json::json!({"credential":{"id":"synthetic-player","credential":"new-secret"}})
                }
                _ => serde_json::json!({}),
            };
            Ok(prost_reflect::DynamicMessage::deserialize(
                moenotes_proto::pool()
                    .get_message_by_name(method.output)
                    .unwrap(),
                value,
            )
            .unwrap())
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn real_client_recovery_persists_and_invalidates_generation() {
        use moenotes_client::{
            ClientOptions, CredentialProvider, Credentials, StaticCredentials,
            auth::SdkAuthorization,
        };
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("context.json");
        std::fs::write(&path,br#"{"device_model":"synthetic","operating_system":"synthetic","device_identifier":"synthetic","global_channel_id":1,"brand_id":1,"area_id":1}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let config = SessionConfig {
            region: "test".into(),
            origin: "https://game.invalid".into(),
            allowed_origins: vec!["https://game.invalid".into()],
            platform: "android".into(),
            client_version: "1".into(),
            master_version: None,
            resource_version: None,
        };
        let provider = StaticCredentials::new(
            config.region.clone(),
            config.origin.clone(),
            Credentials {
                player_id: "synthetic-player".into(),
                credential: "old-secret".into(),
                device_id: None,
                bid: Some("synthetic-sdk".into()),
            },
        );
        let transport = Arc::new(SessionMock(AtomicUsize::new(0)));
        let client = Arc::new(
            Client::with_transport(
                config.clone(),
                Some(&provider),
                ClientOptions {
                    minimum_interval: Duration::ZERO,
                    ..Default::default()
                },
                transport.clone(),
            )
            .unwrap(),
        );
        let login = LoginConfig {
            context_file: path,
            context: None,
            sdk_http_file: None,
            sdk_http: None,
            state_dir: dir.path().into(),
        };
        let sdk = SdkAuthorization::from_callback_json(
            &config,
            br#"{"uid":"synthetic-sdk","accessToken":"token"}"#,
        )
        .unwrap();
        login.persist(&client, &config, &sdk).unwrap();
        let managed = Arc::new(ManagedClient::new(
            client.clone(),
            Some(Arc::new(GameRecovery {
                client: client.clone(),
                login: login.clone(),
                config: config.clone(),
            })),
            Duration::ZERO,
            CancellationToken::new(),
        ));
        let cache = crate::cache::QueryCache::new(
            managed.clone(),
            Default::default(),
            CancellationToken::new(),
        );
        let old = client.generation();
        assert!(
            cache
                .query(Query::Whoami(Default::default()))
                .await
                .is_err()
        );
        managed.wait_idle().await;
        assert_ne!(client.generation(), old);
        assert!(managed.ready());
        assert_eq!(transport.0.load(Ordering::SeqCst), 3);
        let saved = login
            .credentials(&config)
            .unwrap()
            .unwrap()
            .credentials(&config)
            .unwrap()
            .unwrap();
        assert_eq!(saved.credential, "new-secret");
        cache
            .query(Query::Whoami(Default::default()))
            .await
            .unwrap();
        assert_eq!(transport.0.load(Ordering::SeqCst), 4);
    }
    #[async_trait]
    impl QueryClient for Failing {
        fn generation(&self) -> Generation {
            self.generation
        }
        async fn execute(
            &self,
            _: Generation,
            _: Query,
            _: CancellationToken,
        ) -> Result<QueryResponse, ClientError> {
            Err(ClientError::new(self.kind))
        }
    }
    struct Attempt(AtomicUsize);
    #[async_trait]
    impl Recovery for Attempt {
        async fn recover(&self, _: Generation, _: CancellationToken) -> Result<(), ClientError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(ClientError::new(ErrorKind::Authentication))
        }
    }
    #[tokio::test]
    async fn recovery_is_single_attempt_and_never_replays_query() {
        let attempt = Arc::new(Attempt(AtomicUsize::new(0)));
        let client = ManagedClient::new(
            Arc::new(Failing {
                generation: Generation::new_v4(),
                kind: ErrorKind::Authentication,
            }),
            Some(attempt.clone()),
            Duration::ZERO,
            CancellationToken::new(),
        );
        for _ in 0..8 {
            assert!(
                client
                    .execute(
                        client.generation(),
                        Query::Whoami(Default::default()),
                        CancellationToken::new()
                    )
                    .await
                    .is_err()
            );
            tokio::task::yield_now().await;
        }
        assert_eq!(attempt.0.load(Ordering::SeqCst), 1);
        assert_eq!(client.status().phase, Phase::ReauthenticationRequired);
    }
    #[tokio::test]
    async fn non_auth_errors_never_login() {
        for kind in [
            ErrorKind::Maintenance,
            ErrorKind::Version,
            ErrorKind::DeviceConflict,
            ErrorKind::Transport,
            ErrorKind::AuthenticationRequired,
        ] {
            let attempt = Arc::new(Attempt(AtomicUsize::new(0)));
            let client = ManagedClient::new(
                Arc::new(Failing {
                    generation: Generation::new_v4(),
                    kind,
                }),
                Some(attempt.clone()),
                Duration::ZERO,
                CancellationToken::new(),
            );
            let _ = client
                .execute(
                    client.generation(),
                    Query::Whoami(Default::default()),
                    CancellationToken::new(),
                )
                .await;
            tokio::task::yield_now().await;
            assert_eq!(attempt.0.load(Ordering::SeqCst), 0);
        }
    }
}
