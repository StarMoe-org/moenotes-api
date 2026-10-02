use moenotes_client::CancellationToken;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock, time::Duration};
use tokio::{sync::Mutex, time::Instant};

const URL: &str = "https://metadata.bdon.moe/current_version.json";
const TTL: Duration = Duration::from_secs(60);
const MAX_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
struct Snapshot {
    schema_version: u64,
    regions: BTreeMap<String, serde_json::Value>,
}

#[derive(Default)]
struct Cache {
    checked_at: Option<Instant>,
    snapshot: Option<Snapshot>,
}

pub(super) struct MetadataSource {
    url: String,
    http: Option<reqwest::Client>,
    cache: Mutex<Cache>,
}

pub(super) fn shared() -> &'static MetadataSource {
    static SOURCE: OnceLock<MetadataSource> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let http = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(5))
            .build()
            .ok();
        MetadataSource::new(URL.to_owned(), http)
    })
}

impl MetadataSource {
    pub(super) fn new(url: String, http: Option<reqwest::Client>) -> Self {
        Self {
            url,
            http,
            cache: Mutex::new(Cache::default()),
        }
    }

    pub(super) async fn candidate(
        &self,
        region: &str,
        current: &str,
        cancel: CancellationToken,
    ) -> Option<String> {
        let region = match region {
            "hk" | "tw" | "hk-tw-mo" => "hk-tw-mo",
            "en" => "en",
            "kr" => "kr",
            "jp" => "jp",
            _ => return None,
        };
        let current = version_parts(current)?;
        let mut cache = tokio::select! {
            biased;
            _ = cancel.cancelled() => return None,
            cache = self.cache.lock() => cache,
        };
        if cache
            .checked_at
            .is_none_or(|checked| checked.elapsed() >= TTL)
        {
            cache.snapshot = tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                snapshot = self.fetch() => snapshot,
            };
            cache.checked_at = Some(Instant::now());
        }
        let candidate = cache
            .snapshot
            .as_ref()?
            .regions
            .get(region)?
            .get("client_version")?
            .as_str()?;
        (version_parts(candidate)? > current).then(|| candidate.to_owned())
    }

    async fn fetch(&self) -> Option<Snapshot> {
        let mut response = self
            .http
            .as_ref()?
            .get(&self.url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if response.status() != reqwest::StatusCode::OK
            || response
                .content_length()
                .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return None;
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if body.len().saturating_add(chunk.len()) > MAX_BYTES {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        let snapshot: Snapshot = serde_json::from_slice(&body).ok()?;
        (snapshot.schema_version == 1).then_some(snapshot)
    }
}

fn version_parts(value: &str) -> Option<[u32; 3]> {
    let mut parts = value.split('.');
    let mut parsed = [0; 3];
    for part in &mut parsed {
        let text = parts.next()?;
        if text.is_empty() || text.len() > 9 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *part = text.parse().ok()?;
    }
    parts.next().is_none().then_some(parsed)
}

#[cfg(test)]
pub(super) mod tests;
