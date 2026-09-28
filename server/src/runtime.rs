//! One independently configured upstream and its authentication lifecycle.
use crate::{
    accounts::{AccountDirectoryClient, JpAccountDirectoryClient},
    config::Config,
    managed::{GameRecovery, ManagedClient, Recovery},
};
use moenotes_client::{
    CancellationToken, Client, ClientError, CredentialProvider, StaticCredentials,
};
use std::{sync::Arc, time::Duration};

pub struct Runtime {
    pub config: Config,
    pub client: Arc<Client>,
    pub managed: Arc<ManagedClient>,
    accounts: Option<Arc<AccountDirectoryClient>>,
    jp_accounts: bool,
}
impl Runtime {
    /// Validate local state without reading passwords or making upstream requests.
    pub fn new(config: Config, stop: CancellationToken) -> Result<Self, ClientError> {
        if let Some(login) = &config.login {
            login.validate(&config.session)?;
        }
        let provider = if config.accounts.is_some() {
            None
        } else if let Some(login) = &config.login {
            login.credentials(&config.session)?
        } else {
            config
                .credentials_file
                .as_ref()
                .map(|p| StaticCredentials::from_file(p))
                .transpose()?
        };
        let client = Arc::new(Client::new(
            config.session.clone(),
            provider.as_ref().map(|p| p as &dyn CredentialProvider),
            config.client_options(),
        )?);
        if config.recovery.enabled && provider.is_some() {
            let _ = config
                .login
                .as_ref()
                .unwrap()
                .approved_sdk(&config.session)?;
        }
        let accounts = config
            .accounts
            .clone()
            .filter(|_| config.session.region != "jp")
            .map(|accounts| {
                AccountDirectoryClient::new(
                    client.clone(),
                    accounts,
                    config.login.clone().unwrap(),
                    config.session.clone(),
                )
                .map(Arc::new)
            })
            .transpose()?;
        let recovery: Option<Arc<dyn Recovery>> = if config.recovery.enabled {
            if let Some(accounts) = &accounts {
                Some(accounts.clone())
            } else {
                Some(Arc::new(GameRecovery {
                    client: client.clone(),
                    login: config.login.clone().unwrap(),
                    config: config.session.clone(),
                }))
            }
        } else {
            None
        };
        let mut managed = ManagedClient::new(
            client.clone(),
            recovery,
            Duration::from_secs(config.recovery.cooldown_seconds),
            stop,
        );
        if let Some(accounts) = &accounts {
            managed = managed.with_initial_loader(accounts.clone());
        }
        let jp_accounts = config.session.region == "jp" && config.accounts.is_some();
        if jp_accounts {
            managed = managed.with_initial_loader(Arc::new(JpAccountDirectoryClient::new(
                client.clone(),
                config.accounts.clone().unwrap(),
                config.session.clone(),
            )?));
        }
        Ok(Self {
            config,
            client,
            managed: Arc::new(managed),
            accounts,
            jp_accounts,
        })
    }
    pub fn start(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.accounts.is_some() || self.jp_accounts {
            eprintln!(
                "{}",
                serde_json::json!({"event":"account_source","status":"deferred","trigger":"first_authenticated_query"})
            );
        }
        self.config.version_sync.enabled.then(|| {
            self.managed
                .start_version_sync(self.client.clone(), self.config.version_sync.clone())
        })
    }
    pub fn reload(&self) -> Result<(), ClientError> {
        self.managed.reload(|| {
            if self.jp_accounts {
                return self
                    .client
                    .replace_session(self.client.session_config(), None);
            }
            if let Some(accounts) = &self.accounts {
                self.client
                    .replace_session(self.client.session_config(), None)?;
                accounts.reset();
                return Ok(());
            }
            let provider = if let Some(login) = &self.config.login {
                login.credentials(&self.config.session)?
            } else {
                self.config
                    .credentials_file
                    .as_ref()
                    .map(|p| StaticCredentials::from_file(p))
                    .transpose()?
            };
            self.client.replace_session(
                self.client.session_config(),
                provider.as_ref().map(|p| p as &dyn CredentialProvider),
            )
        })
    }
}
