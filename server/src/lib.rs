//! Experimental HTTP gateway. Raw account-dependent data is opt-in, not public-safe.
pub mod accounts;
pub mod cache;
pub mod config;
pub mod config_file;
mod diagnostics;
pub mod jp_operator;
pub mod managed;
mod openapi;
pub mod operator;
mod path_routes;
pub mod pool;
pub mod projection;
mod query_params;
pub mod regions;
pub mod runtime;
pub mod secret;
pub mod startup;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, RawQuery, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use cache::CacheOptions;
use moenotes_client::{CancellationToken, ClientError, ErrorKind, QueryClient};
use pool::{SessionMember, SessionPool};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub const ROUTES: &[(&str, &str)] = &[
    ("/v1/announcement", "announcement"),
    ("/v1/announcements", "announcements"),
    ("/v1/arena/ranking", "arena-ranking"),
    ("/v1/arena/deck-trend", "deck-trend"),
    ("/v1/circle", "circle"),
    ("/v1/circles/recommended", "circle-recommendations"),
    ("/v1/circles/search", "circle-search"),
    ("/v1/event/challenge-ranking", "challenge-ranking"),
    ("/v1/event/deck", "event-deck"),
    ("/v1/event/ranking", "event-ranking"),
    ("/v1/profile", "profile"),
    ("/v1/gacha/rates", "probability"),
    ("/v1/music/ranking", "music-ranking"),
    ("/v1/profile/favorites", "favorite-status"),
    ("/v1/profiles", "profiles"),
];

pub fn openapi_document(mode: projection::ResponseMode) -> serde_json::Value {
    openapi::document_for(mode)
}

#[derive(Clone)]
struct ApiState {
    cache: Arc<SessionPool>,
    key_hash: [u8; 32],
    admission: Arc<tokio::sync::Semaphore>,
    mode: projection::ResponseMode,
    diagnostics: Arc<diagnostics::Diagnostics>,
    access_log: bool,
    profiles: Arc<regions::RegionClients>,
}

pub fn router(
    client: Arc<dyn QueryClient>,
    api_key: Zeroizing<String>,
    enable_raw: bool,
    options: CacheOptions,
    stop: CancellationToken,
) -> Result<Router, ClientError> {
    router_with_options(
        client,
        api_key,
        RouterOptions {
            mode: if enable_raw {
                projection::ResponseMode::Raw
            } else {
                projection::ResponseMode::Disabled
            },
            managed: None,
            access_log: false,
        },
        options,
        stop,
    )
}

pub struct RouterOptions {
    pub mode: projection::ResponseMode,
    pub managed: Option<Arc<managed::ManagedClient>>,
    pub access_log: bool,
}

pub fn router_with_options(
    client: Arc<dyn QueryClient>,
    api_key: Zeroizing<String>,
    router_options: RouterOptions,
    options: CacheOptions,
    stop: CancellationToken,
) -> Result<Router, ClientError> {
    router_with_regions(
        client,
        api_key,
        router_options,
        options,
        stop,
        regions::RegionClients::default(),
    )
}

pub fn router_with_regions(
    client: Arc<dyn QueryClient>,
    api_key: Zeroizing<String>,
    router_options: RouterOptions,
    options: CacheOptions,
    stop: CancellationToken,
    profiles: regions::RegionClients,
) -> Result<Router, ClientError> {
    options.validate()?;
    if api_key.len() < 32
        || api_key.len() > 4096
        || !api_key.is_ascii()
        || api_key
            .bytes()
            .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
    {
        return Err(ClientError::new(ErrorKind::InvalidConfig));
    }
    let cache = match profiles.default_cache() {
        Some(cache) => cache,
        None => SessionPool::new(
            vec![SessionMember {
                client,
                managed: router_options.managed,
            }],
            options,
            stop,
        )?,
    };
    let state = ApiState {
        cache,
        key_hash: Sha256::digest(api_key.as_bytes()).into(),
        admission: Arc::new(tokio::sync::Semaphore::new(64)),
        mode: router_options.mode,
        diagnostics: Arc::new(diagnostics::Diagnostics::default()),
        access_log: router_options.access_log,
        profiles: Arc::new(profiles),
    };
    let mode = state.mode;
    let mut protected = Router::new().route("/openapi.json", get(move || async move { Json(openapi::document_for(mode)) }))
        .route("/readyz",get(|State(state):State<ApiState>|async move {
            let ready=state.cache.ready();
            (if ready {StatusCode::OK}else{StatusCode::SERVICE_UNAVAILABLE},Json(serde_json::json!({"ready":ready})))
        }))
        .route("/v1/status",get(|State(state):State<ApiState>|async move {Json(serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"responseMode":state.mode,"session":state.cache.single_status(),"pool":state.cache.status(),"regions":state.profiles.status(),"diagnostics":state.diagnostics.snapshot()}))}));
    if mode != projection::ResponseMode::Disabled {
        protected = path_routes::install(protected);
        for &(path, name) in ROUTES {
            protected = protected.route(
                path,
                get(
                    move |State(state): State<ApiState>,
                          RawQuery(raw): RawQuery,
                          body: Result<
                        Bytes,
                        axum::extract::rejection::BytesRejection,
                    >| async move {
                        let body = body
                            .map_err(|_| HttpError(ClientError::new(ErrorKind::InvalidRequest)))?;
                        if !body.is_empty() {
                            return Err(HttpError(ClientError::new(ErrorKind::InvalidRequest)));
                        }
                        if !projection::permitted(state.mode,name){return Ok((StatusCode::FORBIDDEN,[("cache-control","no-store")],Json(serde_json::json!({"error":{"kind":"response_policy"}}))).into_response());}
                        let query = query_params::parse(name, raw.as_deref()).map_err(HttpError)?;
                        let (response, status) =
                            state.cache.query(query).await.map_err(HttpError)?;
                        let mut headers = HeaderMap::new();
                        headers.insert("cache-control", "no-store".parse().unwrap());
                        headers.insert("x-moenotes-cache", status.parse().unwrap());
                        headers.insert(
                            "x-moenotes-fetched-at",
                            unix_millis(response.fetched_at)
                                .to_string()
                                .parse()
                                .unwrap(),
                        );
                        let json=if state.mode==projection::ResponseMode::Public {projection::project(name,&response.json).map_err(HttpError)?}else{response.json.clone()};
                        Ok::<_, HttpError>((headers, Json(json)).into_response())
                    },
                ).head(|| async { (StatusCode::METHOD_NOT_ALLOWED, [("allow", "GET"), ("cache-control", "no-store")]) }),
            );
        }
    }
    protected = protected
        .layer(DefaultBodyLimit::max(0))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Ok(Router::new()
        .route("/health", get(startup::health))
        .route("/healthz", get(startup::health))
        .merge(protected)
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, diagnose)))
}

async fn diagnose(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let _guard = state.diagnostics.begin();
    let start = Instant::now();
    let id = uuid::Uuid::new_v4().to_string();
    let matched = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|path| path.as_str());
    let route = ROUTES
        .iter()
        .find(|(path, _)| *path == request.uri().path())
        .map(|(_, name)| *name)
        .or_else(|| matched.and_then(path_routes::method_for_path))
        .unwrap_or("support_or_unknown");
    let mut response = next.run(request).await;
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        state.diagnostics.error();
    }
    response
        .headers_mut()
        .insert("x-request-id", id.parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    if state.access_log {
        eprintln!(
            "{}",
            serde_json::json!({"event":"http_request","request_id":id,"route":route,"status":status.as_u16(),"duration_ms":start.elapsed().as_millis()})
        );
    }
    response
}

async fn authenticate(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let values = request
        .headers()
        .get_all("authorization")
        .iter()
        .collect::<Vec<_>>();
    let provided = if values.len() == 1 {
        values[0]
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
    } else {
        None
    };
    let valid = provided.filter(|v| v.len() <= 4096).is_some_and(|v| {
        let hash: [u8; 32] = Sha256::digest(v.as_bytes()).into();
        bool::from(hash.ct_eq(&state.key_hash))
    });
    if !valid {
        return (
            StatusCode::UNAUTHORIZED,
            [
                ("cache-control", "no-store"),
                ("www-authenticate", "Bearer"),
            ],
            Json(serde_json::json!({"error":{"kind":"unauthorized"}})),
        )
            .into_response();
    }
    let Ok(_permit) = state.admission.try_acquire() else {
        return HttpError(ClientError::new(ErrorKind::QueueFull)).into_response();
    };
    next.run(request).await
}

struct HttpError(ClientError);
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.kind {
            ErrorKind::InvalidRequest => StatusCode::BAD_REQUEST,
            ErrorKind::QueueFull => StatusCode::TOO_MANY_REQUESTS,
            ErrorKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
            ErrorKind::Maintenance
            | ErrorKind::Authentication
            | ErrorKind::AuthenticationRequired
            | ErrorKind::Version
            | ErrorKind::DeviceConflict
            | ErrorKind::SessionChanged
            | ErrorKind::Cancelled => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_GATEWAY,
        };
        (
            status,
            [("cache-control", "no-store")],
            Json(serde_json::json!({"error":{"kind":self.0.kind}})),
        )
            .into_response()
    }
}

fn unix_millis(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests;
