//! Explicit JP registration and credential import support. No automatic registration or transfer.
use crate::{
    CancellationToken, Client, ClientError, Credentials, ErrorKind, Generation, Method,
    QueryClient, Session,
};
use prost_reflect::DynamicMessage;
use serde::Deserialize;
use std::{
    path::Path,
    sync::{Arc, RwLock, atomic::AtomicBool},
};
use zeroize::Zeroizing;

#[cfg(test)]
#[path = "jp_tests.rs"]
mod tests;

pub const REGISTER: Method = Method {
    name: "jp-register",
    path: "/app.player.PlayerService/Register",
    input: "app.player.RegisterRequest",
    output: "app.player.RegisterResponse",
    anonymous: true,
    http: false,
};

pub fn valid_master(value: &str) -> bool {
    let Some((release, hash)) = value.split_once('/') else {
        return false;
    };
    value.len() <= 128
        && numeric_version(release).is_some()
        && hash.len() == 32
        && hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn numeric_version(value: &str) -> Option<[u32; 4]> {
    let parts: Vec<_> = value.split('.').collect();
    if !(2..=4).contains(&parts.len()) {
        return None;
    }
    let mut result = [0; 4];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        result[i] = part.parse().ok()?;
        if result[i] > i32::MAX as u32 {
            return None;
        }
    }
    Some(result)
}
pub(crate) fn asset_version(raw: Option<&str>, client: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Entry {
        #[serde(rename = "minClientVersion")]
        min: String,
        version: String,
        #[serde(rename = "Android")]
        android: Option<String>,
    }
    #[derive(Deserialize)]
    struct Payload {
        version: Option<String>,
        #[serde(rename = "Android")]
        android: Option<String>,
        live: Option<Vec<Option<Entry>>>,
    }
    let raw = raw.filter(|s| s.len() <= 64 * 1024)?;
    let payload: Payload = serde_json::from_str(raw).ok()?;
    let (version, hash) = if let Some(live) = payload.live.filter(|v| !v.is_empty()) {
        let current = numeric_version(client)?;
        let mut best: Option<([u32; 4], Entry)> = None;
        for entry in live.into_iter().flatten() {
            let Some(min) = numeric_version(&entry.min) else {
                continue;
            };
            if min <= current && best.as_ref().is_none_or(|(old, _)| min > *old) {
                best = Some((min, entry));
            }
        }
        let (_, selected) = best?;
        (selected.version, selected.android?)
    } else {
        (payload.version?, payload.android?)
    };
    let version = version.trim();
    if version.is_empty()
        || version.len() > 128
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || matches!(version, "." | "..")
        || hash.trim().len() != 32
        || !hash.trim().bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(version.to_owned())
}

impl Client {
    /// One user-authorized registration. Persist credentials before installing
    /// the new session. Caller must reserve output and never retry ambiguous failures.
    pub async fn register_jp(
        &self,
        generation: Generation,
        output: &Path,
        cancel: CancellationToken,
    ) -> Result<(), ClientError> {
        let session = self.session_for(generation)?;
        if session.config.region != "jp" || session.credentials.is_some() {
            return Err(ClientError::new(ErrorKind::InvalidConfig));
        }
        crate::secret_file::check_new(output)?;
        crate::secret_file::check_new(&output.with_extension("response"))?;
        let pool = moenotes_proto::pool_for_region("jp");
        let request = DynamicMessage::new(pool.get_message_by_name(REGISTER.input).unwrap());
        let metadata = session
            .config
            .metadata(None, &Generation::new_v4().to_string(), true)?;
        self.run_operation(
            session.clone(),
            REGISTER,
            request,
            metadata,
            cancel.clone(),
            |message| {
                let raw = Zeroizing::new(
                    serde_json::to_vec(&message)
                        .map_err(|_| ClientError::new(ErrorKind::Protocol))?,
                );
                // Preserve the response before normalization in case persistence/validation fails.
                crate::secret_file::create(&output.with_extension("response"), &raw)?;
                // Protobuf JSON uses camelCase; parse via the message fields directly
                // so credential output never passes through user-facing JSON logging.
                let field = message
                    .get_field_by_name("credential")
                    .ok_or_else(|| ClientError::new(ErrorKind::Protocol))?;
                let credential = field
                    .as_message()
                    .ok_or_else(|| ClientError::new(ErrorKind::Protocol))?;
                let get = |name| {
                    credential
                        .get_field_by_name(name)
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default()
                };
                let credentials = Credentials {
                    player_id: get("id"),
                    credential: get("credential"),
                    device_id: Some(get("device_id")).filter(|s| !s.is_empty()),
                    bid: None,
                };
                session
                    .config
                    .metadata(Some(&credentials), "validation", false)?;
                crate::session::save_credentials(&session.config, &credentials, output)?;
                let mut current = self.session.write().unwrap();
                if current.generation != generation {
                    return Err(ClientError::new(ErrorKind::SessionChanged));
                }
                let new = Arc::new(Session {
                    generation: Generation::new_v4(),
                    config: session.config.clone(),
                    credentials: Some(credentials),
                    transport: session.transport.clone(),
                    cancel: CancellationToken::new(),
                    blocked: RwLock::new(None),
                    master_mismatch: AtomicBool::new(false),
                    jp_override_pending: AtomicBool::new(true),
                });
                current.cancel.cancel();
                *current = new;
                Ok(())
            },
        )
        .await
    }

    /// Authenticated JP-only self-data probe; not part of the public HTTP query allowlist.
    pub async fn jp_self_data(
        &self,
        cancel: CancellationToken,
    ) -> Result<DynamicMessage, ClientError> {
        let session = self.session_for(self.generation())?;
        if session.config.region != "jp" || session.credentials.is_none() {
            return Err(ClientError::new(ErrorKind::AuthenticationRequired));
        }
        let method = Method {
            name: "jp-self-data",
            path: "/app.player.PlayerService/GetPlayerData",
            input: "app.player.GetPlayerDataRequest",
            output: "app.player.GetPlayerDataResponse",
            anonymous: false,
            http: false,
        };
        let request = DynamicMessage::new(
            moenotes_proto::pool_for_region("jp")
                .get_message_by_name(method.input)
                .unwrap(),
        );
        let metadata = session.config.metadata(
            session.credentials.as_ref(),
            &Generation::new_v4().to_string(),
            false,
        )?;
        self.run_operation(session, method, request, metadata, cancel, Ok)
            .await
    }
}

/// Recover a retained registration response locally after a persistence or
/// validation failure. Makes no request and refuses to overwrite credentials.
pub fn import_registration_response(
    config: &crate::SessionConfig,
    source: &Path,
    output: &Path,
) -> Result<(), ClientError> {
    if config.region != "jp" {
        return Err(ClientError::new(ErrorKind::InvalidConfig));
    }
    config.validate()?;
    let raw = crate::secret_file::read(source)?;
    let message = DynamicMessage::deserialize(
        moenotes_proto::pool_for_region("jp")
            .get_message_by_name(REGISTER.output)
            .unwrap(),
        &mut serde_json::Deserializer::from_slice(&raw),
    )
    .map_err(|_| ClientError::new(ErrorKind::Protocol))?;
    let field = message
        .get_field_by_name("credential")
        .ok_or_else(|| ClientError::new(ErrorKind::Protocol))?;
    let credential = field
        .as_message()
        .ok_or_else(|| ClientError::new(ErrorKind::Protocol))?;
    let get = |name| {
        credential
            .get_field_by_name(name)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let credentials = Credentials {
        player_id: get("id"),
        credential: get("credential"),
        device_id: Some(get("device_id")).filter(|s| !s.is_empty()),
        bid: None,
    };
    config.metadata(Some(&credentials), "validation", false)?;
    crate::session::save_credentials(config, &credentials, output)
}
