use super::*;
use crate::config::VersionSyncConfig;
use std::time::{SystemTime, UNIX_EPOCH};

impl ManagedClient {
    /// Start one bounded anonymous poller. Caller must only use this for `serve`.
    pub fn start_version_sync(
        self: &Arc<Self>,
        client: Arc<Client>,
        config: VersionSyncConfig,
    ) -> tokio::task::JoinHandle<()> {
        self.state.lock().unwrap().version_sync = Some(VersionSyncStatus {
            interval_seconds: config.interval_seconds,
            current: None,
            checks: 0,
            updates: 0,
            last_checked_at: None,
            last_error: None,
        });
        let managed = self.clone();
        tokio::spawn(async move {
            loop {
                if managed.stop.is_cancelled() {
                    break;
                }
                managed.sync_versions(&client).await;
                tokio::select! {
                    _ = managed.stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(config.interval_seconds)) => {}
                }
            }
        })
    }

    async fn sync_versions(&self, client: &Client) {
        {
            let mut state = self.state.lock().unwrap();
            if state.version_updating
                || matches!(state.phase, Phase::Recovering | Phase::PersistenceFailed)
            {
                return;
            }
            state.version_updating = true;
        }
        let generation = client.generation();
        let result = client
            .refresh_versions(generation, self.stop.child_token())
            .await;
        let mut state = self.state.lock().unwrap();
        state.version_updating = false;
        let changed = matches!(result, Ok(true));
        if changed {
            if state.attempted == Some(generation) {
                state.attempted = Some(client.generation());
            }
            if let Some((old, revision)) = state.initial_attempted
                && old == generation
            {
                state.initial_attempted = Some((client.generation(), revision));
            }
        }
        if changed && self.inner.query_error(false).is_none() {
            state.phase = Phase::Unverified;
            state.last_error = None;
        }
        let Some(status) = &mut state.version_sync else {
            return;
        };
        status.checks += 1;
        status.last_checked_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        status.last_error = result.as_ref().err().map(|e| e.kind);
        if result.is_ok() {
            let config = client.session_config();
            status.current = config.master_version.zip(config.resource_version).map(
                |(master_version, resource_version)| moenotes_client::DataVersions {
                    master_version,
                    resource_version,
                },
            );
        }
        if changed {
            status.updates += 1;
        }
        if changed || result.is_err() {
            eprintln!(
                "{}",
                serde_json::json!({"event":"version_sync","changed":changed,"current":status.current,"error":status.last_error})
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moenotes_client::{ClientOptions, Credentials, Method, SessionConfig, StaticCredentials};
    use prost_reflect::DynamicMessage;
    use tonic::metadata::MetadataMap;
    struct Mock {
        versions: Mutex<usize>,
        fail: Mutex<bool>,
    }
    #[async_trait]
    impl moenotes_client::transport::Transport for Mock {
        async fn call(
            &self,
            method: Method,
            _: DynamicMessage,
            _: MetadataMap,
            _: Duration,
        ) -> Result<DynamicMessage, ClientError> {
            let desc = moenotes_proto::pool()
                .get_message_by_name(method.output)
                .unwrap();
            if method.name == "version" {
                if *self.fail.lock().unwrap() {
                    return Err(ClientError::new(ErrorKind::Transport));
                }
                return Ok(DynamicMessage::deserialize(desc, serde_json::json!({"version":format!("master-{}",self.versions.lock().unwrap()),"resourceVersion":"1.0.0.105"})).unwrap());
            }
            let mut headers = MetadataMap::new();
            headers.insert(
                "x-sirius-error-code",
                "MASTER_VERSION_MISMATCH".parse().unwrap(),
            );
            Err(ClientError::from_metadata(
                tonic::Code::FailedPrecondition,
                &headers,
                &MetadataMap::new(),
            )
            .unwrap())
        }
    }
    fn setup() -> (Arc<Client>, Arc<ManagedClient>, Arc<Mock>) {
        let config = SessionConfig {
            region: "test".into(),
            origin: "https://game.invalid".into(),
            allowed_origins: vec!["https://game.invalid".into()],
            platform: "android".into(),
            client_version: "1".into(),
            master_version: Some("old".into()),
            resource_version: Some("old".into()),
        };
        let credentials = StaticCredentials::new(
            config.region.clone(),
            config.origin.clone(),
            Credentials {
                player_id: "synthetic-player".into(),
                credential: "synthetic-secret".into(),
                device_id: None,
                bid: None,
            },
        );
        let mock = Arc::new(Mock {
            versions: Mutex::new(1),
            fail: Mutex::new(false),
        });
        let client = Arc::new(
            Client::with_transport(
                config,
                Some(&credentials),
                ClientOptions {
                    minimum_interval: Duration::ZERO,
                    ..Default::default()
                },
                mock.clone(),
            )
            .unwrap(),
        );
        let managed = Arc::new(ManagedClient::new(
            client.clone(),
            None,
            Duration::from_secs(300),
            CancellationToken::new(),
        ));
        (client, managed, mock)
    }
    #[tokio::test(start_paused = true)]
    async fn startup_periodic_update_failure_and_shutdown() {
        let (client, managed, mock) = setup();
        let task = managed.start_version_sync(client.clone(), VersionSyncConfig::default());
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(managed.status().version_sync.as_ref().unwrap().checks, 1);
        assert_eq!(
            client.session_config().master_version.as_deref(),
            Some("master-1")
        );
        assert!(!managed.ready());
        let generation = client.generation();
        managed
            .execute(
                generation,
                Query::Whoami(Default::default()),
                CancellationToken::new(),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(managed.status().phase, Phase::VersionBlocked);
        // Authentication retry budgets are retained when only versions rotate.
        managed.state.lock().unwrap().attempted = Some(generation);
        *mock.versions.lock().unwrap() = 2;
        tokio::time::advance(Duration::from_secs(60)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            client.session_config().master_version.as_deref(),
            Some("master-2")
        );
        assert_eq!(managed.status().phase, Phase::Unverified);
        assert!(client.query_error(false).is_none());
        assert_eq!(
            managed.state.lock().unwrap().attempted,
            Some(client.generation())
        );
        *mock.fail.lock().unwrap() = true;
        tokio::time::advance(Duration::from_secs(60)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let status = managed.status().version_sync.unwrap();
        assert_eq!(status.checks, 3);
        assert_eq!(status.updates, 2);
        assert_eq!(status.last_error, Some(ErrorKind::Transport));
        assert_eq!(
            client.session_config().master_version.as_deref(),
            Some("master-2")
        );
        managed.stop.cancel();
        task.await.unwrap();
    }
    #[tokio::test]
    async fn never_races_account_recovery_or_clears_persistence_failure() {
        let (client, managed, _) = setup();
        let generation = client.generation();
        for phase in [Phase::Recovering, Phase::PersistenceFailed] {
            managed.state.lock().unwrap().phase = phase;
            managed.sync_versions(&client).await;
            assert_eq!(client.generation(), generation);
            assert_eq!(managed.status().phase, phase);
        }
    }
}
