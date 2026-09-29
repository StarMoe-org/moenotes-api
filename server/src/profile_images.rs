//! JP profile-card PNG proxy. CDN secrets never enter JSON, logs or response headers.
use crate::{ApiState, HttpError, regions::Region};
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use moenotes_client::{
    CancellationToken, Client, ClientError, ErrorKind, Query, transport::DynamicCodec,
};
use prost_reflect::DynamicMessage;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, Semaphore},
    time::Instant,
};
use tonic::{
    Request,
    metadata::MetadataMap,
    transport::{Channel, ClientTlsConfig, Endpoint},
};
use zeroize::Zeroizing;

pub(crate) const ROUTE: &str = "/v1/jp/profile/{profileId}/card/{page}";
const API_ORIGIN: &str = "https://api.bang-dream-on.jp";
const CDN_ORIGIN: &str = "https://static.bang-dream-on.jp";
const MAX_IMAGE: usize = 8 * 1024 * 1024;
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const CACHE_ENTRIES: usize = 128;
const TTL: Duration = Duration::from_secs(300);
const TIMEOUT: Duration = Duration::from_secs(30);

fn err(kind: ErrorKind) -> ClientError {
    ClientError::new(kind)
}
fn error(status: StatusCode, kind: &'static str) -> Response {
    (status, Json(serde_json::json!({"error":{"kind":kind}}))).into_response()
}

pub(crate) fn install(router: Router<ApiState>) -> Router<ApiState> {
    router.route(
        ROUTE,
        get(handle).head(|| async { (StatusCode::METHOD_NOT_ALLOWED, [("allow", "GET")]) }),
    )
}

async fn handle(
    State(state): State<ApiState>,
    params: Result<Path<(String, String)>, axum::extract::rejection::PathRejection>,
    RawQuery(raw): RawQuery,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Response {
    let Ok(Path((id, page))) = params else {
        return HttpError(err(ErrorKind::InvalidRequest)).into_response();
    };
    let positive = |s: &str| {
        !s.is_empty()
            && s.len() <= 19
            && s.bytes().all(|b| b.is_ascii_digit())
            && s.parse::<i64>().is_ok_and(|n| n > 0)
    };
    if !positive(&id)
        || !positive(&page)
        || raw.is_some_and(|q| !q.is_empty())
        || !matches!(body, Ok(ref b) if b.is_empty())
    {
        return HttpError(err(ErrorKind::InvalidRequest)).into_response();
    }
    let id = id.parse::<i64>().unwrap();
    let page = page.parse::<u64>().unwrap();
    let Some(pool) = state.profiles.cache(Region::Jp) else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "region_unconfigured");
    };
    let Some(images) = state.profiles.profile_images() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "image_proxy_unconfigured");
    };
    let work = async {
        let query = Query::from_json(
            "profile",
            serde_json::json!({"playerProfileId":id.to_string()}),
        )?;
        let (profile, _) = pool.query(query).await?;
        let url = profile
            .json
            .pointer("/playerProfile/profileCard/thumbnailUrl")
            .and_then(|v| v.as_array())
            .and_then(|list| usize::try_from(page - 1).ok().and_then(|i| list.get(i)))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let Some(url) = url else {
            return Ok(error(StatusCode::NOT_FOUND, "profile_card_not_found"));
        };
        validate_url(url, id)?;
        let (bytes, status) = images.get(url).await?;
        Ok::<_, ClientError>(
            (
                [
                    ("content-type", "image/png"),
                    ("x-content-type-options", "nosniff"),
                    ("x-moenotes-region", "jp"),
                    ("x-moenotes-cache", status),
                ],
                bytes,
            )
                .into_response(),
        )
    };
    tokio::select! {
        _ = images.stop.cancelled() => HttpError(err(ErrorKind::Cancelled)).into_response(),
        result = tokio::time::timeout(TIMEOUT, work) => match result {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => HttpError(e).into_response(),
            Err(_) => HttpError(err(ErrorKind::Timeout)).into_response(),
        }
    }
}

// Check the raw spelling too: URL parsing normalizes dot segments and backslashes.
fn validate_url(value: &str, id: i64) -> Result<(), ClientError> {
    let prefix = format!("{CDN_ORIGIN}/operation/profilecard/{id}/");
    let Some(file) = value.strip_prefix(&prefix) else {
        return Err(err(ErrorKind::Protocol));
    };
    if file.is_empty()
        || file.len() > 512
        || !file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        || file.contains("..")
        || !file.starts_with(&format!("{id}_"))
    {
        return Err(err(ErrorKind::Protocol));
    }
    Ok(())
}

fn png(bytes: &[u8]) -> bool {
    // Bound dimensions before downstream decoders allocate. No image decoding/re-encoding here.
    bytes.len() >= 45
        && bytes.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
        && bytes.ends_with(b"\0\0\0\0IEND\xaeB`\x82")
        && (1..=8192).contains(&u32::from_be_bytes(bytes[16..20].try_into().unwrap()))
        && (1..=8192).contains(&u32::from_be_bytes(bytes[20..24].try_into().unwrap()))
}

#[async_trait]
trait ImageSource: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<Bytes, ClientError>;
}
struct Cached {
    bytes: Bytes,
    expires: Instant,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<String, Cached>,
    bytes: usize,
}
pub(crate) struct ProfileImages {
    source: Arc<dyn ImageSource>,
    cache: Mutex<Cache>,
    gates: [Mutex<()>; 16],
    downloads: Semaphore,
    stop: CancellationToken,
}
impl ProfileImages {
    pub(crate) fn new(
        client: Arc<Client>,
        stop: CancellationToken,
    ) -> Result<Arc<Self>, ClientError> {
        Ok(Self::with_source(Arc::new(LiveSource::new(client)?), stop))
    }
    fn with_source(source: Arc<dyn ImageSource>, stop: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            source,
            cache: Mutex::new(Cache::default()),
            gates: std::array::from_fn(|_| Mutex::new(())),
            downloads: Semaphore::new(4),
            stop,
        })
    }
    async fn get(&self, url: &str) -> Result<(Bytes, &'static str), ClientError> {
        let bucket = Sha256::digest(url.as_bytes())[0] as usize % self.gates.len();
        let _gate = self.gates[bucket].lock().await;
        {
            let mut cache = self.cache.lock().await;
            cache
                .entries
                .retain(|_, entry| entry.expires > Instant::now());
            cache.bytes = cache.entries.values().map(|e| e.bytes.len()).sum();
            if let Some(entry) = cache.entries.get(url) {
                return Ok((entry.bytes.clone(), "HIT"));
            }
        }
        let _permit = self
            .downloads
            .try_acquire()
            .map_err(|_| err(ErrorKind::QueueFull))?;
        let bytes = self.source.fetch(url).await?;
        if bytes.len() > MAX_IMAGE || !png(&bytes) {
            return Err(err(ErrorKind::Protocol));
        }
        let mut cache = self.cache.lock().await;
        while cache.bytes + bytes.len() > CACHE_BYTES || cache.entries.len() >= CACHE_ENTRIES {
            let Some(key) = cache
                .entries
                .iter()
                .min_by_key(|(_, e)| e.expires)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            cache.bytes -= cache.entries.remove(&key).unwrap().bytes.len();
        }
        cache.bytes += bytes.len();
        cache.entries.insert(
            url.to_owned(),
            Cached {
                bytes: bytes.clone(),
                expires: Instant::now() + TTL,
            },
        );
        Ok((bytes, "MISS"))
    }
}

struct Credential {
    authorization: Zeroizing<String>,
    version: String,
    expires: Instant,
}
#[derive(Default)]
struct AuthState {
    credential: Option<Arc<Credential>>,
    failure: Option<(Instant, ClientError)>,
}
struct LiveSource {
    client: Arc<Client>,
    http: reqwest::Client,
    channel: Channel,
    auth: Mutex<AuthState>,
}
impl LiveSource {
    fn new(client: Arc<Client>) -> Result<Self, ClientError> {
        let config = client.session_config();
        if config.region != "jp"
            || config.origin.trim_end_matches('/') != API_ORIGIN
            || config.platform != "android"
        {
            return Err(err(ErrorKind::InvalidConfig));
        }
        let channel = Endpoint::from_static(API_ORIGIN)
            .tls_config(ClientTlsConfig::new().with_webpki_roots())
            .map_err(|_| err(ErrorKind::InvalidConfig))?
            .connect_timeout(Duration::from_secs(5))
            .connect_lazy();
        let http = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| err(ErrorKind::InvalidConfig))?;
        Ok(Self {
            client,
            http,
            channel,
            auth: Mutex::new(AuthState::default()),
        })
    }
    async fn credential(
        &self,
        rejected: Option<&Arc<Credential>>,
    ) -> Result<Arc<Credential>, ClientError> {
        let version = self.client.session_config().client_version;
        let mut state = self.auth.lock().await;
        if let Some(c) = &state.credential
            && c.version == version
            && c.expires > Instant::now()
            && rejected.is_none_or(|r| !Arc::ptr_eq(r, c))
        {
            return Ok(c.clone());
        }
        if let Some((until, e)) = &state.failure
            && *until > Instant::now()
        {
            return Err(e.clone());
        }
        // Never reuse a rejected credential if refreshing fails.
        state.credential = None;
        match tokio::time::timeout(Duration::from_secs(10), self.discover(&version))
            .await
            .unwrap_or_else(|_| Err(err(ErrorKind::Timeout)))
        {
            Ok(c) => {
                let c = Arc::new(c);
                state.credential = Some(c.clone());
                state.failure = None;
                Ok(c)
            }
            Err(e) => {
                state.failure = Some((Instant::now() + Duration::from_secs(5), e.clone()));
                Err(e)
            }
        }
    }
    async fn discover(&self, version: &str) -> Result<Credential, ClientError> {
        let descriptors = moenotes_proto::pool_for_region("jp");
        let request = DynamicMessage::new(
            descriptors
                .get_message_by_name("app.masterdata.VersionRequest")
                .unwrap(),
        );
        let codec = DynamicCodec(
            descriptors
                .get_message_by_name("app.masterdata.VersionResponse")
                .unwrap(),
        );
        let mut grpc =
            tonic::client::Grpc::new(self.channel.clone()).max_decoding_message_size(65536);
        grpc.ready().await.map_err(|_| err(ErrorKind::Transport))?;
        let mut request = Request::new(request);
        let metadata = request.metadata_mut();
        metadata.insert("x-platform", "android".parse().unwrap());
        metadata.insert(
            "x-client-version",
            version.parse().map_err(|_| err(ErrorKind::InvalidConfig))?,
        );
        metadata.insert(
            "x-request-id",
            uuid::Uuid::new_v4().to_string().parse().unwrap(),
        );
        request.set_timeout(Duration::from_secs(10));
        let response = grpc
            .server_streaming(
                request,
                http::uri::PathAndQuery::from_static("/app.masterdata.MasterdataService/Version"),
                codec,
            )
            .await
            .map_err(|s| grpc_error(s, &MetadataMap::new()))?;
        let initial = response.metadata().clone();
        let mut stream = response.into_inner();
        if stream
            .message()
            .await
            .map_err(|s| grpc_error(s, &initial))?
            .is_none()
            || stream
                .message()
                .await
                .map_err(|s| grpc_error(s, &initial))?
                .is_some()
        {
            return Err(err(ErrorKind::Protocol));
        }
        let trailing = stream
            .trailers()
            .await
            .map_err(|s| grpc_error(s, &initial))?
            .unwrap_or_default();
        if let Some(e) = ClientError::from_metadata(tonic::Code::Ok, &initial, &trailing) {
            return Err(e);
        }
        credential_from_metadata(&initial, &trailing, version)
    }
}
fn grpc_error(status: tonic::Status, initial: &MetadataMap) -> ClientError {
    ClientError::from_metadata(status.code(), initial, status.metadata())
        .unwrap_or_else(|| err(ErrorKind::Protocol))
}
fn credential_from_metadata(
    initial: &MetadataMap,
    trailing: &MetadataMap,
    version: &str,
) -> Result<Credential, ClientError> {
    let header = |name: &'static str| {
        trailing
            .get_all(name)
            .iter()
            .next_back()
            .or_else(|| initial.get_all(name).iter().next_back())
            .and_then(|v| v.to_str().ok())
    };
    if header("x-sirius-env").map(|v| v.trim_end_matches('/')) != Some(CDN_ORIGIN) {
        return Err(err(ErrorKind::Protocol));
    }
    let password = header("x-sirius-cred")
        .filter(|v| !v.is_empty() && v.len() <= 4096 && !v.chars().any(char::is_control))
        .ok_or_else(|| err(ErrorKind::Protocol))?;
    let plain = Zeroizing::new(format!("sirius:{password}"));
    let authorization = Zeroizing::new(format!("Basic {}", STANDARD.encode(plain.as_bytes())));
    Ok(Credential {
        authorization,
        version: version.to_owned(),
        expires: Instant::now() + TTL,
    })
}
#[async_trait]
impl ImageSource for LiveSource {
    async fn fetch(&self, url: &str) -> Result<Bytes, ClientError> {
        let mut credential = self.credential(None).await?;
        for attempt in 0..2 {
            let mut header = HeaderValue::from_str(&credential.authorization)
                .map_err(|_| err(ErrorKind::Protocol))?;
            header.set_sensitive(true);
            let mut response = self
                .http
                .get(url)
                .header("authorization", header)
                .header("user-agent", format!("OurNotes/{}", credential.version))
                .send()
                .await
                .map_err(http_error)?;
            if matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) && attempt == 0
            {
                credential = self.credential(Some(&credential)).await?;
                continue;
            }
            if response.status() != StatusCode::OK {
                return Err(err(ErrorKind::Transport));
            }
            if response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.split(';').next().unwrap().trim())
                != Some("image/png")
                || response
                    .content_length()
                    .is_some_and(|n| n > MAX_IMAGE as u64)
            {
                return Err(err(ErrorKind::Protocol));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(http_error)? {
                if bytes.len() + chunk.len() > MAX_IMAGE {
                    return Err(err(ErrorKind::Protocol));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(Bytes::from(bytes));
        }
        Err(err(ErrorKind::Transport))
    }
}
fn http_error(error: reqwest::Error) -> ClientError {
    err(if error.is_timeout() {
        ErrorKind::Timeout
    } else {
        ErrorKind::Transport
    })
}

#[cfg(test)]
mod tests;
