use super::*;
use crate::config::VersionSyncConfig;
use std::time::{SystemTime, UNIX_EPOCH};

/// Patch releases tried per check once the game refuses the client version.
const CLIENT_CANDIDATES: u32 = 3;

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
            client_version: client.session_config().client_version,
            follow_client_updates: config.follow_client_updates,
            checks: 0,
            updates: 0,
            client_updates: 0,
            last_checked_at: None,
            last_error: None,
        });
        let managed = self.clone();
        tokio::spawn(async move {
            loop {
                if managed.stop.is_cancelled() {
                    break;
                }
                managed
                    .sync_versions(&client, config.follow_client_updates)
                    .await;
                tokio::select! {
                    _ = managed.stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(config.interval_seconds)) => {}
                }
            }
        })
    }

    async fn sync_versions(&self, client: &Client, follow_client_updates: bool) {
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
        let mut result = client
            .refresh_versions(generation, self.stop.child_token())
            .await;
        // The game refuses the client version after a client release. Try the next
        // patch releases in order; one that is refused as well moves on, anything else
        // (e.g. maintenance while the release rolls out) waits for the next check.
        let mut followed = None;
        if follow_client_updates
            && matches!(&result, Err(error) if error.kind == ErrorKind::Version)
        {
            let previous = client.session_config().client_version;
            for candidate in moenotes_client::patch_successors(&previous, CLIENT_CANDIDATES) {
                match client
                    .adopt_client_version(generation, &candidate, self.stop.child_token())
                    .await
                {
                    Err(error) if error.kind == ErrorKind::Version => continue,
                    outcome => {
                        if outcome.is_ok() {
                            followed = Some((previous.clone(), candidate));
                        }
                        result = outcome;
                        break;
                    }
                }
            }
        }
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
        status.client_version = client.session_config().client_version;
        if let Some((from, to)) = &followed {
            status.client_updates += 1;
            eprintln!(
                "{}",
                serde_json::json!({"event":"client_version_update","from":from,"to":to})
            );
        }
        if result.is_ok() {
            let config = client.session_config();
            status.current =
                config
                    .master_version
                    .map(|master_version| moenotes_client::DataVersions {
                        master_version,
                        resource_version: config.resource_version.unwrap_or_default(),
                    });
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
            managed.sync_versions(&client, false).await;
            assert_eq!(client.generation(), generation);
            assert_eq!(managed.status().phase, phase);
        }
    }

    /// The game after a client release: older clients get Version, the live one is
    /// served unless the release is still in maintenance, newer ones get maintenance.
    struct Release {
        live: Mutex<String>,
        maintenance: Mutex<bool>,
        seen: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl moenotes_client::transport::Transport for Release {
        async fn call(
            &self,
            method: Method,
            _: DynamicMessage,
            metadata: MetadataMap,
            _: Duration,
        ) -> Result<DynamicMessage, ClientError> {
            assert_eq!(
                method.name, "version",
                "only the anonymous Version call is made"
            );
            let presented = metadata
                .get("x-client-version")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            self.seen.lock().unwrap().push(presented.clone());
            let live = self.live.lock().unwrap().clone();
            let order = |v: &str| {
                v.split('.')
                    .map(|p| p.parse::<u32>().unwrap())
                    .collect::<Vec<_>>()
            };
            match order(&presented).cmp(&order(&live)) {
                std::cmp::Ordering::Less => Err(ClientError::new(ErrorKind::Version)),
                std::cmp::Ordering::Equal if !*self.maintenance.lock().unwrap() => {
                    let desc = moenotes_proto::pool()
                        .get_message_by_name(method.output)
                        .unwrap();
                    Ok(DynamicMessage::deserialize(
                        desc,
                        serde_json::json!({"version":format!("master-{live}"),"resourceVersion":"1.0.0.105"}),
                    )
                    .unwrap())
                }
                _ => Err(ClientError::new(ErrorKind::Maintenance)),
            }
        }
    }
    fn release(configured: &str, live: &str) -> (Arc<Client>, Arc<ManagedClient>, Arc<Release>) {
        let config = SessionConfig {
            region: "test".into(),
            origin: "https://game.invalid".into(),
            allowed_origins: vec!["https://game.invalid".into()],
            platform: "android".into(),
            client_version: configured.into(),
            master_version: Some("old".into()),
            resource_version: Some("old".into()),
        };
        let mock = Arc::new(Release {
            live: Mutex::new(live.into()),
            maintenance: Mutex::new(false),
            seen: Mutex::new(Vec::new()),
        });
        let client = Arc::new(
            Client::with_transport(
                config,
                None,
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
        managed.state.lock().unwrap().version_sync = Some(VersionSyncStatus {
            interval_seconds: 60,
            current: None,
            client_version: configured.into(),
            follow_client_updates: true,
            checks: 0,
            updates: 0,
            client_updates: 0,
            last_checked_at: None,
            last_error: None,
        });
        (client, managed, mock)
    }
    #[tokio::test]
    async fn follows_a_patch_release_the_game_accepts() {
        let (client, managed, mock) = release("1.0.3", "1.0.5");
        managed.sync_versions(&client, true).await;
        let config = client.session_config();
        assert_eq!(config.client_version, "1.0.5");
        assert_eq!(config.master_version.as_deref(), Some("master-1.0.5"));
        assert_eq!(*mock.seen.lock().unwrap(), ["1.0.3", "1.0.4", "1.0.5"]);
        let status = managed.status().version_sync.unwrap();
        assert_eq!(status.client_version, "1.0.5");
        assert_eq!(status.client_updates, 1);
        assert_eq!(status.last_error, None);

        // Once followed, later checks present the new version and stop searching.
        mock.seen.lock().unwrap().clear();
        managed.sync_versions(&client, true).await;
        assert_eq!(*mock.seen.lock().unwrap(), ["1.0.5"]);
        assert_eq!(managed.status().version_sync.unwrap().client_updates, 1);
    }
    #[tokio::test]
    async fn waits_out_release_maintenance_and_stays_put_when_off() {
        let (client, managed, mock) = release("1.0.3", "1.0.4");
        *mock.maintenance.lock().unwrap() = true;
        managed.sync_versions(&client, true).await;
        // 1.0.4 is known but closed: stop there, keep 1.0.3, report maintenance.
        assert_eq!(*mock.seen.lock().unwrap(), ["1.0.3", "1.0.4"]);
        assert_eq!(client.session_config().client_version, "1.0.3");
        let status = managed.status().version_sync.unwrap();
        assert_eq!(status.last_error, Some(ErrorKind::Maintenance));
        assert_eq!(status.client_updates, 0);

        *mock.maintenance.lock().unwrap() = false;
        managed.sync_versions(&client, true).await;
        assert_eq!(client.session_config().client_version, "1.0.4");

        let (client, managed, mock) = release("1.0.3", "1.0.4");
        managed.sync_versions(&client, false).await;
        assert_eq!(*mock.seen.lock().unwrap(), ["1.0.3"]);
        assert_eq!(client.session_config().client_version, "1.0.3");
        assert_eq!(
            managed.status().version_sync.unwrap().last_error,
            Some(ErrorKind::Version)
        );
    }
}
