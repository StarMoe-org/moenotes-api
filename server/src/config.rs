use crate::{operator::LoginConfig, projection::ResponseMode};
use moenotes_client::{ClientError, ClientOptions, ErrorKind, SessionConfig};
use serde::Deserialize;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub enable_experimental_raw: bool,
    pub response_mode: Option<ResponseMode>,
    #[serde(default = "yes")]
    pub access_log: bool,
    pub login: Option<LoginConfig>,
    pub accounts: Option<crate::accounts::AccountsConfig>,
    #[serde(default)]
    pub recovery: RecoveryConfig,
    #[serde(default)]
    pub version_sync: VersionSyncConfig,
    #[serde(default)]
    pub api_key_file: PathBuf,
    pub api_key: Option<crate::secret::SecretString>,
    pub credentials_file: Option<PathBuf>,
    pub session: SessionConfig,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "interval")]
    pub minimum_interval_ms: u64,
    #[serde(default = "capacity")]
    pub queue_capacity: usize,
    #[serde(default = "ttl")]
    pub cache_ttl_seconds: u64,
    #[serde(default = "entries")]
    pub cache_capacity: usize,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionSyncConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "version_interval")]
    pub interval_seconds: u64,
}
fn version_interval() -> u64 {
    60
}
impl Default for VersionSyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: version_interval(),
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "cooldown")]
    pub cooldown_seconds: u64,
}
fn cooldown() -> u64 {
    300
}
fn yes() -> bool {
    true
}
impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cooldown_seconds: cooldown(),
        }
    }
}
fn listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
}
fn timeout() -> u64 {
    10
}
fn interval() -> u64 {
    1000
}
fn capacity() -> usize {
    32
}
fn ttl() -> u64 {
    15
}
fn entries() -> usize {
    1024
}

impl Config {
    pub fn read(path: &Path) -> Result<Self, ClientError> {
        let text = read_config(path).map_err(|_| invalid())?;
        Self::from_text(&text, path)
    }

    pub(crate) fn from_text(text: &str, path: &Path) -> Result<Self, ClientError> {
        let mut config: Self = toml::from_str(text).map_err(|_| invalid())?;
        let value: toml::Value = toml::from_str(text).map_err(|_| invalid())?;
        if value.get("api_key").is_some() && value.get("api_key_file").is_some() {
            return Err(invalid());
        }
        if let Some(login) = value.get("login") {
            for (inline, file) in [("context", "context_file"), ("sdk_http", "sdk_http_file")] {
                if login.get(inline).is_some() && login.get(file).is_some() {
                    return Err(invalid());
                }
            }
        }
        check_inline_permissions(&value, path)?;
        if config.cache_capacity > 16384
            || config.cache_ttl_seconds > 3600
            || config.minimum_interval_ms > 60000
            || config.queue_capacity > 4096
            || config.timeout_seconds == 0
            || config.timeout_seconds > 120
            || config.recovery.cooldown_seconds < 60
            || config.recovery.cooldown_seconds > 86400
            || !(30..=86400).contains(&config.version_sync.interval_seconds)
            || config.recovery.enabled && config.login.is_none()
            || config.login.is_some() && config.credentials_file.is_some()
            || config.accounts.is_some() && config.login.is_none()
            || config.accounts.is_some() && config.login.as_ref().is_some_and(|l| !l.has_sdk_http())
            || config.api_key.is_some() == !config.api_key_file.as_os_str().is_empty()
            || config.enable_experimental_raw
                && config.response_mode.is_some_and(|m| m != ResponseMode::Raw)
        {
            return Err(invalid());
        }
        config.session.validate()?;
        let parent = path.parent().unwrap_or(Path::new("."));
        if !config.api_key_file.as_os_str().is_empty() && config.api_key_file.is_relative() {
            config.api_key_file = parent.join(&config.api_key_file);
        }
        if let Some(file) = &mut config.credentials_file
            && file.is_relative()
        {
            *file = parent.join(&*file);
        }
        if let Some(login) = &mut config.login {
            login.validate_sources()?;
            if !login.context_file.as_os_str().is_empty() && login.context_file.is_relative() {
                login.context_file = parent.join(&login.context_file);
            }
            if login.state_dir.is_relative() {
                login.state_dir = parent.join(&login.state_dir);
            }
            if let Some(file) = &mut login.sdk_http_file
                && file.is_relative()
            {
                *file = parent.join(&*file);
            }
        }
        if let Some(accounts) = &mut config.accounts {
            accounts.validate()?;
            if accounts.directory.is_relative() {
                accounts.directory = parent.join(&accounts.directory);
            }
        }
        Ok(config)
    }

    pub fn read_api_key(&self) -> Result<Zeroizing<String>, ClientError> {
        let value = match &self.api_key {
            Some(key) => Zeroizing::new(key.0.clone()),
            None => read_api_key(&self.api_key_file)?,
        };
        validate_api_key(&value)?;
        Ok(value)
    }

    pub fn mode(&self) -> ResponseMode {
        self.response_mode
            .unwrap_or(if self.enable_experimental_raw {
                ResponseMode::Raw
            } else {
                ResponseMode::Public
            })
    }

    pub fn client_options(&self) -> ClientOptions {
        ClientOptions {
            timeout: Duration::from_secs(self.timeout_seconds),
            minimum_interval: Duration::from_millis(self.minimum_interval_ms),
            queue_capacity: self.queue_capacity,
        }
    }
}

pub(crate) fn validate_api_key(key: &str) -> Result<(), ClientError> {
    if !(32..=4096).contains(&key.len())
        || !key.is_ascii()
        || key
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn read_config(path: &Path) -> Result<Zeroizing<String>, std::io::Error> {
    use std::io::{Error, ErrorKind, Read};
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::from(ErrorKind::InvalidInput));
    }
    let mut text = Zeroizing::new(String::new());
    file.take(65537).read_to_string(&mut text)?;
    if text.len() > 65536 {
        return Err(Error::from(ErrorKind::InvalidInput));
    }
    Ok(text)
}

pub(crate) fn check_inline_permissions(
    value: &toml::Value,
    path: &Path,
) -> Result<(), ClientError> {
    let has_inline = value
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|s| !s.is_empty())
        || value.get("login").is_some_and(|login| {
            login
                .get("context")
                .and_then(toml::Value::as_table)
                .is_some_and(|table| {
                    table
                        .values()
                        .any(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                })
                || login
                    .get("sdk_http")
                    .and_then(|v| v.get("app_key"))
                    .and_then(toml::Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                || login
                    .get("sdk_http")
                    .and_then(|v| v.get("common"))
                    .and_then(toml::Value::as_table)
                    .is_some_and(|table| {
                        table
                            .values()
                            .any(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                    })
        });
    if has_inline {
        let meta = std::fs::symlink_metadata(path).map_err(|_| invalid())?;
        if !meta.is_file() {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(invalid());
            }
        }
        #[cfg(not(unix))]
        return Err(invalid());
    }
    Ok(())
}

pub fn read_api_key(path: &Path) -> Result<Zeroizing<String>, ClientError> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| invalid())?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    let mut value = Zeroizing::new(String::new());
    file.take(4097)
        .read_to_string(&mut value)
        .map_err(|_| invalid())?;
    if value.len() > 4096 {
        return Err(invalid());
    }
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    Ok(value)
}
fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::startup::{Startup, inspect};
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    const INLINE: &str = include_str!("../tests/fixtures/inline-config.toml");
    fn setup(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        (dir, path)
    }
    #[tokio::test]
    async fn all_inline_and_mixed_sources_validate_without_external_files() {
        let (_dir, path) = setup(INLINE);
        let config = Config::read(&path).unwrap();
        assert_eq!(
            &*config.read_api_key().unwrap(),
            "synthetic-bootstrap-api-key-1234567890"
        );
        assert_eq!(
            format!("{:?}", config.api_key.as_ref().unwrap()),
            "[REDACTED]"
        );
        config
            .login
            .as_ref()
            .unwrap()
            .validate(&config.session)
            .unwrap();
        assert!(matches!(
            inspect(&path, "127.0.0.1:0".parse().unwrap()).unwrap(),
            Startup::Configured(_)
        ));
        let key = path.with_file_name("key");
        fs::write(&key, "synthetic-bootstrap-api-key-1234567890").unwrap();
        fs::set_permissions(key, fs::Permissions::from_mode(0o600)).unwrap();
        let mixed = INLINE.replace(
            "api_key = \"synthetic-bootstrap-api-key-1234567890\"",
            "api_key_file = \"key\"",
        );
        fs::write(&path, mixed).unwrap();
        let config = Config::read(&path).unwrap();
        config.read_api_key().unwrap();
        config
            .login
            .as_ref()
            .unwrap()
            .validate(&config.session)
            .unwrap();
    }
    #[test]
    fn inline_values_require_private_config_including_partial_config() {
        let (dir, path) = setup(INLINE);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Config::read(&path).is_err());
        assert!(inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link.toml");
        symlink(&path, &link).unwrap();
        assert!(Config::read(&link).is_err());
        fs::write(&path, "[login.sdk_http.common]\nudid=\"synthetic-secret\"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
    }
    #[test]
    fn missing_inline_fields_bootstrap_without_secret_values() {
        for (old, new, key) in [
            (
                "api_key = \"synthetic-bootstrap-api-key-1234567890\"",
                "api_key = \"\"",
                "api_key",
            ),
            (
                "device_identifier = \"synthetic-never-log\"",
                "device_identifier = \"\"",
                "login.context.device_identifier",
            ),
            (
                "app_key = \"synthetic-never-log\"",
                "app_key = \"\"",
                "login.sdk_http.app_key",
            ),
            ("country_id = 1", "", "login.sdk_http.country_id"),
            ("game_id = \"synthetic\"", "", "login.sdk_http.common"),
        ] {
            let (_dir, path) = setup(&INLINE.replace(old, new));
            let Startup::Unconfigured { missing, .. } =
                inspect(&path, "127.0.0.1:0".parse().unwrap()).unwrap()
            else {
                panic!("expected health only")
            };
            assert!(missing.contains(&key));
            assert!(!format!("{missing:?}").contains("synthetic"));
        }
    }
    #[test]
    fn conflicting_sources_types_and_sdk_omissions_fail_closed() {
        for text in [
            format!("api_key_file=\"key\"\n{INLINE}"),
            INLINE.replace("[login]\n", "[login]\ncontext_file=\"\"\n"),
            INLINE.replace("[login]\n", "[login]\nsdk_http_file=\"sdk.json\"\n"),
            INLINE.replace(
                "omit_common = [\"adid\"]",
                "omit_common = [\"adid\",\"adid\"]",
            ),
            INLINE.replace("omit_common = [\"adid\"]", "omit_common = [\"game_id\"]"),
            INLINE.replace("omit_common = [\"adid\"]", "omit_common = [\"unknown\"]"),
            INLINE.replace("country_id = 1", "country_id = -1"),
            INLINE.replace("game_id = \"synthetic\"", "game_id = 12"),
        ] {
            let (_dir, path) = setup(&text);
            assert!(inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
        }
        let (_dir, path) = setup(&" ".repeat(65537));
        assert!(Config::read(&path).is_err());
        assert!(inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
    }
}
