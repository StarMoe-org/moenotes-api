use moenotes_client::{CancellationToken, ErrorKind, SessionStatus};
use moenotes_server::{config::Config, regions::Region, runtime::Runtime};
use std::{fs, path::Path};

fn write(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
fn config_text() -> String {
    r#"api_key="synthetic-region-key-not-for-production-123456"
credentials_file="tw.json"
[version_sync]
enabled=false
[session]
region="hk"
origin="https://tw.invalid"
allowed_origins=["https://tw.invalid"]
platform="android"
client_version="1"
[regions.en]
credentials_file="en.json"
[regions.en.version_sync]
enabled=false
[regions.en.session]
region="en"
origin="https://en.invalid"
allowed_origins=["https://en.invalid"]
platform="android"
client_version="1"
"#
    .into()
}
fn credential(region: &str, origin: &str) -> String {
    serde_json::json!({"region":region,"origin":origin,"credentials":{"player_id":format!("synthetic-{region}"),"credential":"synthetic-secret","device_id":null,"bid":null}}).to_string()
}
#[tokio::test]
async fn separate_runtime_credentials_and_relative_paths_do_not_inherit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    write(&path, &config_text());
    write(
        &dir.path().join("tw.json"),
        &credential("hk", "https://tw.invalid"),
    );
    write(
        &dir.path().join("en.json"),
        &credential("en", "https://en.invalid"),
    );
    let config = Config::read(&path).unwrap();
    let en = config.for_region(&config.regions[&Region::En]);
    assert_eq!(
        en.credentials_file.as_ref().unwrap(),
        &dir.path().join("en.json")
    );
    assert!(en.login.is_none() && en.accounts.is_none());
    let tw = Runtime::new(config.clone(), CancellationToken::new()).unwrap();
    let en = Runtime::new(en, CancellationToken::new()).unwrap();
    assert_eq!(
        tw.client.session_status(),
        SessionStatus::CredentialsUnverified
    );
    assert_eq!(
        en.client.session_status(),
        SessionStatus::CredentialsUnverified
    );
    write(
        &dir.path().join("en.json"),
        &credential("hk", "https://tw.invalid"),
    );
    assert!(
        Runtime::new(
            config.for_region(&config.regions[&Region::En]),
            CancellationToken::new()
        )
        .is_err()
    );
    let no_credentials = config_text().replace("credentials_file=\"en.json\"\n", "");
    write(&path, &no_credentials);
    let config = Config::read(&path).unwrap();
    let en = Runtime::new(
        config.for_region(&config.regions[&Region::En]),
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(en.client.session_status(), SessionStatus::Anonymous);
}
#[test]
fn duplicate_regions_mismatched_labels_origins_and_unknown_config_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = config_text();
    for content in [
        original.replace("regions.en", "regions.tw"),
        original.replace("region=\"en\"", "region=\"kr\""),
        original.replace("https://en.invalid", "https://tw.invalid/"),
        original.replace("regions.en", "regions.jp"),
        original.replace(
            "[regions.en]\n",
            "[regions.en]\napi_key=\"unused-must-fail\"\n",
        ),
        original.replace(
            "[regions.en.version_sync]\nenabled=false",
            "[regions.en.version_sync]\ninterval_seconds=0",
        ),
    ] {
        write(&path, &content);
        assert!(Config::read(&path).is_err());
    }
}
#[test]
fn shared_state_directory_and_unsafe_nested_secrets_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let base = config_text()
        .replace(
            "credentials_file=\"tw.json\"",
            "[login]\ncontext_file=\"context.json\"\nstate_dir=\"state\"",
        )
        .replace(
            "credentials_file=\"en.json\"",
            "[regions.en.login]\ncontext_file=\"context.json\"\nstate_dir=\"./state\"",
        );
    write(&path, &base);
    assert!(Config::read(&path).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let text=config_text().replace("api_key=\"synthetic-region-key-not-for-production-123456\"","api_key_file=\"key\"")
            .replace("credentials_file=\"en.json\"", "[regions.en.login]\nstate_dir=\"en-state\"\n[regions.en.login.context]\ndevice_model=\"synthetic\"\noperating_system=\"synthetic\"\ndevice_identifier=\"private-nested-context\"\nglobal_channel_id=1\nbrand_id=1\narea_id=1");
        write(&path, &text);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            Config::read(&path).err().unwrap().kind,
            ErrorKind::InvalidConfig
        );
        assert!(moenotes_server::startup::inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
    }
}
#[test]
fn recognized_profile_ranges_and_session_aliases() {
    for (id, region) in [
        ("20000000000", Region::Tw),
        ("29999999999", Region::Tw),
        ("30000000000", Region::En),
        ("39999999999", Region::En),
        ("40000000000", Region::Kr),
        ("49999999999", Region::Kr),
    ] {
        assert_eq!(Region::from_profile_id(id).unwrap().0, region);
    }
    for id in [
        "19999999999",
        "50000000000",
        "020000000001",
        "2e10",
        "２００００００００００",
        "",
    ] {
        assert!(Region::from_profile_id(id).is_err());
    }
    assert_eq!(Region::from_session("hk"), Some(Region::Tw));
    assert_eq!(Region::from_session("hk-tw-mo"), Some(Region::Tw));
    assert_eq!(Region::from_session("jp"), None);
}

#[test]
fn relative_config_path_resolves_each_source_once() {
    let dir = tempfile::Builder::new()
        .prefix("regions-test-")
        .tempdir_in(".")
        .unwrap();
    let path = dir.path().join("config.toml");
    write(&path, &config_text());
    let config = Config::read(&path).unwrap();
    let en = config.for_region(&config.regions[&Region::En]);
    let expected = std::env::current_dir()
        .unwrap()
        .join(dir.path())
        .join("en.json");
    assert_eq!(en.credentials_file.unwrap(), expected);
}
