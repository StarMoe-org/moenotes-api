//! Readable aliases use the same query validation, projection and cache as v1.
use crate::{ApiState, HttpError, projection, query_params, regions::Region, unix_millis};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use moenotes_client::{ClientError, ErrorKind};
use std::collections::HashMap;

pub(crate) struct Route {
    pub path: &'static str,
    pub method: &'static str,
    /// (path parameter, protobuf query field, comma-separated list).
    pub fields: &'static [(&'static str, &'static str, bool)],
}
pub(crate) const ROUTES: &[Route] = &[
    Route {
        path: "/v1/profile/{profileId}",
        method: "profile",
        fields: &[("profileId", "playerProfileId", false)],
    },
    Route {
        path: "/v1/profiles/{accountIds}",
        method: "profiles",
        fields: &[("accountIds", "accountIds", true)],
    },
    Route {
        path: "/v1/player/{playerId}/favorites",
        method: "favorite-status",
        fields: &[("playerId", "playerId", false)],
    },
    Route {
        path: "/v1/announcements/{selectedTab}",
        method: "announcements",
        fields: &[("selectedTab", "selectedTab", false)],
    },
    Route {
        path: "/v1/announcement/{id}",
        method: "announcement",
        fields: &[("id", "id", false)],
    },
    Route {
        path: "/v1/gacha/{gachaId}/rates",
        method: "probability",
        fields: &[("gachaId", "gachaId", false)],
    },
    Route {
        path: "/v1/gacha/{gachaId}/rates/{selectedPickUp}",
        method: "probability",
        fields: &[
            ("gachaId", "gachaId", false),
            ("selectedPickUp", "selectedPickUp", true),
        ],
    },
    Route {
        path: "/v1/music/{musicId}/ranking",
        method: "music-ranking",
        fields: &[("musicId", "musicId", false)],
    },
    Route {
        path: "/v1/event/{eventId}/ranking/{ranks}",
        method: "event-ranking",
        fields: &[("eventId", "eventId", false), ("ranks", "ranks", true)],
    },
    Route {
        path: "/v1/event/challenge/{challengeMusicId}/ranking",
        method: "challenge-ranking",
        fields: &[("challengeMusicId", "challengeMusicId", false)],
    },
    Route {
        path: "/v1/event/{eventId}/deck/{playerId}",
        method: "event-deck",
        fields: &[
            ("eventId", "eventId", false),
            ("playerId", "playerId", false),
        ],
    },
    Route {
        path: "/v1/arena/{arenaSeasonId}/ranking/{rankingStart}/{rankingEnd}",
        method: "arena-ranking",
        fields: &[
            ("arenaSeasonId", "arenaSeasonId", false),
            ("rankingStart", "rankingStart", false),
            ("rankingEnd", "rankingEnd", false),
        ],
    },
    Route {
        path: "/v1/arena/{arenaSeasonId}/ranking/{rankingStart}/{rankingEnd}/band/{bandId}",
        method: "arena-ranking",
        fields: &[
            ("arenaSeasonId", "arenaSeasonId", false),
            ("rankingStart", "rankingStart", false),
            ("rankingEnd", "rankingEnd", false),
            ("bandId", "bandId", false),
        ],
    },
    Route {
        path: "/v1/arena/{arenaSeasonId}/music/{musicId}/deck-trend",
        method: "deck-trend",
        fields: &[
            ("arenaSeasonId", "arenaSeasonId", false),
            ("musicId", "musicId", false),
        ],
    },
    Route {
        path: "/v1/circle/{circleId}",
        method: "circle",
        fields: &[("circleId", "circleId", false)],
    },
    Route {
        path: "/v1/circles/search/{name}",
        method: "circle-search",
        fields: &[("name", "options.name", false)],
    },
];

pub(crate) fn method_for_path(path: &str) -> Option<&'static str> {
    let unscoped = ["/v1/tw", "/v1/en", "/v1/kr"]
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix));
    let path = unscoped
        .map(|suffix| format!("/v1{suffix}"))
        .unwrap_or_else(|| path.to_owned());
    ROUTES
        .iter()
        .find(|route| route.path == path)
        .map(|route| route.method)
        .or_else(|| {
            crate::ROUTES
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, method)| *method)
        })
}

fn valid_path(path: &str) -> bool {
    path.len() <= 8192
        && path.bytes().enumerate().all(|(i, b)| {
            b != b'%'
                || path
                    .as_bytes()
                    .get(i + 1..i + 3)
                    .is_some_and(|v| v.iter().all(u8::is_ascii_hexdigit))
        })
}

pub(crate) fn install(mut router: Router<ApiState>) -> Router<ApiState> {
    for route in ROUTES {
        for region in [None, Some(Region::Tw), Some(Region::En), Some(Region::Kr)] {
            let path = region
                .map(|r| format!("/v1/{}{}", r.as_str(), route.path.trim_start_matches("/v1")))
                .unwrap_or_else(|| route.path.to_owned());
            router = router.route(&path, get(move |State(state):State<ApiState>, OriginalUri(uri):OriginalUri, params:Result<Path<HashMap<String,String>>,axum::extract::rejection::PathRejection>, RawQuery(raw):RawQuery, body:Result<Bytes,axum::extract::rejection::BytesRejection>| async move {
                if !valid_path(uri.path()) { return invalid().into_response(); }
                let Ok(Path(params)) = params else { return invalid().into_response(); };
                if !matches!(body, Ok(ref bytes) if bytes.is_empty()) { return invalid().into_response(); }
                let raw = match path_query(route, &params, raw.as_deref()) { Ok(raw)=>raw, Err(error)=>return HttpError(error).into_response() };
                let selected = if route.method == "profile" {
                    let Some(id)=params.get("profileId") else { return invalid().into_response(); };
                    let inferred = match Region::from_profile_id(id) { Ok((r,_))=>r, Err(kind)=>return error(StatusCode::BAD_REQUEST,kind) };
                    if region.is_some_and(|r|r!=inferred) { return error(StatusCode::BAD_REQUEST,"profile_region_mismatch"); }
                    Some(inferred)
                } else { region };
                execute(state,route.method,Some(&raw),selected).await
            }).head(method_not_allowed));
        }
    }
    // Explicit regions also support the established query-form endpoints and
    // routes that already need no parameters (announcements, circle discovery).
    for &(path, method) in crate::ROUTES {
        for region in [Region::Tw, Region::En, Region::Kr] {
            let path = format!("/v1/{}{}", region.as_str(), path.trim_start_matches("/v1"));
            router = router.route(&path,get(move |State(state):State<ApiState>,RawQuery(raw):RawQuery,body:Result<Bytes,axum::extract::rejection::BytesRejection>| async move {
                if !matches!(body, Ok(ref bytes) if bytes.is_empty()) { return invalid().into_response(); }
                execute(state,method,raw.as_deref(),Some(region)).await
            }).head(method_not_allowed));
        }
    }
    router
}

async fn method_not_allowed() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [("allow", "GET"), ("cache-control", "no-store")],
    )
}
fn invalid() -> HttpError {
    HttpError(ClientError::new(ErrorKind::InvalidRequest))
}
fn error(status: StatusCode, kind: &'static str) -> Response {
    (status, Json(serde_json::json!({"error":{"kind":kind}}))).into_response()
}

pub(crate) fn path_query(
    route: &Route,
    params: &HashMap<String, String>,
    raw: Option<&str>,
) -> Result<String, ClientError> {
    let raw = raw.unwrap_or_default();
    if raw.len() > 8192 {
        return Err(invalid().0);
    }
    for (key, _) in url::form_urlencoded::parse(raw.as_bytes()) {
        if route.fields.iter().any(|(_, field, _)| *field == key) {
            return Err(invalid().0);
        }
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (path, field, list) in route.fields {
        let value = params.get(*path).ok_or_else(|| invalid().0)?;
        if value.is_empty() || value.len() > 8192 || value.chars().any(char::is_control) {
            return Err(invalid().0);
        }
        if *list {
            let values: Vec<_> = value.split(',').collect();
            if values.len() > 100 || values.iter().any(|v| v.is_empty()) {
                return Err(invalid().0);
            }
            for value in values {
                serializer.append_pair(field, value);
            }
        } else {
            serializer.append_pair(field, value);
        }
    }
    let mut result = serializer.finish();
    if !raw.is_empty() {
        result.push('&');
        result.push_str(raw);
    }
    if result.len() > 8192 {
        return Err(invalid().0);
    }
    Ok(result)
}

async fn execute(
    state: ApiState,
    method: &'static str,
    raw: Option<&str>,
    region: Option<Region>,
) -> Response {
    if !projection::permitted(state.mode, method) {
        return error(StatusCode::FORBIDDEN, "response_policy");
    }
    let query = match query_params::parse(method, raw) {
        Ok(q) => q,
        Err(e) => return HttpError(e).into_response(),
    };
    let cache = if let Some(region) = region {
        match state.profiles.cache(region) {
            Some(cache) => cache,
            None => return error(StatusCode::SERVICE_UNAVAILABLE, "region_unconfigured"),
        }
    } else {
        &state.cache
    };
    let (response, status) = match cache.query(query).await {
        Ok(r) => r,
        Err(e) => return HttpError(e).into_response(),
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-moenotes-cache", status.parse().unwrap());
    headers.insert(
        "x-moenotes-fetched-at",
        unix_millis(response.fetched_at)
            .to_string()
            .parse()
            .unwrap(),
    );
    if let Some(region) = region {
        headers.insert("x-moenotes-region", region.as_str().parse().unwrap());
    }
    let json = if state.mode == projection::ResponseMode::Public {
        match projection::project(method, &response.json) {
            Ok(json) => json,
            Err(e) => return HttpError(e).into_response(),
        }
    } else {
        response.json.clone()
    };
    (headers, Json(json)).into_response()
}
