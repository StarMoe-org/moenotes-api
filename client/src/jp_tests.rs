use super::*;
use crate::transport::{Transport, TransportResponse};
use crate::{ClientOptions, Query, SessionConfig, StaticCredentials};
use async_trait::async_trait;
use std::{sync::Mutex, time::Duration};
use tonic::metadata::MetadataMap;

fn config() -> SessionConfig {
    SessionConfig {
        region: "jp".into(),
        origin: "https://jp.invalid".into(),
        allowed_origins: vec!["https://jp.invalid".into()],
        platform: "android".into(),
        client_version: "1.0.3".into(),
        master_version: None,
        resource_version: None,
    }
}
struct Mock {
    calls: Mutex<Vec<(Method, MetadataMap)>>,
    asset: Mutex<Option<String>>,
    fail: Mutex<Option<ErrorKind>>,
}
#[async_trait]
impl Transport for Mock {
    async fn call(
        &self,
        m: Method,
        r: DynamicMessage,
        h: MetadataMap,
        t: Duration,
    ) -> Result<DynamicMessage, ClientError> {
        self.call_with_metadata(m, r, h, t).await.map(|r| r.message)
    }
    async fn call_with_metadata(
        &self,
        m: Method,
        _: DynamicMessage,
        h: MetadataMap,
        _: Duration,
    ) -> Result<TransportResponse, ClientError> {
        self.calls.lock().unwrap().push((m, h));
        if let Some(kind) = self.fail.lock().unwrap().take() {
            return Err(ClientError::new(kind));
        }
        let data = match m.name {
            "version" => {
                serde_json::json!({"version":"1.0.0.100/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"})
            }
            "jp-register" => {
                serde_json::json!({"credential":{"id":"synthetic-player","credential":"synthetic-secret","deviceId":"synthetic-device","profileId":"12345"},"profileId":"12345"})
            }
            "whoami" => serde_json::json!({"playerId":"synthetic-player"}),
            "profiles" => serde_json::json!({"players":[]}),
            _ => serde_json::json!({}),
        };
        Ok(TransportResponse {
            message: DynamicMessage::deserialize(
                moenotes_proto::pool_for_region("jp")
                    .get_message_by_name(m.output)
                    .unwrap(),
                data,
            )
            .unwrap(),
            asset_version: self.asset.lock().unwrap().clone(),
        })
    }
}
fn setup() -> (Client, Arc<Mock>) {
    let mock=Arc::new(Mock{calls:Mutex::new(vec![]),asset:Mutex::new(Some(r#"{"live":[{"minClientVersion":"1.0.0","version":"1.0.0.100","Android":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}"#.into())),fail:Mutex::new(None)});
    let client = Client::with_transport(
        config(),
        None,
        ClientOptions {
            minimum_interval: Duration::ZERO,
            ..Default::default()
        },
        mock.clone(),
    )
    .unwrap();
    (client, mock)
}
#[test]
fn paths_asset_selection_and_credential_binding() {
    assert!(valid_master("1.0.0.100/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    for s in [
        "../x",
        "1.0/%2f",
        "1.0/../x",
        "1.0/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?x=1",
    ] {
        assert!(!valid_master(s));
    }
    let raw = r#"{"version":"fallback","Android":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","live":[{"minClientVersion":"2.0","version":"new","Android":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
    assert_eq!(asset_version(Some(raw), "1.0.3"), None);
    let raw = r#"{"live":[{"minClientVersion":"1.0","version":"first","Android":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"minClientVersion":"1.0.0","version":"second","Android":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
    assert_eq!(asset_version(Some(raw), "1.0.3").as_deref(), Some("first"));
    let mut c = config();
    c.master_version = Some("1.0/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    assert!(c.validate().is_ok());
    let cred = Credentials {
        player_id: "synthetic-player".into(),
        credential: "synthetic-secret".into(),
        device_id: Some("synthetic-device".into()),
        bid: None,
    };
    let h = c.metadata(Some(&cred), "req", false).unwrap();
    assert!(
        !h.contains_key("x-resource-version")
            && !h.contains_key("x-player-bid")
            && !h.contains_key("x-override-device-id")
    );
    let raw=br#"{"region":"hk","origin":"https://jp.invalid","credentials":{"player_id":"p","credential":"k","device_id":null,"bid":null}}"#;
    assert!(StaticCredentials::from_json(&c, raw).is_err());
    assert!(
        Query::ServerList(Default::default())
            .method_for_region("jp")
            .is_err()
    );
}
#[tokio::test]
async fn version_registration_persistence_and_override_are_explicit() {
    let (client, mock) = setup();
    client
        .refresh_versions(client.generation(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        client.session_config().resource_version.as_deref(),
        Some("1.0.0.100")
    );
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output = dir.path().join("worker.json");
    client
        .register_jp(client.generation(), &output, CancellationToken::new())
        .await
        .unwrap();
    assert!(output.with_extension("response").is_file());
    let provider = StaticCredentials::from_file(&output).unwrap();
    assert!(
        client
            .matches_identity(client.generation(), &provider)
            .unwrap()
    );
    assert!(
        client
            .register_jp(client.generation(), &output, CancellationToken::new())
            .await
            .is_err()
    );
    *mock.fail.lock().unwrap() = Some(ErrorKind::Transport);
    assert!(
        client
            .query(Query::Whoami(Default::default()))
            .await
            .is_err()
    );
    client
        .query(Query::Whoami(Default::default()))
        .await
        .unwrap();
    client
        .query(Query::Whoami(Default::default()))
        .await
        .unwrap();
    let calls = mock.calls.lock().unwrap();
    let who: Vec<_> = calls.iter().filter(|(m, _)| m.name == "whoami").collect();
    assert!(who[0].1.contains_key("x-override-device-id"));
    assert!(who[1].1.contains_key("x-override-device-id"));
    assert!(!who[2].1.contains_key("x-override-device-id"));
    let register = calls.iter().find(|(m, _)| m.name == "jp-register").unwrap();
    assert!(
        !register.1.contains_key("x-player-id") && !register.1.contains_key("x-master-version")
    );
}
#[tokio::test]
async fn missing_asset_master_only_and_profiles_route() {
    let (client, mock) = setup();
    *mock.asset.lock().unwrap() = None;
    client
        .refresh_versions(client.generation(), CancellationToken::new())
        .await
        .unwrap();
    assert!(client.session_config().resource_version.is_none());
    let session = client.session.read().unwrap().clone();
    let provider = StaticCredentials::new(
        "jp".into(),
        "https://jp.invalid".into(),
        Credentials {
            player_id: "p".into(),
            credential: "k".into(),
            device_id: None,
            bid: None,
        },
    );
    let client = Client::with_transport(
        session.config.clone(),
        Some(&provider),
        ClientOptions {
            minimum_interval: Duration::ZERO,
            ..Default::default()
        },
        mock.clone(),
    )
    .unwrap();
    let query = Query::Profiles(crate::generated::app::playerext::GetPlayerListRequest {
        account_ids: vec![1],
    });
    client.query(query).await.unwrap();
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
        calls.last().unwrap().0.path,
        "/app.player.PlayerService/GetPlayerList"
    );
}

#[test]
fn saved_registration_without_device_id_is_imported_without_network() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let source = dir.path().join("test.response");
    let output = dir.path().join("test.json");
    crate::secret_file::create(
        &source,
        br#"{"credential":{"id":"p","credential":"k","profileId":"12345"},"profileId":"12345"}"#,
    )
    .unwrap();
    import_registration_response(&config(), &source, &output).unwrap();
    let credentials = StaticCredentials::from_file(&output).unwrap();
    let data = crate::CredentialProvider::credentials(&credentials, &config())
        .unwrap()
        .unwrap();
    assert!(data.device_id.is_none() && data.bid.is_none());
    assert!(import_registration_response(&config(), &source, &output).is_err());
}

#[tokio::test]
async fn existing_output_prevents_registration_and_import_cas_rejects_stale_generation() {
    let (client, mock) = setup();
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output = dir.path().join("test.json");
    crate::secret_file::create(&output, b"existing").unwrap();
    assert!(
        client
            .register_jp(client.generation(), &output, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(mock.calls.lock().unwrap().is_empty());
    let old = client.generation();
    client
        .refresh_versions(old, CancellationToken::new())
        .await
        .unwrap();
    let provider = StaticCredentials::new(
        "jp".into(),
        "https://jp.invalid".into(),
        Credentials {
            player_id: "p".into(),
            credential: "k".into(),
            device_id: None,
            bid: None,
        },
    );
    assert_eq!(
        client.import_credentials(old, &provider).unwrap_err().kind,
        ErrorKind::SessionChanged
    );
    client
        .import_credentials(client.generation(), &provider)
        .unwrap();
    client
        .query(Query::Whoami(Default::default()))
        .await
        .unwrap();
    assert!(
        !mock
            .calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .1
            .contains_key("x-override-device-id")
    );
}
