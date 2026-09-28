use moenotes_client::{CancellationToken, QueryClient, SessionStatus};
use moenotes_server::{
    accounts::{AccountsConfig, JpAccountDirectoryClient},
    config::Config,
    managed::Recovery,
    runtime::Runtime,
};
use std::{fs, path::Path};
fn private(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
fn settings() -> &'static str {
    r#"api_key="synthetic-jp-key-at-least-thirty-two-characters"
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
selected="test.json"
"#
}
#[tokio::test]
async fn import_is_private_bound_lazy_and_never_calls_sdk() {
    let dir = tempfile::tempdir().unwrap();
    moenotes_server::jp_operator::init_accounts(&dir.path().join("accounts")).unwrap();
    let cfgpath = dir.path().join("config.toml");
    private(&cfgpath, settings());
    let config = Config::read(&cfgpath).unwrap();
    assert!(matches!(
        moenotes_server::startup::inspect(&cfgpath, "127.0.0.1:0".parse().unwrap()).unwrap(),
        moenotes_server::startup::Startup::Configured(_)
    ));
    let runtime = Runtime::new(config.clone(), CancellationToken::new()).unwrap();
    assert_eq!(runtime.client.session_status(), SessionStatus::Anonymous);
    let path = dir.path().join("accounts/jp/test.json");
    private(
        &path,
        r#"{"region":"jp","origin":"https://jp.invalid","credentials":{"player_id":"synthetic-player","credential":"synthetic-secret","device_id":"synthetic-device","bid":null}}"#,
    );
    let loader = JpAccountDirectoryClient::new(
        runtime.client.clone(),
        config.accounts.clone().unwrap(),
        config.session.clone(),
    )
    .unwrap();
    loader
        .recover(runtime.client.generation(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        runtime.client.session_status(),
        SessionStatus::CredentialsUnverified
    );
    runtime.reload().unwrap();
    assert_eq!(runtime.client.session_status(), SessionStatus::Anonymous);
    private(
        &path,
        r#"{"region":"hk","origin":"https://jp.invalid","credentials":{"player_id":"p","credential":"k"}}"#,
    );
    assert!(loader.provider().is_err());
    private(&path, r#"{"user":"u","password":"p"}"#);
    assert!(loader.provider().is_err());
    private(
        &cfgpath,
        &settings().replace("[version_sync]", "[recovery]\nenabled=true\n[version_sync]"),
    );
    assert!(Config::read(&cfgpath).is_err());
}
#[tokio::test]
async fn registration_refuses_nonempty_directory_before_network() {
    let dir = tempfile::tempdir().unwrap();
    moenotes_server::jp_operator::init_accounts(&dir.path().join("accounts")).unwrap();
    let cfg = dir.path().join("config.toml");
    private(&cfg, settings());
    let config = Config::read(&cfg).unwrap();
    private(
        &dir.path().join("accounts/jp/registration.attempt"),
        "already attempted",
    );
    assert!(
        moenotes_server::jp_operator::register(&config, "test.json")
            .await
            .is_err()
    );
    assert!(
        moenotes_server::jp_operator::register(&config, "../test.json")
            .await
            .is_err()
    );
    assert!(
        moenotes_server::jp_operator::register(&config, "not-json")
            .await
            .is_err()
    );
}
#[test]
fn separate_defaults_and_legacy_explicit_directory() {
    let accounts: AccountsConfig = toml::from_str("").unwrap();
    assert_eq!(accounts.directory, Path::new("/accounts/international"));
    let legacy: AccountsConfig = toml::from_str("directory=\"/accounts\"").unwrap();
    assert_eq!(legacy.directory, Path::new("/accounts"));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    private(&p, &settings().replace("directory=\"accounts/jp\"\n", ""));
    let cfg = Config::read(&p).unwrap();
    assert_eq!(cfg.accounts.unwrap().directory, Path::new("/accounts/jp"));
}
