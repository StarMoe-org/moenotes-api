//! Missing configuration permits liveness only, never anonymous business access.
use crate::config::{
    Config, check_inline_permissions, read_api_key, read_config, validate_api_key,
};
use axum::{Json, Router, http::StatusCode, middleware, routing::get};
use moenotes_client::{ClientError, ErrorKind};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};
use toml::Value;
use zeroize::Zeroizing;

const REQUIRED: &[&str] = &[
    "api_key_file",
    "session.region",
    "session.origin",
    "session.allowed_origins",
    "session.platform",
    "session.client_version",
];

pub enum Startup {
    Configured(Box<Config>),
    Unconfigured {
        listen: SocketAddr,
        missing: Vec<&'static str>,
    },
}
fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}

fn field<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |value, key| value.get(key))
}
fn string_missing(value: &Value, path: &str) -> Result<bool, ClientError> {
    match field(value, path) {
        None => Ok(true),
        Some(Value::String(s)) => Ok(s.trim().is_empty()),
        _ => Err(invalid()),
    }
}
fn file_missing(path: &Path) -> Result<bool, ClientError> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(_) => Err(invalid()),
    }
}
fn relative(value: &Value, key: &str, parent: &Path) -> Result<Option<PathBuf>, ClientError> {
    match field(value, key) {
        None => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => {
            let path = PathBuf::from(s);
            Ok(Some(if path.is_relative() {
                parent.join(path)
            } else {
                path
            }))
        }
        _ => Err(invalid()),
    }
}

pub fn inspect(path: &Path, fallback: SocketAddr) -> Result<Startup, ClientError> {
    let mut missing = Vec::new();
    let text = match read_config(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            missing.push("config_file");
            Zeroizing::new(String::new())
        }
        Err(_) => return Err(invalid()),
    };
    let value: Value = toml::from_str(&text).map_err(|_| invalid())?;
    for table in [
        "session",
        "login",
        "recovery",
        "accounts",
        "version_sync",
        "regions",
    ] {
        if value.get(table).is_some_and(|v| !v.is_table()) {
            return Err(invalid());
        }
    }
    if let Some(sync) = value.get("version_sync") {
        let sync: crate::config::VersionSyncConfig =
            sync.clone().try_into().map_err(|_| invalid())?;
        if !(30..=86400).contains(&sync.interval_seconds) {
            return Err(invalid());
        }
    }
    for table in ["login.context", "login.sdk_http", "login.sdk_http.common"] {
        if field(&value, table).is_some_and(|v| !v.is_table()) {
            return Err(invalid());
        }
    }
    for (inline, file) in [
        ("api_key", "api_key_file"),
        ("login.context", "login.context_file"),
        ("login.sdk_http", "login.sdk_http_file"),
    ] {
        if field(&value, inline).is_some() && field(&value, file).is_some() {
            return Err(invalid());
        }
    }
    check_inline_permissions(&value, path)?;
    let listen = match value.get("listen") {
        Some(Value::String(s)) => s.parse().map_err(|_| invalid())?,
        None => fallback,
        _ => return Err(invalid()),
    };
    for key in REQUIRED {
        if *key == "api_key_file" && value.get("api_key").is_some() {
            continue;
        }
        let absent = if *key == "session.allowed_origins" {
            match field(&value, key) {
                None => true,
                Some(Value::Array(v)) => {
                    if v.iter().any(|v| !v.is_str()) {
                        return Err(invalid());
                    }
                    v.is_empty() || v.iter().all(|v| v.as_str().unwrap().trim().is_empty())
                }
                _ => return Err(invalid()),
            }
        } else {
            string_missing(&value, key)?
        };
        if absent {
            missing.push(*key);
        }
    }
    if value.get("api_key").is_some() {
        if string_missing(&value, "api_key")? {
            missing.push("api_key");
        } else {
            validate_api_key(value["api_key"].as_str().ok_or_else(invalid)?)?;
        }
    }
    let jp_session = field(&value, "session.region").and_then(Value::as_str) == Some("jp");
    if value.get("login").is_some() || value.get("accounts").is_some() && !jp_session {
        for key in ["login.context_file", "login.state_dir"] {
            if key == "login.context_file" && field(&value, "login.context").is_some() {
                continue;
            }
            if string_missing(&value, key)? {
                missing.push(key);
            }
        }
    }
    if !jp_session
        && value.get("accounts").is_some()
        && field(&value, "login.sdk_http").is_none()
        && string_missing(&value, "login.sdk_http_file")?
    {
        missing.push("login.sdk_http_file");
    }
    if field(&value, "login.context").is_some() {
        for key in [
            "login.context.device_model",
            "login.context.operating_system",
            "login.context.device_identifier",
        ] {
            if string_missing(&value, key)? {
                missing.push(key);
            }
        }
        for key in [
            "login.context.global_channel_id",
            "login.context.brand_id",
            "login.context.area_id",
        ] {
            match field(&value, key) {
                None => missing.push(key),
                Some(Value::Integer(n)) if u32::try_from(*n).is_ok() => {}
                _ => return Err(invalid()),
            }
        }
    }
    if field(&value, "login.sdk_http").is_some() {
        for key in ["login.sdk_http.base_url", "login.sdk_http.app_key"] {
            if string_missing(&value, key)? {
                missing.push(key);
            }
        }
        match field(&value, "login.sdk_http.country_id") {
            None => missing.push("login.sdk_http.country_id"),
            Some(Value::Integer(n)) if u32::try_from(*n).is_ok() => {}
            _ => return Err(invalid()),
        }
        match field(&value, "login.sdk_http.allowed_base_urls") {
            None => missing.push("login.sdk_http.allowed_base_urls"),
            Some(Value::Array(items)) if items.iter().all(Value::is_str) => {
                if items.is_empty() || items.iter().all(|v| v.as_str().unwrap().trim().is_empty()) {
                    missing.push("login.sdk_http.allowed_base_urls");
                }
            }
            _ => return Err(invalid()),
        }
        let mut supplied = std::collections::BTreeSet::new();
        if let Some(Value::Table(common)) = field(&value, "login.sdk_http.common") {
            for (key, value) in common {
                if !value.is_str() || !supplied.insert(key.as_str()) {
                    return Err(invalid());
                }
            }
        }
        if let Some(omit) = field(&value, "login.sdk_http.omit_common") {
            let items = omit.as_array().ok_or_else(invalid)?;
            for item in items {
                if !supplied.insert(item.as_str().ok_or_else(invalid)?) {
                    return Err(invalid());
                }
            }
        }
        let expected = moenotes_client::sdk_http::COMMON_FIELDS;
        if supplied.iter().any(|key| !expected.contains(key)) {
            return Err(invalid());
        }
        if expected.iter().any(|key| !supplied.contains(key)) {
            missing.push("login.sdk_http.common");
        }
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    for key in [
        "api_key_file",
        "credentials_file",
        "login.context_file",
        "login.sdk_http_file",
        "login.state_dir",
    ] {
        if let Some(path) = relative(&value, key, parent)? {
            if file_missing(&path)? {
                if !missing.contains(&key) {
                    missing.push(key);
                }
            } else if key == "api_key_file" {
                let key = read_api_key(&path)?;
                if key.is_empty() {
                    missing.push("api_key_file.content");
                } else {
                    validate_api_key(&key)?;
                }
            }
        } else if matches!(key, "credentials_file" | "login.sdk_http_file")
            && field(&value, key).is_some()
        {
            missing.push(key);
        }
    }
    if !missing.is_empty() {
        return Ok(Startup::Unconfigured { listen, missing });
    }
    Ok(Startup::Configured(Box::new(Config::from_text(
        &text, path,
    )?)))
}

pub fn health_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/healthz", get(health))
        .route(
            "/readyz",
            get(|| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"ready":false,"configured":false})),
                )
            }),
        )
        .fallback(|| async {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":{"kind":"unconfigured"}})),
            )
        })
        .layer(middleware::map_response(
            |mut response: axum::response::Response| async move {
                response
                    .headers_mut()
                    .insert("cache-control", "no-store".parse().unwrap());
                response.headers_mut().insert(
                    "x-request-id",
                    uuid::Uuid::new_v4().to_string().parse().unwrap(),
                );
                response
            },
        ))
}
pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok"}))
}

pub async fn serve(listen: SocketAddr, missing: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!(
        "{}",
        serde_json::json!({"event":"configuration_missing","mode":"health_only","missing":missing,"action":"Provide the missing configuration and restart; upstream checks and business routes are disabled."})
    );
    let listener = tokio::net::TcpListener::bind(listen).await?;
    println!(
        "moenotes-api listening on {} (configuration missing; health only)",
        listener.local_addr()?
    );
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    axum::serve(listener,health_router()).with_graceful_shutdown(async move{
        #[cfg(unix)]loop{tokio::select!{
            _=tokio::signal::ctrl_c()=>break,
            _=term.recv()=>break,
            _=hup.recv()=>eprintln!("{{\"event\":\"configuration_reload\",\"status\":\"restart_required\"}}"),
        }}
        #[cfg(not(unix))]let _=tokio::signal::ctrl_c().await;
    }).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::fs;
    use tower::ServiceExt;

    const COMPLETE: &str = r#"api_key_file="key"
[session]
region="test"
origin="https://game.invalid"
allowed_origins=["https://game.invalid"]
platform="android"
client_version="1"
"#;
    fn setup() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            dir.path().join("key"),
            "synthetic-bootstrap-api-key-not-for-production",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path().join("key"), fs::Permissions::from_mode(0o600)).unwrap();
        }
        (dir, path)
    }
    fn missing(path: &Path) -> (SocketAddr, Vec<&'static str>) {
        match inspect(path, "127.0.0.1:0".parse().unwrap()).unwrap() {
            Startup::Unconfigured { listen, missing } => (listen, missing),
            Startup::Configured(_) => panic!("expected unconfigured"),
        }
    }
    #[test]
    fn missing_empty_and_partial_fields_are_reported_together() {
        let (_dir, path) = setup();
        let (_, fields) = missing(&path);
        assert!(fields.contains(&"config_file"));
        for key in REQUIRED {
            assert!(fields.contains(key));
        }
        fs::write(&path, "").unwrap();
        assert_eq!(missing(&path).1, REQUIRED);
        fs::write(&path,"listen=\"127.0.0.1:4567\"\napi_key_file=\"key\"\n[session]\nregion=\"test\"\norigin=\"\"\nallowed_origins=[]\n").unwrap();
        let (listen, fields) = missing(&path);
        assert_eq!(listen.port(), 4567);
        assert_eq!(
            fields,
            vec![
                "session.origin",
                "session.allowed_origins",
                "session.platform",
                "session.client_version"
            ]
        );
    }
    #[test]
    fn missing_files_and_enabled_login_fields_are_named_without_values() {
        let (dir, path) = setup();
        fs::remove_file(dir.path().join("key")).unwrap();
        fs::write(
            &path,
            format!("credentials_file=\"absent.json\"\n{COMPLETE}"),
        )
        .unwrap();
        assert_eq!(missing(&path).1, vec!["api_key_file", "credentials_file"]);
        fs::write(
            &path,
            format!("{COMPLETE}\n[login]\ncontext_file=\"absent-device.json\"\n"),
        )
        .unwrap();
        let fields = missing(&path).1;
        assert!(fields.contains(&"login.context_file") && fields.contains(&"login.state_dir"));
        assert!(!fields.iter().any(|key| key.contains("absent")));
    }
    #[test]
    fn empty_key_is_missing_and_complete_config_keeps_normal_listen_default() {
        let (dir, path) = setup();
        fs::write(&path, COMPLETE).unwrap();
        let Startup::Configured(config) = inspect(&path, "0.0.0.0:9876".parse().unwrap()).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            config.listen,
            "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
        );
        fs::write(dir.path().join("key"), "\n").unwrap();
        assert_eq!(missing(&path).1, vec!["api_key_file.content"]);
    }
    #[test]
    fn malformed_config_and_invalid_key_fail_closed() {
        let (dir, path) = setup();
        for text in [
            "[broken",
            "listen=\"invalid\"",
            "session=7",
            "api_key_file=12",
            "[session]\nallowed_origins=[12]",
        ] {
            fs::write(&path, text).unwrap();
            assert!(inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
        }
        fs::write(&path, COMPLETE).unwrap();
        fs::write(dir.path().join("key"), "short").unwrap();
        assert!(inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(
                dir.path().join("key"),
                "synthetic-bootstrap-api-key-not-for-production",
            )
            .unwrap();
            fs::set_permissions(dir.path().join("key"), fs::Permissions::from_mode(0o644)).unwrap();
            assert!(inspect(&path, "127.0.0.1:0".parse().unwrap()).is_err());
        }
    }
    #[tokio::test]
    async fn health_only_has_no_business_or_diagnostic_data() {
        for (path, status) in [
            ("/health", 200),
            ("/healthz", 200),
            ("/readyz", 503),
            ("/v1/profile?playerProfileId=123", 503),
            ("/v1/status", 503),
            ("/openapi.json", 503),
        ] {
            let response = health_router()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert!(response.headers().contains_key("x-request-id"));
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if status == 200 {
                assert_eq!(value, serde_json::json!({"status":"ok"}));
            }
            assert!(!String::from_utf8_lossy(&bytes).contains("api_key_file"));
        }
    }
}
