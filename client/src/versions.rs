use crate::{CancellationToken, Client, ClientError, ErrorKind, Generation, Query, Session};
use prost::Message;
use prost_reflect::DynamicMessage;
use serde::Serialize;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

/// Public data revision identifiers; never account credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DataVersions {
    pub master_version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resource_version: String,
}

fn valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !matches!(value, "." | "..")
}

/// `MAJOR.MINOR.PATCH` with decimal parts, e.g. the store version `1.0.4`.
fn patch_version(value: &str) -> Option<[u32; 3]> {
    let mut parts = value.split('.');
    let mut out = [0; 3];
    for part in &mut out {
        let text = parts.next()?;
        if text.is_empty() || text.len() > 9 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *part = text.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// The next `count` patch releases after `current` (`1.0.3` -> `1.0.4`, `1.0.5`, ...), for following a
/// client update. Empty when `current` is not `MAJOR.MINOR.PATCH`; minor and major releases are never guessed.
pub fn patch_successors(current: &str, count: u32) -> Vec<String> {
    let Some([major, minor, patch]) = patch_version(current) else {
        return Vec::new();
    };
    (1..=count)
        .filter_map(|step| patch.checked_add(step))
        .map(|next| format!("{major}.{minor}.{next}"))
        .collect()
}

impl Client {
    /// Current configuration, including the last successfully discovered versions.
    pub fn session_config(&self) -> crate::SessionConfig {
        self.session.read().unwrap().config.clone()
    }

    /// One anonymous Version RPC. Atomically install a complete version pair while
    /// retaining identity and credentials; cancel old work and invalidate its cache.
    /// Only an explicit master mismatch can be cleared. No login or query replay.
    pub async fn refresh_versions(
        &self,
        generation: Generation,
        cancel: CancellationToken,
    ) -> Result<bool, ClientError> {
        self.discover_versions(generation, cancel, None).await
    }

    /// One anonymous Version RPC presenting `candidate` as the client version. Only when
    /// the game accepts it are the candidate and the versions it returned installed
    /// together, which also clears a version block: the game asked for this client.
    /// A rejected candidate (ErrorKind::Version) or any other failure changes nothing.
    /// No login or query replay. Callers choose candidates, e.g. patch_successors.
    pub async fn adopt_client_version(
        &self,
        generation: Generation,
        candidate: &str,
        cancel: CancellationToken,
    ) -> Result<bool, ClientError> {
        if patch_version(candidate).is_none() {
            return Err(ClientError::new(ErrorKind::InvalidRequest));
        }
        self.discover_versions(generation, cancel, Some(candidate.to_owned()))
            .await
    }

    async fn discover_versions(
        &self,
        generation: Generation,
        cancel: CancellationToken,
        client_version: Option<String>,
    ) -> Result<bool, ClientError> {
        let session = self.session_for(generation)?;
        let mut presented = session.config.clone();
        if let Some(candidate) = client_version {
            presented.client_version = candidate;
        }
        let query = Query::Version(Default::default());
        let method = query.method_for_region(&presented.region)?;
        let request = DynamicMessage::new(
            moenotes_proto::pool_for_region(&presented.region)
                .get_message_by_name(method.input)
                .unwrap(),
        );
        let metadata = presented.metadata(None, &Generation::new_v4().to_string(), true)?;
        self.run_operation_response(
            session.clone(),
            method,
            request,
            metadata,
            cancel.clone(),
            |result| {
                let message = result.message;
                let response = crate::generated::app::masterdata::VersionResponse::decode(
                    message.encode_to_vec().as_slice(),
                )
                .map_err(|_| ClientError::new(ErrorKind::Protocol))?;
                let resource = if session.config.region == "jp" {
                    crate::jp::asset_version(
                        result.asset_version.as_deref(),
                        &presented.client_version,
                    )
                } else {
                    Some(response.resource_version.clone())
                };
                if !(if session.config.region == "jp" {
                    crate::jp::valid_master(&response.version)
                } else {
                    valid(&response.version)
                }) || (session.config.region != "jp" && !valid(&response.resource_version))
                {
                    return Err(ClientError::new(ErrorKind::Protocol));
                }
                let mut current = self.session.write().unwrap();
                if current.generation != generation {
                    return Err(ClientError::new(ErrorKind::SessionChanged));
                }
                if cancel.is_cancelled() {
                    return Err(ClientError::new(ErrorKind::Cancelled));
                }
                if current.config.master_version.as_ref() == Some(&response.version)
                    && current.config.resource_version == resource
                    && current.config.client_version == presented.client_version
                {
                    return Ok(false);
                }
                let mut config = current.config.clone();
                config.master_version = Some(response.version);
                config.resource_version = resource;
                let client_changed = config.client_version != presented.client_version;
                config.client_version = presented.client_version;
                let mut blocked = *current.blocked.read().unwrap();
                if blocked == Some(ErrorKind::Version)
                    && (client_changed || current.master_mismatch.load(Ordering::SeqCst))
                {
                    blocked = None;
                }
                let new = Arc::new(Session {
                    generation: Generation::new_v4(),
                    config,
                    credentials: current.credentials.clone(),
                    transport: current.transport.clone(),
                    cancel: CancellationToken::new(),
                    blocked: RwLock::new(blocked),
                    master_mismatch: AtomicBool::new(false),
                    jp_override_pending: AtomicBool::new(
                        current.jp_override_pending.load(Ordering::SeqCst),
                    ),
                });
                current.cancel.cancel();
                *current = new;
                Ok(true)
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::patch_successors;

    #[test]
    fn patch_successors_only_guess_patch_releases() {
        assert_eq!(patch_successors("1.0.3", 3), ["1.0.4", "1.0.5", "1.0.6"]);
        assert_eq!(patch_successors("2.10.99", 1), ["2.10.100"]);
        for unusable in [
            "1",
            "1.0",
            "1.0.0.1",
            "1.0.x",
            "1..3",
            "",
            "01.0.3a",
            "1.0.9999999999",
        ] {
            assert!(patch_successors(unusable, 3).is_empty(), "{unusable}");
        }
    }
}
