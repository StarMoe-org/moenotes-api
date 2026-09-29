//! Request round robin over independent sessions, queues and identity-scoped caches.
use crate::{
    cache::{CacheOptions, CachedResponse, QueryCache},
    managed::ManagedClient,
};
use moenotes_client::{CancellationToken, ClientError, ErrorKind, Generation, Query, QueryClient};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::time::Instant;

pub const MAX_SESSIONS: usize = 32;
const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(5);

pub struct SessionMember {
    pub client: Arc<dyn QueryClient>,
    pub managed: Option<Arc<ManagedClient>>,
}
struct Member {
    client: Arc<dyn QueryClient>,
    managed: Option<Arc<ManagedClient>>,
    cache: Arc<QueryCache>,
    requests: AtomicU64,
    errors: AtomicU64,
    cooldown: Mutex<Option<(Instant, Generation, ErrorKind)>>,
}
impl Member {
    fn cooldown_error(&self) -> Option<ClientError> {
        let mut cooldown = self.cooldown.lock().unwrap();
        if let Some((until, generation, kind)) = *cooldown {
            if generation == self.client.generation() && Instant::now() < until {
                return Some(ClientError::new(kind));
            }
            *cooldown = None;
        }
        None
    }
}

pub struct SessionPool {
    members: Vec<Member>,
    next: Mutex<usize>,
    stop: CancellationToken,
}
impl SessionPool {
    pub fn new(
        members: Vec<SessionMember>,
        options: CacheOptions,
        stop: CancellationToken,
    ) -> Result<Arc<Self>, ClientError> {
        options.validate()?;
        if members.len() > MAX_SESSIONS {
            return Err(ClientError::new(ErrorKind::InvalidConfig));
        }
        Ok(Arc::new(Self {
            members: members
                .into_iter()
                .map(|m| Member {
                    cache: QueryCache::new(m.client.clone(), options.clone(), stop.clone()),
                    client: m.client,
                    managed: m.managed,
                    requests: AtomicU64::new(0),
                    errors: AtomicU64::new(0),
                    cooldown: Mutex::new(None),
                })
                .collect(),
            next: Mutex::new(0),
            stop,
        }))
    }
    pub fn ready(&self) -> bool {
        self.members
            .iter()
            .any(|m| m.cooldown_error().is_none() && m.managed.as_ref().is_some_and(|m| m.ready()))
    }
    /// Diagnostics never probe credentials or trigger lazy authentication.
    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "sessions": self.members.len(),
            "ready_sessions": self.members.iter().filter(|m| m.cooldown_error().is_none() && m.managed.as_ref().is_some_and(|m| m.ready())).count(),
            "members": self.members.iter().enumerate().map(|(slot, m)| serde_json::json!({
                "slot": slot, "requests": m.requests.load(Ordering::Relaxed), "errors": m.errors.load(Ordering::Relaxed),
                "cooling_down": m.cooldown_error().is_some(),
                "session": m.managed.as_ref().map(|m| m.status()),
            })).collect::<Vec<_>>()
        })
    }
    pub(crate) fn single_status(&self) -> Option<crate::managed::Status> {
        if self.members.len() == 1 {
            self.members[0].managed.as_ref().map(|m| m.status())
        } else {
            None
        }
    }
    pub async fn query(
        &self,
        query: Query,
    ) -> Result<(Arc<CachedResponse>, &'static str), ClientError> {
        query.validate()?;
        if self.stop.is_cancelled() {
            return Err(ClientError::new(ErrorKind::Cancelled));
        }
        let anonymous = query.method().anonymous;
        // Choose once before entering that session's cache. Raw responses and
        // coalesced work can never cross an account boundary. No failed RPC replay.
        let index = {
            let mut next = self.next.lock().unwrap();
            let mut chosen = None;
            let mut first_error = None;
            for offset in 0..self.members.len() {
                let index = (*next + offset) % self.members.len();
                let m = &self.members[index];
                // A single backend preserves the existing retry/lifecycle contract.
                let error = if self.members.len() > 1 {
                    m.cooldown_error()
                        .or_else(|| m.client.query_error(anonymous))
                } else {
                    None
                };
                if let Some(error) = error {
                    first_error.get_or_insert(error);
                } else {
                    chosen = Some(index);
                    *next = (index + 1) % self.members.len();
                    break;
                }
            }
            chosen.ok_or_else(|| {
                first_error.unwrap_or_else(|| ClientError::new(ErrorKind::AuthenticationRequired))
            })?
        };
        let member = &self.members[index];
        member.requests.fetch_add(1, Ordering::Relaxed);
        let generation = member.client.generation();
        let result = member.cache.query(query).await;
        if let Err(error) = &result {
            member.errors.fetch_add(1, Ordering::Relaxed);
            if self.members.len() > 1
                && matches!(
                    error.kind,
                    ErrorKind::Transport | ErrorKind::Timeout | ErrorKind::Maintenance
                )
                && member.client.generation() == generation
            {
                *member.cooldown.lock().unwrap() =
                    Some((Instant::now() + TRANSIENT_COOLDOWN, generation, error.kind));
            }
        }
        result
    }
}
