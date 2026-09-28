//! Lazy account-file login. Health/status calls never read passwords or log in.
use crate::{
    managed::{GameRecovery, Recovery},
    operator::{self, LoginConfig},
};
use async_trait::async_trait;
use moenotes_client::{
    CancellationToken, Client, ClientError, ErrorKind, Generation, QueryClient, SessionConfig,
    SessionStatus, auth::SdkAuthorization,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const MAX_FILES: usize = 128;
fn directory() -> PathBuf {
    PathBuf::from("/accounts")
}
fn yes() -> bool {
    true
}
fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountsConfig {
    #[serde(default = "directory")]
    pub directory: PathBuf,
    pub selected: Option<String>,
    #[serde(default = "yes")]
    pub allow_create: bool,
    /// Explicit assertion that required SDK consent/post-login steps are complete.
    #[serde(default)]
    pub sdk_ready: bool,
}
impl AccountsConfig {
    pub fn validate(&self) -> Result<(), ClientError> {
        if self.directory.as_os_str().is_empty()
            || self.selected.as_deref().is_some_and(|s| !valid_name(s))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn valid_name(name: &str) -> bool {
    !name.starts_with('.')
        && name.ends_with(".json")
        && name.len() <= 200
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
        && Path::new(name).components().count() == 1
}
#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct Account {
    user: String,
    password: String,
}
impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Account([REDACTED])")
    }
}

fn file_list(config: &AccountsConfig) -> Result<Vec<PathBuf>, ClientError> {
    config.validate()?;
    match fs::symlink_metadata(&config.directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(invalid()),
        Ok(_) => operator::private_dir(&config.directory)?,
    }
    if let Some(name) = &config.selected {
        let path = config.directory.join(name);
        return match fs::symlink_metadata(&path) {
            Ok(_) => Ok(vec![path]),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err(invalid()),
        };
    }
    let mut files = Vec::new();
    for (index, entry) in fs::read_dir(&config.directory)
        .map_err(|_| invalid())?
        .take(MAX_FILES + 1)
        .enumerate()
    {
        let entry = entry.map_err(|_| invalid())?;
        if index == MAX_FILES {
            return Err(invalid());
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(invalid)?;
        if valid_name(name) {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}
fn select(config: &AccountsConfig) -> Result<Account, ClientError> {
    let files = file_list(config)?;
    if files.is_empty() {
        return Err(ClientError::new(ErrorKind::AuthenticationRequired));
    }
    if files.len() != 1 {
        return Err(invalid());
    }
    let raw = operator::private_read(&files[0])?;
    let account: Account = serde_json::from_slice(&raw).map_err(|_| invalid())?;
    if account.user.trim().is_empty()
        || account.user.len() > 1024
        || account.password.is_empty()
        || account.password.len() > 4096
    {
        return Err(invalid());
    }
    Ok(account)
}
fn hash_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hash.finalize().into()
}

pub struct AccountDirectoryClient {
    client: Arc<Client>,
    accounts: AccountsConfig,
    login: LoginConfig,
    session: SessionConfig,
    selected_identity: Mutex<Option<[u8; 32]>>,
}
impl AccountDirectoryClient {
    pub fn new(
        client: Arc<Client>,
        accounts: AccountsConfig,
        login: LoginConfig,
        session: SessionConfig,
    ) -> Result<Self, ClientError> {
        accounts.validate()?;
        session.validate()?;
        Ok(Self {
            client,
            accounts,
            login,
            session,
            selected_identity: Mutex::new(None),
        })
    }
    pub fn reset(&self) {
        *self.selected_identity.lock().unwrap() = None;
    }
    fn scoped_login(&self, account: &Account) -> Result<LoginConfig, ClientError> {
        operator::private_dir(&self.login.state_dir)?;
        let hash = hash_parts(&[
            account.user.as_bytes(),
            self.session.region.as_bytes(),
            self.session.origin.as_bytes(),
        ]);
        let name = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let state = self.login.state_dir.join(format!("account-{name}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&state) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(invalid()),
        }
        operator::private_dir(&state)?;
        Ok(LoginConfig {
            state_dir: state,
            ..self.login.clone()
        })
    }
    async fn login_role(
        &self,
        sdk: &SdkAuthorization,
        login: &LoginConfig,
        generation: Generation,
        cancel: CancellationToken,
    ) -> Result<(), ClientError> {
        let context = login.context()?;
        let exists = self
            .client
            .pre_login_with_sdk(generation, sdk, &context, cancel.clone())
            .await?;
        if !exists && !self.accounts.allow_create {
            eprintln!("{{\"event\":\"account_load\",\"status\":\"role_creation_disabled\"}}");
            return Err(ClientError::new(ErrorKind::AuthenticationRequired));
        }
        let receipt = self
            .client
            .login_with_sdk(generation, sdk, &context, cancel.clone())
            .await?;
        if cancel.is_cancelled() {
            return Err(ClientError::new(ErrorKind::Cancelled));
        }
        login.persist(&self.client, &self.session, sdk)?;
        eprintln!(
            "{}",
            serde_json::json!({"event":"account_load","status":"persisted","role_existed":exists,"is_new_user":receipt.is_new_user})
        );
        Ok(())
    }
}
#[async_trait]
impl Recovery for AccountDirectoryClient {
    fn input_revision(&self) -> Option<[u8; 32]> {
        let account = select(&self.accounts);
        Some(match account {
            Ok(account) => hash_parts(&[account.user.as_bytes(), account.password.as_bytes()]),
            Err(e) => hash_parts(&[format!("{:?}", e.kind).as_bytes()]),
        })
    }
    async fn recover(
        &self,
        generation: Generation,
        cancel: CancellationToken,
    ) -> Result<(), ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::new(ErrorKind::Cancelled));
        }
        if self.client.generation() != generation {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        let account = select(&self.accounts).inspect_err(|e| {
            eprintln!(
                "{}",
                serde_json::json!({"event":"account_load","status":if e.kind==ErrorKind::AuthenticationRequired {"no_account"} else {"invalid_account_selection"},"error":e.kind})
            )
        })?;
        let identity = hash_parts(&[account.user.as_bytes()]);
        {
            let mut current = self.selected_identity.lock().unwrap();
            if current.is_some_and(|old| old != identity) {
                return Err(invalid());
            }
            *current = Some(identity);
        }
        let login = self.scoped_login(&account)?;
        if self.client.session_status() == SessionStatus::AuthenticationRejected {
            drop(account);
            return GameRecovery {
                client: self.client.clone(),
                login,
                config: self.session.clone(),
            }
            .recover(generation, cancel)
            .await;
        }
        if self.client.session_status() != SessionStatus::Anonymous {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        let _lock = login.lock()?;
        if let Some(saved) = login.credentials(&self.session)? {
            if cancel.is_cancelled() {
                return Err(ClientError::new(ErrorKind::Cancelled));
            }
            self.client
                .replace_session(self.client.session_config(), Some(&saved))?;
            eprintln!("{{\"event\":\"account_load\",\"status\":\"saved_session_loaded\"}}");
            return Ok(());
        }
        if !self.accounts.sdk_ready {
            eprintln!(
                "{{\"event\":\"account_load\",\"status\":\"sdk_readiness_confirmation_required\"}}"
            );
            return Err(ClientError::new(ErrorKind::AuthenticationRequired));
        }
        login.validate(&self.session)?;
        let sdk_path = login.state_dir.join("initial-sdk.json");
        let sdk = match fs::symlink_metadata(&sdk_path) {
            Ok(meta) => {
                if !meta.is_file() {
                    return Err(invalid());
                }
                SdkAuthorization::from_file(&self.session, &sdk_path)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let sdk = operator::password_authorization(
                    &self.session,
                    &login,
                    account.user.clone(),
                    Zeroizing::new(account.password.clone()),
                    cancel.clone(),
                )
                .await
                .map_err(|safe_error| {
                    eprintln!(
                        "{}",
                        serde_json::json!({"event":"account_sdk_login","error":safe_error})
                    );
                    ClientError::new(ErrorKind::Authentication)
                })?;
                sdk.save_to_file(&sdk_path)?;
                sdk
            }
            Err(_) => return Err(invalid()),
        };
        drop(account);
        self.login_role(&sdk, &login, generation, cancel).await
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use moenotes_client::{ClientOptions, Credentials, StaticCredentials};
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn setup() -> (
        tempfile::TempDir,
        AccountsConfig,
        LoginConfig,
        SessionConfig,
    ) {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let accounts = dir.path().join("accounts");
        fs::create_dir(&accounts).unwrap();
        fs::set_permissions(&accounts, fs::Permissions::from_mode(0o700)).unwrap();
        let context = dir.path().join("context.json");
        write(&context, br#"{"device_model":"synthetic","operating_system":"synthetic","device_identifier":"synthetic","global_channel_id":1,"brand_id":1,"area_id":1}"#);
        let login = LoginConfig {
            context_file: context,
            context: None,
            sdk_http_file: None,
            sdk_http: None,
            state_dir: dir.path().into(),
        };
        let config = SessionConfig {
            region: "test".into(),
            origin: "https://game.invalid".into(),
            allowed_origins: vec!["https://game.invalid".into()],
            platform: "android".into(),
            client_version: "1".into(),
            master_version: None,
            resource_version: None,
        };
        (
            dir,
            AccountsConfig {
                directory: accounts,
                selected: None,
                allow_create: true,
                sdk_ready: true,
            },
            login,
            config,
        )
    }
    fn account() -> Account {
        Account {
            user: "synthetic-user".into(),
            password: "synthetic-password".into(),
        }
    }
    fn put_account(config: &AccountsConfig) {
        write(
            &config.directory.join("one.json"),
            br#"{"user":"synthetic-user","password":"synthetic-password"}"#,
        );
    }
    fn sdk(config: &SessionConfig) -> SdkAuthorization {
        SdkAuthorization::from_callback_json(
            config,
            br#"{"uid":"synthetic-sdk","accessToken":"synthetic-token"}"#,
        )
        .unwrap()
    }
    struct Mock {
        calls: Mutex<Vec<&'static str>>,
        exists: bool,
        pre_error: Option<ErrorKind>,
        completed: AtomicUsize,
        fail_pointer: Mutex<Option<PathBuf>>,
    }
    #[async_trait]
    impl moenotes_client::transport::Transport for Mock {
        async fn call(
            &self,
            method: moenotes_client::Method,
            _: prost_reflect::DynamicMessage,
            _: tonic::metadata::MetadataMap,
            _: Duration,
        ) -> Result<prost_reflect::DynamicMessage, ClientError> {
            self.calls.lock().unwrap().push(method.name);
            if method.name == "player-pre-login"
                && let Some(kind) = self.pre_error
            {
                return Err(ClientError::new(kind));
            }
            let value = match method.name {
                "player-pre-login" => serde_json::json!({"isAccountCreated":self.exists}),
                "player-login" => {
                    self.completed.fetch_add(1, Ordering::SeqCst);
                    if let Some(path) = self.fail_pointer.lock().unwrap().take() {
                        fs::create_dir(path).unwrap();
                    }
                    serde_json::json!({"credential":{"id":"synthetic-player","credential":"synthetic-secret"},"isNewUser":u32::from(!self.exists)})
                }
                _ => return Err(ClientError::new(ErrorKind::Authentication)),
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
    fn client(
        config: &SessionConfig,
        exists: bool,
        pre_error: Option<ErrorKind>,
    ) -> (Arc<Client>, Arc<Mock>) {
        let mock = Arc::new(Mock {
            calls: Mutex::new(Vec::new()),
            exists,
            pre_error,
            completed: AtomicUsize::new(0),
            fail_pointer: Mutex::new(None),
        });
        let client = Client::with_transport(
            config.clone(),
            None,
            ClientOptions {
                minimum_interval: Duration::ZERO,
                ..Default::default()
            },
            mock.clone(),
        )
        .unwrap();
        (Arc::new(client), mock)
    }
    #[test]
    fn defaults_and_strict_selection() {
        let default: AccountsConfig = toml::from_str("").unwrap();
        assert_eq!(default.directory, Path::new("/accounts"));
        assert!(default.allow_create && !default.sdk_ready);
        let (_dir, mut accounts, _, _) = setup();
        assert_eq!(
            select(&accounts).unwrap_err().kind,
            ErrorKind::AuthenticationRequired
        );
        put_account(&accounts);
        assert_eq!(select(&accounts).unwrap().user, "synthetic-user");
        write(
            &accounts.directory.join("two.json"),
            br#"{"user":"other","password":"secret"}"#,
        );
        assert_eq!(
            select(&accounts).unwrap_err().kind,
            ErrorKind::InvalidConfig
        );
        accounts.selected = Some("one.json".into());
        assert_eq!(select(&accounts).unwrap().user, "synthetic-user");
        for name in [
            "../one.json",
            "/one.json",
            "one\\two.json",
            ".one.json",
            "one\n.json",
        ] {
            accounts.selected = Some(name.into());
            assert!(accounts.validate().is_err());
        }
    }
    #[test]
    fn invalid_secret_files_are_bounded_and_redacted() {
        let (_dir, accounts, _, _) = setup();
        let path = accounts.directory.join("one.json");
        for content in [
            br#"{"user":"u","password":"p","extra":1}"#.as_slice(),
            br#"{"user":"u","user":"v","password":"p"}"#,
            br#"{"user":"u"}"#,
            br#"{"user":" ","password":"p"}"#,
            br#"{"user":"u","password":""}"#,
            b"not json",
        ] {
            write(&path, content);
            assert!(select(&accounts).is_err());
        }
        write(&path, &vec![b' '; 65537]);
        assert!(select(&accounts).is_err());
        put_account(&accounts);
        assert_eq!(
            format!("{:?}", select(&accounts).unwrap()),
            "Account([REDACTED])"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(select(&accounts).is_err());
        fs::remove_file(&path).unwrap();
        symlink("absent", &path).unwrap();
        assert!(select(&accounts).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(select(&accounts).is_err());
    }
    #[test]
    fn directory_bounds_and_private_permissions() {
        let (dir, mut accounts, _, _) = setup();
        accounts.directory = dir.path().join("absent");
        assert_eq!(
            select(&accounts).unwrap_err().kind,
            ErrorKind::AuthenticationRequired
        );
        accounts.directory = dir.path().join("link");
        symlink(dir.path().join("accounts"), &accounts.directory).unwrap();
        assert!(select(&accounts).is_err());
        accounts.directory = dir.path().join("accounts");
        for n in 0..=MAX_FILES {
            write(&accounts.directory.join(format!("ignored-{n}")), b"");
        }
        assert!(select(&accounts).is_err());
        accounts.selected = Some("one.json".into());
        put_account(&accounts);
        assert!(select(&accounts).is_ok());
        fs::set_permissions(&accounts.directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(select(&accounts).is_err());
    }
    #[tokio::test]
    async fn prelogin_controls_creation_and_persists_existing_and_new_roles() {
        for (exists, allow, expected_calls) in [
            (true, true, 2),
            (true, false, 2),
            (false, true, 2),
            (false, false, 1),
        ] {
            let (_dir, mut accounts, mut login, config) = setup();
            if exists {
                login.context = Some(
                    serde_json::from_slice(&operator::private_read(&login.context_file).unwrap())
                        .unwrap(),
                );
                fs::remove_file(&login.context_file).unwrap();
                login.context_file = PathBuf::new();
            }
            accounts.allow_create = allow;
            put_account(&accounts);
            let (client, mock) = client(&config, exists, None);
            let loader =
                AccountDirectoryClient::new(client.clone(), accounts, login, config.clone())
                    .unwrap();
            let scoped = loader.scoped_login(&account()).unwrap();
            sdk(&config)
                .save_to_file(&scoped.state_dir.join("initial-sdk.json"))
                .unwrap();
            let old = client.generation();
            let result = loader.recover(old, CancellationToken::new()).await;
            assert_eq!(result.is_ok(), exists || allow);
            assert_eq!(mock.calls.lock().unwrap().len(), expected_calls);
            assert_eq!(
                scoped.credentials(&config).unwrap().is_some(),
                exists || allow
            );
            assert_eq!(client.generation() != old, exists || allow);
            assert_eq!(
                mock.completed.load(Ordering::SeqCst),
                usize::from(exists || allow)
            );
        }
    }
    #[tokio::test]
    async fn failed_prelogin_never_creates_or_retries() {
        let (_dir, accounts, login, config) = setup();
        put_account(&accounts);
        let (client, mock) = client(&config, false, Some(ErrorKind::Maintenance));
        let loader =
            AccountDirectoryClient::new(client.clone(), accounts, login, config.clone()).unwrap();
        let scoped = loader.scoped_login(&account()).unwrap();
        sdk(&config)
            .save_to_file(&scoped.state_dir.join("initial-sdk.json"))
            .unwrap();
        let result = loader
            .recover(client.generation(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(result.kind, ErrorKind::Maintenance);
        assert_eq!(*mock.calls.lock().unwrap(), vec!["player-pre-login"]);
        assert!(scoped.credentials(&config).unwrap().is_none());
    }
    #[tokio::test]
    async fn first_protected_query_loads_once_without_replaying_and_health_stays_live() {
        use crate::managed::{ManagedClient, Phase};
        use moenotes_client::Query;
        let (_dir, accounts, login, config) = setup();
        put_account(&accounts);
        let (client, mock) = client(&config, false, None);
        let loader = Arc::new(
            AccountDirectoryClient::new(client.clone(), accounts, login, config.clone()).unwrap(),
        );
        let scoped = loader.scoped_login(&account()).unwrap();
        sdk(&config)
            .save_to_file(&scoped.state_dir.join("initial-sdk.json"))
            .unwrap();
        let stop = CancellationToken::new();
        let managed = Arc::new(
            ManagedClient::new(client.clone(), None, Duration::ZERO, stop.clone())
                .with_initial_loader(loader),
        );
        let cache =
            crate::cache::QueryCache::new(managed.clone(), Default::default(), stop.clone());
        assert!(!managed.ready());
        assert!(mock.calls.lock().unwrap().is_empty());
        assert_eq!(
            cache
                .query(Query::Whoami(Default::default()))
                .await
                .err()
                .unwrap()
                .kind,
            ErrorKind::AuthenticationRequired
        );
        managed.wait_idle().await;
        assert_eq!(
            *mock.calls.lock().unwrap(),
            vec!["player-pre-login", "player-login"]
        );
        assert_eq!(managed.status().phase, Phase::Unverified);
        assert!(!managed.ready());
        assert!(managed.query_error(false).is_none());
        assert_eq!(managed.status().recovery_attempts, 1);
        stop.cancel();
    }
    #[tokio::test]
    async fn saved_session_loads_without_network_and_keeps_scoped_identity() {
        let (_dir, mut accounts, login, config) = setup();
        put_account(&accounts);
        accounts.sdk_ready = false;
        let (client, mock) = client(&config, true, None);
        let loader = AccountDirectoryClient::new(
            client.clone(),
            accounts.clone(),
            login.clone(),
            config.clone(),
        )
        .unwrap();
        let scoped = loader.scoped_login(&account()).unwrap();
        let saved = Client::new(
            config.clone(),
            Some(&StaticCredentials::new(
                config.region.clone(),
                config.origin.clone(),
                Credentials {
                    player_id: "synthetic-player".into(),
                    credential: "synthetic-secret".into(),
                    device_id: None,
                    bid: Some("synthetic-sdk".into()),
                },
            )),
            ClientOptions::default(),
        )
        .unwrap();
        scoped.persist(&saved, &config, &sdk(&config)).unwrap();
        loader
            .recover(client.generation(), CancellationToken::new())
            .await
            .unwrap();
        assert!(client.query_error(false).is_none());
        assert!(mock.calls.lock().unwrap().is_empty());
        assert!(login.credentials(&config).unwrap().is_none());
        client.replace_session(config.clone(), None).unwrap();
        write(
            &accounts.directory.join("one.json"),
            br#"{"user":"other","password":"secret"}"#,
        );
        assert_eq!(
            loader
                .recover(client.generation(), CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            ErrorKind::InvalidConfig
        );
        loader.reset();
        assert_eq!(
            loader
                .recover(client.generation(), CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            ErrorKind::AuthenticationRequired
        );
    }
    #[tokio::test]
    async fn account_and_region_scopes_are_independent() {
        let (_dir, accounts, login, config) = setup();
        let (client, _) = client(&config, true, None);
        let loader = AccountDirectoryClient::new(
            client.clone(),
            accounts.clone(),
            login.clone(),
            config.clone(),
        )
        .unwrap();
        let path = loader.scoped_login(&account()).unwrap().state_dir;
        assert_eq!(path, loader.scoped_login(&account()).unwrap().state_dir);
        let mut other = account();
        other.user = "other".into();
        assert_ne!(path, loader.scoped_login(&other).unwrap().state_dir);
        let mut region = config;
        region.region = "another".into();
        let other = AccountDirectoryClient::new(client, accounts, login, region).unwrap();
        assert_ne!(path, other.scoped_login(&account()).unwrap().state_dir);
    }
    #[tokio::test]
    async fn missing_file_can_be_added_after_failed_load_and_recovery_uses_saved_sdk() {
        use crate::managed::ManagedClient;
        use moenotes_client::Query;
        let (_dir, accounts, login, config) = setup();
        let (client, mock) = client(&config, true, None);
        let loader = Arc::new(
            AccountDirectoryClient::new(client.clone(), accounts.clone(), login, config.clone())
                .unwrap(),
        );
        let scoped = loader.scoped_login(&account()).unwrap();
        sdk(&config)
            .save_to_file(&scoped.state_dir.join("initial-sdk.json"))
            .unwrap();
        let managed = ManagedClient::new(
            client.clone(),
            Some(loader.clone()),
            Duration::ZERO,
            CancellationToken::new(),
        )
        .with_initial_loader(loader);
        assert_eq!(
            managed.query_error(false).unwrap().kind,
            ErrorKind::AuthenticationRequired
        );
        managed.wait_idle().await;
        assert_eq!(managed.status().recovery_attempts, 1);
        assert!(mock.calls.lock().unwrap().is_empty());
        assert!(managed.query_error(false).is_some());
        assert_eq!(managed.status().recovery_attempts, 1);
        put_account(&accounts);
        assert!(managed.query_error(false).is_some());
        managed.wait_idle().await;
        assert!(managed.query_error(false).is_none());
        assert_eq!(mock.completed.load(Ordering::SeqCst), 1);
        // The protected query is rejected; recovery must use the same scoped SDK.
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
        managed.wait_idle().await;
        assert_eq!(mock.completed.load(Ordering::SeqCst), 2);
        assert!(managed.ready());
        assert_eq!(managed.status().recovery_attempts, 3);
    }
    #[tokio::test]
    async fn first_load_persistence_failure_blocks_installed_session() {
        use crate::managed::{ManagedClient, Phase};
        let (_dir, accounts, login, config) = setup();
        put_account(&accounts);
        let (client, mock) = client(&config, true, None);
        let loader = Arc::new(
            AccountDirectoryClient::new(client.clone(), accounts, login, config.clone()).unwrap(),
        );
        let scoped = loader.scoped_login(&account()).unwrap();
        sdk(&config)
            .save_to_file(&scoped.state_dir.join("initial-sdk.json"))
            .unwrap();
        // Simulate a storage failure after the upstream login has been sent.
        *mock.fail_pointer.lock().unwrap() = Some(scoped.state_dir.join("current.json"));
        let managed = ManagedClient::new(
            client.clone(),
            None,
            Duration::ZERO,
            CancellationToken::new(),
        )
        .with_initial_loader(loader);
        assert!(managed.query_error(false).is_some());
        managed.wait_idle().await;
        assert_eq!(mock.completed.load(Ordering::SeqCst), 1);
        assert_eq!(managed.status().phase, Phase::PersistenceFailed);
        assert!(client.query_error(false).is_none());
        assert!(managed.query_error(false).is_some());
        assert!(!managed.ready());
    }
    #[tokio::test]
    async fn dangling_session_pointer_never_initiates_login() {
        let (_dir, accounts, login, config) = setup();
        put_account(&accounts);
        let (client, mock) = client(&config, true, None);
        let loader =
            AccountDirectoryClient::new(client.clone(), accounts, login, config.clone()).unwrap();
        let scoped = loader.scoped_login(&account()).unwrap();
        sdk(&config)
            .save_to_file(&scoped.state_dir.join("initial-sdk.json"))
            .unwrap();
        symlink("missing", scoped.state_dir.join("current.json")).unwrap();
        let original = client.generation();
        assert!(
            loader
                .recover(original, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(client.generation(), original);
        assert!(mock.calls.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn construction_and_cancel_do_not_read_accounts_or_create_state() {
        let (_dir, accounts, login, config) = setup();
        write(&accounts.directory.join("one.json"), b"invalid json");
        let (client, mock) = client(&config, true, None);
        let loader =
            AccountDirectoryClient::new(client.clone(), accounts, login.clone(), config).unwrap();
        let stop = CancellationToken::new();
        stop.cancel();
        assert_eq!(
            loader
                .recover(client.generation(), stop)
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Cancelled
        );
        assert_eq!(fs::read_dir(login.state_dir).unwrap().count(), 2);
        assert!(mock.calls.lock().unwrap().is_empty());
    }
}
