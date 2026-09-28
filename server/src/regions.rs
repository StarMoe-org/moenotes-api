//! Region routing. Every backend owns its session and cache.
use crate::{
    cache::{CacheOptions, QueryCache},
    managed::ManagedClient,
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
}
impl Region {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tw => "tw",
            Self::En => "en",
            Self::Kr => "kr",
        }
    }
    pub fn from_session(region: &str) -> Option<Self> {
        match region {
            "hk" | "tw" | "hk-tw-mo" => Some(Self::Tw),
            "en" => Some(Self::En),
            "kr" => Some(Self::Kr),
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
}
struct Backend {
    cache: Arc<QueryCache>,
    managed: Option<Arc<ManagedClient>>,
}
impl RegionClients {
    pub fn new(
        default_region: Option<Region>,
        backends: Vec<RegionBackend>,
        options: CacheOptions,
        stop: CancellationToken,
    ) -> Result<Self, ClientError> {
        options.validate()?;
        let mut result = Self::default();
        for backend in backends {
            if result
                .backends
                .insert(
                    backend.region,
                    Backend {
                        cache: QueryCache::new(backend.client, options.clone(), stop.clone()),
                        managed: backend.managed,
                    },
                )
                .is_some()
            {
                return Err(ClientError::new(moenotes_client::ErrorKind::InvalidConfig));
            }
        }
        if default_region.is_some_and(|region| !result.backends.contains_key(&region)) {
            return Err(ClientError::new(moenotes_client::ErrorKind::InvalidConfig));
        }
        result.default_region = default_region;
        Ok(result)
    }
    pub(crate) fn cache(&self, region: Region) -> Option<&Arc<QueryCache>> {
        self.backends.get(&region).map(|b| &b.cache)
    }
    pub(crate) fn default_cache(&self) -> Option<Arc<QueryCache>> {
        self.default_region
            .and_then(|region| self.cache(region).cloned())
    }
    pub fn status(&self) -> serde_json::Value {
        serde_json::Value::Object(self.backends.iter().map(|(region, b)| (region.as_str().into(), serde_json::json!({"ready": b.managed.as_ref().is_some_and(|m|m.ready()), "session":b.managed.as_ref().map(|m|m.status())}))).collect())
    }
}
