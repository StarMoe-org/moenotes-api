use crate::{ClientError, ErrorKind};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path};
use tonic::metadata::{MetadataMap, MetadataValue};
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Clone, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub player_id: String,
    pub credential: String,
    pub device_id: Option<String>,
    pub bid: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials([REDACTED])")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    pub region: String,
    pub origin: String,
    pub allowed_origins: Vec<String>,
    pub platform: String,
    pub client_version: String,
    pub master_version: Option<String>,
    pub resource_version: Option<String>,
}

pub trait CredentialProvider: Send + Sync {
    fn credentials(&self, config: &SessionConfig) -> Result<Option<Credentials>, ClientError>;
}

pub struct StaticCredentials {
    region: String,
    origin: String,
    credentials: Credentials,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    region: String,
    origin: String,
    credentials: Credentials,
}

pub(crate) fn save_credentials(
    config: &SessionConfig,
    credentials: &Credentials,
    path: &Path,
) -> Result<(), ClientError> {
    #[derive(Serialize)]
    struct ExportCredentials<'a> {
        player_id: &'a str,
        credential: &'a str,
        device_id: &'a Option<String>,
        bid: &'a Option<String>,
    }
    #[derive(Serialize)]
    struct Export<'a> {
        region: &'a str,
        origin: &'a str,
        credentials: ExportCredentials<'a>,
    }
    let bytes = Zeroizing::new(
        serde_json::to_vec(&Export {
            region: &config.region,
            origin: &config.origin,
            credentials: ExportCredentials {
                player_id: &credentials.player_id,
                credential: &credentials.credential,
                device_id: &credentials.device_id,
                bid: &credentials.bid,
            },
        })
        .map_err(|_| bad_config())?,
    );
    crate::secret_file::create(path, &bytes)
}

impl StaticCredentials {
    pub fn from_json(config: &SessionConfig, bytes: &[u8]) -> Result<Self, ClientError> {
        if bytes.len() > 64 * 1024 {
            return Err(bad_config());
        }
        let stored: CredentialFile = serde_json::from_slice(bytes).map_err(|_| bad_config())?;
        let provider = Self::new(stored.region, stored.origin, stored.credentials);
        let credentials = provider.credentials(config)?.ok_or_else(bad_config)?;
        config.metadata(Some(&credentials), "validation", false)?;
        Ok(provider)
    }
    pub fn new(region: String, origin: String, credentials: Credentials) -> Self {
        Self {
            region,
            origin,
            credentials,
        }
    }

    /// Explicit read-only operator input. On Unix, group/other permissions are rejected.
    pub fn from_file(path: &Path) -> Result<Self, ClientError> {
        if !std::fs::symlink_metadata(path)
            .map_err(|_| bad_config())?
            .is_file()
        {
            return Err(bad_config());
        }
        let file = std::fs::File::open(path).map_err(|_| bad_config())?;
        let meta = file.metadata().map_err(|_| bad_config())?;
        if !meta.is_file() || meta.len() > 64 * 1024 {
            return Err(bad_config());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(bad_config());
            }
        }
        use std::io::Read;
        let mut input = Zeroizing::new(String::new());
        file.take(64 * 1024 + 1)
            .read_to_string(&mut input)
            .map_err(|_| bad_config())?;
        if input.len() > 64 * 1024 {
            return Err(bad_config());
        }
        let parsed: CredentialFile = serde_json::from_str(&input).map_err(|_| bad_config())?;
        Ok(Self::new(parsed.region, parsed.origin, parsed.credentials))
    }
}

impl CredentialProvider for StaticCredentials {
    fn credentials(&self, config: &SessionConfig) -> Result<Option<Credentials>, ClientError> {
        if self.region != config.region || origin(&self.origin)? != origin(&config.origin)? {
            return Err(bad_config());
        }
        Ok(Some(self.credentials.clone()))
    }
}

impl SessionConfig {
    pub fn validate(&self) -> Result<(), ClientError> {
        let target = origin(&self.origin)?;
        if self.region.is_empty()
            || self.platform.is_empty()
            || self.client_version.is_empty()
            || !self
                .allowed_origins
                .iter()
                .any(|v| origin(v).ok().as_ref() == Some(&target))
            || (self.region != "jp"
                && self.master_version.is_some() != self.resource_version.is_some())
            || (self.region == "jp"
                && (self.platform != "android"
                    || self.resource_version.is_some() && self.master_version.is_none()))
            || self.region == "jp"
                && self
                    .master_version
                    .as_ref()
                    .is_some_and(|v| !crate::jp::valid_master(v))
        {
            return Err(bad_config());
        }
        self.metadata(None, "validation", false)?;
        Ok(())
    }

    pub(crate) fn metadata(
        &self,
        credentials: Option<&Credentials>,
        request_id: &str,
        anonymous: bool,
    ) -> Result<MetadataMap, ClientError> {
        let mut result = MetadataMap::new();
        for (key, value) in [
            ("x-request-id", request_id),
            ("x-platform", &self.platform),
            ("x-client-version", &self.client_version),
        ] {
            insert(&mut result, key, value, false)?;
        }
        // Deliberately do not attach player credentials to anonymous support calls.
        if !anonymous && let Some(auth) = credentials {
            if auth.player_id.is_empty() || auth.credential.is_empty() {
                return Err(bad_config());
            }
            insert(&mut result, "x-player-id", &auth.player_id, true)?;
            if self.region == "jp" && auth.bid.is_some() {
                return Err(bad_config());
            }
            insert(&mut result, "x-player-credential", &auth.credential, true)?;
            for (key, value) in [
                ("x-device-id", &auth.device_id),
                ("x-player-bid", &auth.bid),
            ] {
                if self.region == "jp" && key == "x-player-bid" {
                    continue;
                }
                if let Some(value) = value {
                    insert(&mut result, key, value, true)?;
                }
            }
            for (key, value) in [
                ("x-master-version", &self.master_version),
                ("x-resource-version", &self.resource_version),
            ] {
                if self.region == "jp" && key == "x-resource-version" {
                    continue;
                }
                if let Some(value) = value {
                    insert(&mut result, key, value, false)?;
                }
            }
        }
        Ok(result)
    }
}

fn insert(
    map: &mut MetadataMap,
    key: &'static str,
    value: &str,
    secret: bool,
) -> Result<(), ClientError> {
    let mut value = MetadataValue::try_from(value).map_err(|_| bad_config())?;
    value.set_sensitive(secret);
    map.insert(key, value);
    Ok(())
}

pub(crate) fn origin(value: &str) -> Result<String, ClientError> {
    let url = Url::parse(value).map_err(|_| bad_config())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(bad_config());
    }
    Ok(url.origin().ascii_serialization())
}

fn bad_config() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}
