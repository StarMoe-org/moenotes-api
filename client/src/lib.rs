//! Experimental game queries and explicit SDK-to-game login. No automatic retry.
pub mod auth;
mod error;
pub mod jp;
mod query;
pub mod sdk_http;
mod secret_file;
mod session;
pub mod transport;
mod versions;

pub use versions::DataVersions;

pub use error::{ClientError, ErrorKind};
pub use moenotes_proto::generated;
pub use query::{METHODS, Method, Query};
pub use session::{CredentialProvider, Credentials, SessionConfig, StaticCredentials};
pub use tokio_util::sync::CancellationToken;
pub use uuid::Uuid as Generation;

use async_trait::async_trait;
#[cfg(test)]
use moenotes_proto::pool;
use prost_reflect::DynamicMessage;
use std::{
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{Mutex, Semaphore},
    time::Instant,
};
use transport::{GrpcTransport, Transport};

#[derive(Clone)]
pub struct QueryResponse {
    pub message: DynamicMessage,
    pub fetched_at: SystemTime,
    pub generation: Generation,
}

#[async_trait]
pub trait QueryClient: Send + Sync {
    fn generation(&self) -> Generation;
    /// A local admission check; it must not contact the upstream service.
    fn query_error(&self, _anonymous: bool) -> Option<ClientError> {
        None
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        cancel: CancellationToken,
    ) -> Result<QueryResponse, ClientError>;
}

#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub timeout: Duration,
    pub minimum_interval: Duration,
    pub queue_capacity: usize,
}
impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            minimum_interval: Duration::from_secs(1),
            queue_capacity: 32,
        }
    }
}

/// Local observations, never a claim that a credential is currently valid upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Anonymous,
    CredentialsUnverified,
    AuthenticationRejected,
    VersionBlocked,
    DeviceConflict,
}

struct Session {
    generation: Generation,
    config: SessionConfig,
    credentials: Option<Credentials>,
    transport: Arc<dyn Transport>,
    cancel: CancellationToken,
    blocked: RwLock<Option<ErrorKind>>,
    master_mismatch: std::sync::atomic::AtomicBool,
    jp_override_pending: std::sync::atomic::AtomicBool,
}

pub struct Client {
    session: RwLock<Arc<Session>>,
    serial: Mutex<Instant>,
    admission: Semaphore,
    options: ClientOptions,
}

impl Client {
    pub fn new(
        config: SessionConfig,
        provider: Option<&dyn CredentialProvider>,
        options: ClientOptions,
    ) -> Result<Self, ClientError> {
        let transport = Arc::new(GrpcTransport::new(&config)?);
        Self::with_transport(config, provider, options, transport)
    }

    pub fn with_transport(
        config: SessionConfig,
        provider: Option<&dyn CredentialProvider>,
        options: ClientOptions,
        transport: Arc<dyn Transport>,
    ) -> Result<Self, ClientError> {
        if options.timeout.is_zero()
            || options.timeout > Duration::from_secs(120)
            || options.minimum_interval > Duration::from_secs(60)
            || options.queue_capacity > 4096
        {
            return Err(ClientError::new(ErrorKind::InvalidConfig));
        }
        let session = Self::make_session(config, provider, transport)?;
        Ok(Self {
            session: RwLock::new(Arc::new(session)),
            serial: Mutex::new(Instant::now()),
            admission: Semaphore::new(options.queue_capacity + 1),
            options,
        })
    }

    fn make_session(
        config: SessionConfig,
        provider: Option<&dyn CredentialProvider>,
        transport: Arc<dyn Transport>,
    ) -> Result<Session, ClientError> {
        config.validate()?;
        let credentials = provider
            .map(|p| p.credentials(&config))
            .transpose()?
            .flatten();
        config.metadata(credentials.as_ref(), "validation", false)?;
        Ok(Session {
            generation: Generation::new_v4(),
            config,
            credentials,
            transport,
            cancel: CancellationToken::new(),
            blocked: RwLock::new(None),
            master_mismatch: std::sync::atomic::AtomicBool::new(false),
            jp_override_pending: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Replaces rather than mutates authentication. Old work is cancelled and cannot
    /// be cached as the new identity. New origins never inherit old credentials.
    pub fn replace_session(
        &self,
        config: SessionConfig,
        provider: Option<&dyn CredentialProvider>,
    ) -> Result<(), ClientError> {
        let transport = Arc::new(GrpcTransport::new(&config)?);
        let new = Arc::new(Self::make_session(config, provider, transport)?);
        let mut current = self.session.write().unwrap();
        current.cancel.cancel();
        *current = new;
        Ok(())
    }

    /// Install a locally imported credential into the expected generation while
    /// preserving versions and transport. Never sets a device-override flag.
    pub fn import_credentials(
        &self,
        generation: Generation,
        provider: &dyn CredentialProvider,
    ) -> Result<(), ClientError> {
        let mut current = self.session.write().unwrap();
        if current.generation != generation {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        let config = current.config.clone();
        let transport = current.transport.clone();
        let new = Arc::new(Self::make_session(config, Some(provider), transport)?);
        current.cancel.cancel();
        *current = new;
        Ok(())
    }

    pub async fn query(&self, query: Query) -> Result<QueryResponse, ClientError> {
        self.execute(self.generation(), query, CancellationToken::new())
            .await
    }

    pub fn session_status(&self) -> SessionStatus {
        let session = self.session.read().unwrap();
        match *session.blocked.read().unwrap() {
            Some(ErrorKind::Authentication) => SessionStatus::AuthenticationRejected,
            Some(ErrorKind::Version) => SessionStatus::VersionBlocked,
            Some(ErrorKind::DeviceConflict) => SessionStatus::DeviceConflict,
            _ if session.credentials.is_some() => SessionStatus::CredentialsUnverified,
            _ => SessionStatus::Anonymous,
        }
    }

    /// Compare a scoped provider's account identity without exposing credentials.
    /// This is a local check, not upstream verification; token rotation is ignored.
    pub fn matches_identity(
        &self,
        generation: Generation,
        provider: &dyn CredentialProvider,
    ) -> Result<bool, ClientError> {
        let session = self.session_for(generation)?;
        let supplied = provider.credentials(&session.config)?;
        Ok(match (&session.credentials, supplied) {
            (Some(a), Some(b)) => a.player_id == b.player_id && a.bid == b.bid,
            _ => false,
        })
    }

    /// Explicit Unix-only, no-overwrite export for StaticCredentials::from_file.
    /// The parent directory must be private. No network call or renewal is made.
    pub fn save_session(
        &self,
        generation: Generation,
        path: &std::path::Path,
    ) -> Result<(), ClientError> {
        let session = self.session.read().unwrap();
        if session.generation != generation {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        let blocked = session.blocked.read().unwrap();
        if let Some(kind) = *blocked {
            return Err(ClientError::new(kind));
        }
        let credentials = session
            .credentials
            .as_ref()
            .ok_or_else(|| ClientError::new(ErrorKind::AuthenticationRequired))?;
        session::save_credentials(&session.config, credentials, path)
    }

    fn session_for(&self, generation: Generation) -> Result<Arc<Session>, ClientError> {
        let session = self.session.read().unwrap().clone();
        if session.generation != generation {
            return Err(ClientError::new(ErrorKind::SessionChanged));
        }
        Ok(session)
    }

    // Keep response validation and any session installation inside the serial slot.
    async fn run_operation<T>(
        &self,
        session: Arc<Session>,
        method: Method,
        request: DynamicMessage,
        metadata: tonic::metadata::MetadataMap,
        cancel: CancellationToken,
        finish: impl FnOnce(DynamicMessage) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        self.run_operation_response(session, method, request, metadata, cancel, |r| {
            finish(r.message)
        })
        .await
    }

    async fn run_operation_response<T>(
        &self,
        session: Arc<Session>,
        method: Method,
        request: DynamicMessage,
        mut metadata: tonic::metadata::MetadataMap,
        cancel: CancellationToken,
        finish: impl FnOnce(transport::TransportResponse) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let _admission = self
            .admission
            .try_acquire()
            .map_err(|_| ClientError::new(ErrorKind::QueueFull))?;
        let work = async {
            let mut next = self.serial.lock().await;
            if let Some(kind) = *session.blocked.read().unwrap()
                && (!method.anonymous
                    || ((auth::is_login_method(method) || method == jp::REGISTER)
                        && matches!(kind, ErrorKind::Version | ErrorKind::DeviceConflict)))
            {
                return Err(ClientError::new(kind));
            }
            tokio::time::sleep_until(*next).await;
            *next = Instant::now() + self.options.minimum_interval;
            let override_sent = session.config.region == "jp"
                && !method.anonymous
                && session
                    .jp_override_pending
                    .load(std::sync::atomic::Ordering::SeqCst);
            if override_sent {
                metadata.insert("x-override-device-id", "1".parse().unwrap());
            }
            let result = session
                .transport
                .call_with_metadata(method, request, metadata, self.options.timeout)
                .await;
            if let Err(error) = &result
                && (!method.anonymous || auth::is_login_method(method))
                && matches!(
                    error.kind,
                    ErrorKind::Authentication | ErrorKind::Version | ErrorKind::DeviceConflict
                )
            {
                let mut blocked = session.blocked.write().unwrap();
                // Anonymous support errors must not weaken a version/device block.
                if !matches!(
                    *blocked,
                    Some(ErrorKind::Version | ErrorKind::DeviceConflict)
                ) {
                    let (initial, trailing) = error.business_codes();
                    session.master_mismatch.store(
                        trailing
                            .last()
                            .or(initial.last())
                            .is_some_and(|code| code == "MASTER_VERSION_MISMATCH"),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    *blocked = Some(error.kind);
                }
            }
            let response = result?;
            if override_sent {
                session
                    .jp_override_pending
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
            if cancel.is_cancelled() {
                return Err(ClientError::new(ErrorKind::Cancelled));
            }
            if self.generation() != session.generation {
                return Err(ClientError::new(ErrorKind::SessionChanged));
            }
            use prost_reflect::ReflectMessage;
            if response.message.descriptor().full_name() != method.output {
                return Err(ClientError::new(ErrorKind::Protocol));
            }
            finish(response)
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ClientError::new(ErrorKind::Cancelled)),
            _ = session.cancel.cancelled() => Err(ClientError::new(ErrorKind::SessionChanged)),
            result = tokio::time::timeout(self.options.timeout, work) => result.unwrap_or_else(|_| Err(ClientError::new(ErrorKind::Timeout))),
        }
    }
}

#[async_trait]
impl QueryClient for Client {
    fn generation(&self) -> Generation {
        self.session.read().unwrap().generation
    }

    fn query_error(&self, anonymous: bool) -> Option<ClientError> {
        if anonymous {
            return None;
        }
        let session = self.session.read().unwrap();
        if let Some(kind) = *session.blocked.read().unwrap() {
            return Some(ClientError::new(kind));
        }
        if session.credentials.is_none() {
            return Some(ClientError::new(ErrorKind::AuthenticationRequired));
        }
        None
    }

    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        cancel: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        query.validate()?;
        let session = self.session_for(generation)?;
        if !query.method().anonymous && session.credentials.is_none() {
            return Err(ClientError::new(ErrorKind::AuthenticationRequired));
        }
        let method = query.method_for_region(&session.config.region)?;
        let request = DynamicMessage::decode(
            moenotes_proto::pool_for_region(&session.config.region)
                .get_message_by_name(method.input)
                .ok_or_else(|| ClientError::new(ErrorKind::Protocol))?,
            query.encode().as_slice(),
        )
        .map_err(|_| ClientError::new(ErrorKind::Protocol))?;
        let metadata = session.config.metadata(
            session.credentials.as_ref(),
            &Generation::new_v4().to_string(),
            method.anonymous,
        )?;
        self.run_operation(session, method, request, metadata, cancel, |message| {
            Ok(QueryResponse {
                message,
                fetched_at: SystemTime::now(),
                generation,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests;
