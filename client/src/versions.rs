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
        let session = self.session_for(generation)?;
        let query = Query::Version(Default::default());
        let method = *query.method();
        let request = DynamicMessage::new(
            moenotes_proto::pool()
                .get_message_by_name(method.input)
                .unwrap(),
        );
        let metadata = session
            .config
            .metadata(None, &Generation::new_v4().to_string(), true)?;
        self.run_operation(
            session.clone(),
            method,
            request,
            metadata,
            cancel.clone(),
            |message| {
                let response = crate::generated::app::masterdata::VersionResponse::decode(
                    message.encode_to_vec().as_slice(),
                )
                .map_err(|_| ClientError::new(ErrorKind::Protocol))?;
                if !valid(&response.version) || !valid(&response.resource_version) {
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
                    && current.config.resource_version.as_ref() == Some(&response.resource_version)
                {
                    return Ok(false);
                }
                let mut config = current.config.clone();
                config.master_version = Some(response.version);
                config.resource_version = Some(response.resource_version);
                let mut blocked = *current.blocked.read().unwrap();
                if blocked == Some(ErrorKind::Version)
                    && current.master_mismatch.load(Ordering::SeqCst)
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
                });
                current.cancel.cancel();
                *current = new;
                Ok(true)
            },
        )
        .await
    }
}
