use super::*;
use std::sync::Mutex as StdMutex;

struct Versions {
    reply: StdMutex<Result<serde_json::Value, ErrorKind>>,
    calls: Calls,
}
#[async_trait]
impl Transport for Versions {
    async fn call(
        &self,
        method: Method,
        request: DynamicMessage,
        metadata: MetadataMap,
        _: Duration,
    ) -> Result<DynamicMessage, ClientError> {
        self.calls
            .lock()
            .unwrap()
            .push((method, request.encode_to_vec(), metadata));
        let desc = pool().get_message_by_name(method.output).unwrap();
        if method.name == "version" {
            match self.reply.lock().unwrap().clone() {
                Ok(value) => Ok(DynamicMessage::deserialize(desc, value).unwrap()),
                Err(kind) => Err(ClientError::new(kind)),
            }
        } else {
            Ok(DynamicMessage::new(desc))
        }
    }
}
fn setup() -> (Client, Arc<Versions>) {
    let mock = Arc::new(Versions {
        reply: StdMutex::new(Ok(
            serde_json::json!({"version":"new-master","resourceVersion":"1.0.0.105"}),
        )),
        calls: Arc::default(),
    });
    let client =
        Client::with_transport(config(), Some(&credentials()), options(), mock.clone()).unwrap();
    (client, mock)
}

#[tokio::test]
async fn updates_both_versions_preserves_credentials_and_rejects_old_work() {
    let (client, mock) = setup();
    let old = client.generation();
    assert!(
        client
            .refresh_versions(old, CancellationToken::new())
            .await
            .unwrap()
    );
    assert_ne!(client.generation(), old);
    assert!(
        client
            .matches_identity(client.generation(), &credentials())
            .unwrap()
    );
    assert_eq!(
        client
            .execute(
                old,
                Query::Whoami(Default::default()),
                CancellationToken::new()
            )
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::SessionChanged
    );
    client
        .query(Query::Whoami(Default::default()))
        .await
        .unwrap();
    let generation = client.generation();
    assert!(
        !client
            .refresh_versions(generation, CancellationToken::new())
            .await
            .unwrap()
    );
    assert_eq!(client.generation(), generation);
    let calls = mock.calls.lock().unwrap();
    for (method, _, headers) in calls.iter() {
        if method.name == "version" {
            for key in [
                "x-player-id",
                "x-player-credential",
                "x-player-bid",
                "x-master-version",
                "x-resource-version",
            ] {
                assert!(!headers.contains_key(key));
            }
        } else {
            assert_eq!(headers.get("x-master-version").unwrap(), "new-master");
            assert_eq!(headers.get("x-resource-version").unwrap(), "1.0.0.105");
            assert_eq!(
                headers.get("x-player-credential").unwrap(),
                "synthetic-private-value"
            );
        }
        assert!(!matches!(method.name, "player-login" | "player-pre-login"));
    }
}

#[tokio::test]
async fn clears_only_explicit_master_mismatch_on_a_changed_pair() {
    for (kind, mismatch, expected) in [
        (ErrorKind::Version, true, None),
        (ErrorKind::Version, false, Some(ErrorKind::Version)),
        (
            ErrorKind::Authentication,
            false,
            Some(ErrorKind::Authentication),
        ),
        (
            ErrorKind::DeviceConflict,
            false,
            Some(ErrorKind::DeviceConflict),
        ),
    ] {
        let (client, _) = setup();
        let current = client.session.read().unwrap().clone();
        *current.blocked.write().unwrap() = Some(kind);
        current.master_mismatch.store(mismatch, Ordering::SeqCst);
        client
            .refresh_versions(client.generation(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(client.query_error(false).map(|e| e.kind), expected);
    }
    let (client, mock) = setup();
    *mock.reply.lock().unwrap() =
        Ok(serde_json::json!({"version":"master-test","resourceVersion":"resource-test"}));
    let current = client.session.read().unwrap().clone();
    *current.blocked.write().unwrap() = Some(ErrorKind::Version);
    current.master_mismatch.store(true, Ordering::SeqCst);
    assert!(
        !client
            .refresh_versions(client.generation(), CancellationToken::new())
            .await
            .unwrap()
    );
    assert_eq!(client.session_status(), SessionStatus::VersionBlocked);
}

#[tokio::test]
async fn malformed_unavailable_and_cancelled_checks_keep_last_good_versions() {
    let (client, mock) = setup();
    let old = client.generation();
    for reply in [
        Ok(serde_json::json!({"version":"new-master"})),
        Ok(serde_json::json!({"version":"bad\nvalue","resourceVersion":"new"})),
        Err(ErrorKind::Maintenance),
        Err(ErrorKind::Transport),
    ] {
        *mock.reply.lock().unwrap() = reply;
        assert!(
            client
                .refresh_versions(old, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(client.generation(), old);
        assert_eq!(
            client.session_config().master_version.as_deref(),
            Some("master-test")
        );
        assert!(client.query_error(false).is_none());
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client.refresh_versions(old, cancel).await.unwrap_err().kind,
        ErrorKind::Cancelled
    );
}
