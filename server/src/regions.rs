//! Region routing. Every backend owns its session and cache.
use crate::{
    cache::CacheOptions,
    managed::ManagedClient,
    pool::{SessionMember, SessionPool},
};
use moenotes_client::{CancellationToken, ClientError, QueryClient};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Tw,
    En,
    Kr,
    Jp,
}
impl Region {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tw => "tw",
            Self::En => "en",
            Self::Kr => "kr",
            Self::Jp => "jp",
        }
    }
    pub fn from_session(region: &str) -> Option<Self> {
        match region {
            "hk" | "tw" | "hk-tw-mo" => Some(Self::Tw),
            "en" => Some(Self::En),
            "kr" => Some(Self::Kr),
            "jp" => Some(Self::Jp),
            _ => None,
        }
    }
    pub fn from_profile_id(id: &str) -> Result<(Self, i64), &'static str> {
        if id.len() != 11 || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid_profile_id");
        }
        let region = match id.as_bytes()[0] {
            b'2' => Self::Tw,
            b'3' => Self::En,
            b'4' => Self::Kr,
            _ => return Err("unsupported_profile_region"),
        };
        Ok((region, id.parse().map_err(|_| "invalid_profile_id")?))
    }
}

pub struct RegionBackend {
    pub region: Region,
    pub client: Arc<dyn QueryClient>,
    pub managed: Option<Arc<ManagedClient>>,
}
#[derive(Default)]
pub struct RegionClients {
    backends: BTreeMap<Region, Backend>,
    default_region: Option<Region>,
    images: BTreeMap<Region, Arc<crate::profile_images::ProfileImages>>,
}
struct Backend {
    pool: Arc<SessionPool>,
}
impl RegionClients {
    /// Enable this client's regional card proxy. JP uses its effective client version
    /// for anonymous CDN credential discovery; international downloads are public.
    /// Construction makes no network request.
    pub fn enable_profile_images(
        &mut self,
        client: Arc<moenotes_client::Client>,
        stop: CancellationToken,
    ) -> Result<(), ClientError> {
        let region = Region::from_session(&client.session_config().region)
            .ok_or_else(|| ClientError::new(moenotes_client::ErrorKind::InvalidConfig))?;
        self.images.insert(
            region,
            crate::profile_images::ProfileImages::new(client, stop)?,
        );
        Ok(())
    }
    pub(crate) fn profile_images(
        &self,
        region: Region,
    ) -> Option<&Arc<crate::profile_images::ProfileImages>> {
        self.images.get(&region)
    }
    #[cfg(test)]
    pub(crate) fn set_profile_images(
        &mut self,
        region: Region,
        images: Arc<crate::profile_images::ProfileImages>,
    ) {
        self.images.insert(region, images);
    }
    pub fn new(
        default_region: Option<Region>,
        backends: Vec<RegionBackend>,
        options: CacheOptions,
        stop: CancellationToken,
    ) -> Result<Self, ClientError> {
        options.validate()?;
        let pools = backends
            .into_iter()
            .map(|backend| {
                Ok((
                    backend.region,
                    SessionPool::new(
                        vec![SessionMember {
                            client: backend.client,
                            managed: backend.managed,
                        }],
                        options.clone(),
                        stop.clone(),
                    )?,
                ))
            })
            .collect::<Result<Vec<_>, ClientError>>()?;
        Self::with_pools(default_region, pools)
    }
    pub fn with_pools(
        default_region: Option<Region>,
        pools: Vec<(Region, Arc<SessionPool>)>,
    ) -> Result<Self, ClientError> {
        let mut result = Self::default();
        for (region, pool) in pools {
            if result.backends.insert(region, Backend { pool }).is_some() {
                return Err(ClientError::new(moenotes_client::ErrorKind::InvalidConfig));
            }
        }
        if default_region.is_some_and(|region| !result.backends.contains_key(&region)) {
            return Err(ClientError::new(moenotes_client::ErrorKind::InvalidConfig));
        }
        result.default_region = default_region;
        Ok(result)
    }
    pub(crate) fn cache(&self, region: Region) -> Option<&Arc<SessionPool>> {
        self.backends.get(&region).map(|b| &b.pool)
    }
    pub(crate) fn default_cache(&self) -> Option<Arc<SessionPool>> {
        self.default_region
            .and_then(|region| self.cache(region).cloned())
    }
    pub fn status(&self) -> serde_json::Value {
        serde_json::Value::Object(self.backends.iter().map(|(region, b)| (region.as_str().into(), serde_json::json!({"ready": b.pool.ready(), "session":b.pool.single_status(), "pool":b.pool.status()}))).collect())
    }
}
