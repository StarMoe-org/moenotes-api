//! Explicit JP registration. The HTTP server and lazy loader never invoke this module.
use crate::{accounts::JpAccountDirectoryClient, config::Config, operator};
use moenotes_client::{CancellationToken, Client, CredentialProvider, Query, QueryClient};
use std::{fs, io::Write, path::Path, sync::Arc};

pub async fn register(config: &Config, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if config.session.region != "jp"
        || config.login.is_some()
        || config.credentials_file.is_some()
        || !name.ends_with(".json")
        || name.starts_with('.')
        || name.contains(['/', '\\'])
        || name.len() > 100
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(
            "JP registration requires a JP session, accounts directory and a plain .json filename"
                .into(),
        );
    }
    let accounts = config
        .accounts
        .as_ref()
        .ok_or("JP accounts configuration required")?;
    let parent = &accounts.directory;
    operator::private_dir(parent)?;
    // Refuse to register into a nonempty account directory. A response/attempt
    // marker also blocks repeats after a lost response or persistence error.
    if fs::read_dir(parent)?.next().is_some() {
        return Err("JP registration directory must be empty; an existing account or attempt cannot be overwritten".into());
    }
    let output = parent.join(name);
    let client = Client::new(config.session.clone(), None, config.client_options())?;
    client
        .refresh_versions(client.generation(), CancellationToken::new())
        .await?;
    let mut attempt = fs::OpenOptions::new();
    attempt.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        attempt.mode(0o600);
    }
    let mut marker = attempt.open(parent.join("registration.attempt"))?;
    marker.write_all(b"Registration was requested. Do not retry if outcome is uncertain.\n")?;
    marker.sync_all()?;
    client
        .register_jp(client.generation(), &output, CancellationToken::new())
        .await?;
    println!(
        "{}",
        serde_json::json!({"status":"registered_and_saved","region":"jp","credential_file":output,"automatic_retry":false})
    );
    Ok(())
}

pub async fn check(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    if config.session.region != "jp" {
        return Err("JP session required".into());
    }
    let client = Arc::new(Client::new(
        config.session.clone(),
        None,
        config.client_options(),
    )?);
    let provider = if let Some(accounts) = &config.accounts {
        JpAccountDirectoryClient::new(client.clone(), accounts.clone(), config.session.clone())?
            .provider()?
    } else {
        moenotes_client::StaticCredentials::from_file(
            config
                .credentials_file
                .as_ref()
                .ok_or("JP credentials required")?,
        )?
    };
    let credentials = provider
        .credentials(&config.session)?
        .ok_or("JP credentials required")?;
    client.replace_session(config.session.clone(), Some(&provider))?;
    client
        .refresh_versions(client.generation(), CancellationToken::new())
        .await?;
    let who = client.query(Query::Whoami(Default::default())).await?;
    let id = who
        .message
        .get_field_by_name("player_id")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or("Whoami missing id")?;
    if id != credentials.player_id {
        return Err("Whoami identity mismatch".into());
    }
    let own = client.jp_self_data(CancellationToken::new()).await?;
    let self_data_present = own.has_field_by_name("player_data");
    if !self_data_present {
        return Err("Self-data missing player_data".into());
    }
    let account_id = own
        .get_field_by_name("accountid")
        .and_then(|v| v.as_i64())
        .filter(|v| *v > 0);
    let bulk_profiles_received = if let Some(id) = account_id {
        client
            .query(Query::Profiles(
                moenotes_client::generated::app::playerext::GetPlayerListRequest {
                    account_ids: vec![id],
                },
            ))
            .await?;
        true
    } else {
        false
    };
    println!(
        "{}",
        serde_json::json!({"region":"jp","whoami_matches_import":true,"self_data_received":self_data_present,"bulk_profiles_received":bulk_profiles_received,"master_version":client.session_config().master_version})
    );
    Ok(())
}

pub fn import_registration(config: &Config, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !name.ends_with(".json")
        || name.starts_with('.')
        || name.contains(['/', '\\'])
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err("plain JSON filename required".into());
    }
    let accounts = config.accounts.as_ref().ok_or("JP accounts required")?;
    operator::private_dir(&accounts.directory)?;
    let output = accounts.directory.join(name);
    moenotes_client::jp::import_registration_response(
        &config.session,
        &output.with_extension("response"),
        &output,
    )?;
    println!(
        "{}",
        serde_json::json!({"status":"registration_response_imported","region":"jp","network_requests":0})
    );
    Ok(())
}

/// Create the shared account root and release directories without reading or moving secrets.
pub fn init_accounts(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for path in [root.to_owned(), root.join("international"), root.join("jp")] {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        operator::private_dir(&path)?;
    }
    Ok(())
}
