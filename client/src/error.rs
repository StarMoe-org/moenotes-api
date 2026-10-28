use serde::Serialize;
use tonic::{Code, metadata::MetadataMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidConfig,
    InvalidRequest,
    AuthenticationRequired,
    Authentication,
    Maintenance,
    Version,
    DeviceConflict,
    Business,
    Transport,
    Protocol,
    Timeout,
    Cancelled,
    QueueFull,
    SessionChanged,
}

/// Safe to display: no upstream status text, headers or account identifiers.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{kind:?}")]
pub struct ClientError {
    pub kind: ErrorKind,
    pub grpc_code: Option<Code>,
    // Kept private to prevent accidental Debug/log disclosure of arbitrary values.
    business_codes: PrivateCodes,
    recommended_client_version: PrivateRecommended,
}

#[derive(Clone, Default)]
struct PrivateCodes {
    initial: Vec<String>,
    trailing: Vec<String>,
}

impl std::fmt::Debug for PrivateCodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// The release the game names in `x-client-recommended-version`; untrusted input.
#[derive(Clone, Default)]
struct PrivateRecommended(Option<String>);

impl std::fmt::Debug for PrivateRecommended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl ClientError {
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            grpc_code: None,
            business_codes: PrivateCodes::default(),
            recommended_client_version: PrivateRecommended::default(),
        }
    }

    /// Explicit diagnostics only. Values remain untrusted and must not be logged.
    pub fn business_codes(&self) -> (&[String], &[String]) {
        (&self.business_codes.initial, &self.business_codes.trailing)
    }

    /// The client release the game asked for, when it named one. Untrusted; callers
    /// must validate it before presenting it as `x-client-version`.
    pub fn recommended_client_version(&self) -> Option<&str> {
        self.recommended_client_version.0.as_deref()
    }

    pub fn from_metadata(
        code: Code,
        initial: &MetadataMap,
        trailing: &MetadataMap,
    ) -> Option<Self> {
        fn codes(map: &MetadataMap) -> Vec<String> {
            map.get_all("x-sirius-error-code")
                .iter()
                .filter_map(|v| v.to_str().ok().map(str::to_owned))
                .collect()
        }
        fn release(map: &MetadataMap) -> Option<&str> {
            map.get_all("x-client-recommended-version")
                .iter()
                .next_back()
                .and_then(|v| v.to_str().ok())
                .filter(|v| crate::versions::patch_version(v).is_some())
        }
        let codes = PrivateCodes {
            initial: codes(initial),
            trailing: codes(trailing),
        };
        let recommended_client_version = PrivateRecommended(
            release(trailing)
                .or_else(|| release(initial))
                .map(str::to_owned),
        );
        let last = codes.trailing.last().or(codes.initial.last());
        if code == Code::Ok && last.is_none() {
            return None;
        }
        let kind = match last.map(String::as_str) {
            Some("UNDER_MAINTENANCE") => ErrorKind::Maintenance,
            Some("TOKEN_ILLEGAL" | "TOKEN_MISSING") => ErrorKind::Authentication,
            Some("CLIENT_UPDATE_REQUIRED" | "MASTER_VERSION_MISMATCH") => ErrorKind::Version,
            Some("CONCURRENT_DEVICE") => ErrorKind::DeviceConflict,
            Some(_) => ErrorKind::Business,
            None if code == Code::DeadlineExceeded => ErrorKind::Timeout,
            None => ErrorKind::Transport,
        };
        Some(Self {
            kind,
            grpc_code: Some(code),
            business_codes: codes,
            recommended_client_version,
        })
    }
}
