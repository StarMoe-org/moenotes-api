use moenotes_server::{RouterOptions, cache::CacheOptions, config::Config, operator};
use std::{
    io::{self, IsTerminal, Read},
    path::Path,
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    if command == "--version" || command == "-V" {
        println!("moenotes-server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if matches!(command.as_str(), "help" | "--help") {
        println!(
            "moenotes-server <serve|init-config|config-path|check-config|sdk-login|game-login|auth-status> [config.toml]\nConfig: explicit path > MOENOTES_CONFIG > existing /etc/moenotes/config.toml > container default > ./config.toml\ninit-config: create a private template without overwriting existing files\nconfig-path: show the actual path without revealing configuration values\nsdk-login: --password-stdin reads one private JSON object, otherwise prompts without password echo\ngame-login: --confirm-sdk-ready [--allow-create]\nSIGHUP: reload saved session, or reset lazy accounts loading; never log in on the signal.\nmoenotes-server --version"
        );
        println!(
            "JP: jp-register [config.toml] --allow-create --name NAME.json; jp-check [config.toml]; jp-import-registration [config.toml] --name NAME.json\nAccount directories: init-accounts [ROOT] (default ./accounts)"
        );
        return Ok(());
    }
    if command == "init-accounts" {
        let root = args.next().unwrap_or_else(|| "accounts".into());
        if args.next().is_some() {
            return Err("unexpected argument".into());
        }
        moenotes_server::jp_operator::init_accounts(Path::new(&root))?;
        println!("Created/verified accounts/international and accounts/jp directories.");
        return Ok(());
    }
    let mut args = args.peekable();
    let explicit = if args.peek().is_some_and(|arg| !arg.starts_with("--")) {
        args.next()
    } else {
        None
    };
    let path = moenotes_server::config_file::resolve(explicit);
    let flags: Vec<_> = args.collect();
    if matches!(command.as_str(), "init-config" | "config-path") {
        if !flags.is_empty() {
            return Err("unexpected argument".into());
        }
        let created = if command == "init-config" {
            moenotes_server::config_file::create(&path)?
        } else {
            false
        };
        println!(
            "{}",
            serde_json::json!({"config_path":moenotes_server::config_file::absolute(&path)?,"exists":path.exists(),"created":created,"action":"Edit this file, run check-config, then restart the service."})
        );
        return Ok(());
    }
    if command == "serve" {
        if !flags.is_empty() {
            return Err("unexpected argument".into());
        }
        let outcome = moenotes_server::config_file::create(&path);
        eprintln!(
            "{}",
            serde_json::json!({"event":"configuration_path","path":moenotes_server::config_file::absolute(&path)?,"template_created":matches!(outcome, Ok(true)),"template_error":outcome.err().map(|e|format!("{:?}",e.kind())),"action":"Edit this config.toml, run check-config, then restart; use a persistent volume."})
        );
    }
    let config = if command == "serve" {
        if !flags.is_empty() {
            return Err("unexpected argument".into());
        }
        let fallback = std::env::var("MOENOTES_BOOTSTRAP_LISTEN")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()
            .map_err(|_| "invalid MOENOTES_BOOTSTRAP_LISTEN")?;
        match moenotes_server::startup::inspect(Path::new(&path), fallback)? {
            moenotes_server::startup::Startup::Configured(config) => *config,
            moenotes_server::startup::Startup::Unconfigured { listen, missing } => {
                let listen = if std::env::var_os("MOENOTES_BOOTSTRAP_LISTEN").is_some() {
                    fallback
                } else {
                    listen
                };
                return moenotes_server::startup::serve(listen, &missing).await;
            }
        }
    } else {
        Config::read(Path::new(&path))?
    };
    if config.accounts.is_some() && matches!(command.as_str(), "sdk-login" | "game-login") {
        return Err("accounts mode uses lazy login; use a separate configuration without [accounts] for manual operator login".into());
    }
    match command.as_str() {
        "jp-import-registration" => {
            if flags.len() != 2 || flags[0] != "--name" {
                return Err("requires --name NAME.json".into());
            }
            return moenotes_server::jp_operator::import_registration(&config, &flags[1]);
        }
        "jp-register" => {
            if flags.len() != 3 || flags[0] != "--allow-create" || flags[1] != "--name" {
                return Err("requires --allow-create --name NAME.json".into());
            }
            return moenotes_server::jp_operator::register(&config, &flags[2]).await;
        }
        "jp-check" => {
            if !flags.is_empty() {
                return Err("unexpected argument".into());
            }
            return moenotes_server::jp_operator::check(&config).await;
        }
        "sdk-login" => {
            if flags.iter().any(|f| f != "--password-stdin") {
                return Err("unknown flag".into());
            }
            let login = config
                .login
                .as_ref()
                .ok_or("login configuration required")?;
            let (email, password) = if flags.iter().any(|f| f == "--password-stdin") {
                if io::stdin().is_terminal() {
                    return Err("--password-stdin requires a pipe, not a terminal".into());
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    email: String,
                    password: String,
                }
                let mut bytes = Zeroizing::new(Vec::new());
                io::stdin().take(16385).read_to_end(&mut bytes)?;
                if bytes.len() > 16384 {
                    return Err("secret input too large".into());
                }
                let i: Input =
                    serde_json::from_slice(&bytes).map_err(|_| "invalid secret JSON input")?;
                (i.email, Zeroizing::new(i.password))
            } else {
                let email = rpassword::prompt_password("Email: ")?;
                let password = Zeroizing::new(rpassword::prompt_password("Password: ")?);
                (email, password)
            };
            operator::sdk_login(&config.session, login, email, password).await?;
            println!(
                "SDK authorization saved as pending. Complete required SDK consent/checks, then run game-login --confirm-sdk-ready."
            );
            return Ok(());
        }
        "game-login" => {
            if !flags.iter().any(|f| f == "--confirm-sdk-ready")
                || flags
                    .iter()
                    .any(|f| f != "--confirm-sdk-ready" && f != "--allow-create")
            {
                return Err("requires --confirm-sdk-ready; optional --allow-create".into());
            }
            operator::game_login(
                &config.session,
                config
                    .login
                    .as_ref()
                    .ok_or("login configuration required")?,
                config.client_options(),
                flags.iter().any(|f| f == "--allow-create"),
            )
            .await?;
            println!(
                "Game session saved. Reload a running service with SIGHUP; do not log in again merely to restart it."
            );
            return Ok(());
        }
        "serve" | "check-config" | "auth-status" => {
            if !flags.is_empty() {
                return Err("unexpected argument".into());
            }
        }
        _ => return Err("unknown command".into()),
    }
    let stop = moenotes_client::CancellationToken::new();
    let default = Arc::new(moenotes_server::runtime::Runtime::new(
        config.clone(),
        stop.clone(),
    )?);
    let mut runtimes = vec![default.clone()];
    let mut backends = Vec::new();
    if let Some(region) = moenotes_server::regions::Region::from_session(&config.session.region) {
        backends.push(moenotes_server::regions::RegionBackend {
            region,
            client: default.managed.clone(),
            managed: Some(default.managed.clone()),
        });
    }
    for (region, settings) in &config.regions {
        let runtime = Arc::new(moenotes_server::runtime::Runtime::new(
            config.for_region(settings),
            stop.clone(),
        )?);
        backends.push(moenotes_server::regions::RegionBackend {
            region: *region,
            client: runtime.managed.clone(),
            managed: Some(runtime.managed.clone()),
        });
        runtimes.push(runtime);
    }
    if command == "auth-status" {
        println!(
            "{}",
            serde_json::json!({"session":format!("{:?}",default.client.session_status()),"recovery_enabled":config.recovery.enabled,"accounts_enabled":config.accounts.is_some(),"network_checked":false,"regions":runtimes.iter().filter_map(|r|moenotes_server::regions::Region::from_session(&r.config.session.region).map(|region|(region.as_str(),serde_json::json!({"session":format!("{:?}",r.client.session_status()),"recovery_enabled":r.config.recovery.enabled,"accounts_enabled":r.config.accounts.is_some()})))).collect::<std::collections::BTreeMap<_,_>>()})
        );
        return Ok(());
    }
    let cache_options = CacheOptions {
        ttl: Duration::from_secs(config.cache_ttl_seconds),
        capacity: config.cache_capacity,
        ..Default::default()
    };
    let regions = moenotes_server::regions::RegionClients::new(
        moenotes_server::regions::Region::from_session(&config.session.region),
        backends,
        cache_options.clone(),
        stop.clone(),
    )?;
    let app = moenotes_server::router_with_regions(
        default.managed.clone(),
        config.read_api_key()?,
        RouterOptions {
            mode: config.mode(),
            managed: Some(default.managed.clone()),
            access_log: config.access_log,
        },
        cache_options,
        stop.clone(),
        regions,
    )?;
    if command == "check-config" {
        println!("Configuration valid; no upstream requests made; session validity not verified.");
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let version_tasks: Vec<_> = runtimes
        .iter()
        .filter_map(|runtime| runtime.start())
        .collect();
    println!(
        "moenotes-api listening on {} (response mode: {:?})",
        listener.local_addr()?,
        config.mode()
    );
    #[cfg(unix)]
    {
        let stop_reload = stop.clone();
        let runtimes = runtimes.clone();
        tokio::spawn(async move {
            let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("SIGHUP handler");
            loop {
                tokio::select! {
                    _ = stop_reload.cancelled() => break,
                    _ = hup.recv() => {
                        for runtime in &runtimes {
                            let success = runtime.reload().is_ok();
                            eprintln!("{}",serde_json::json!({"event":"session_reload","region":moenotes_server::regions::Region::from_session(&runtime.config.session.region),"success":success}));
                        }
                    }
                }
            }
        });
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            stop.cancel();
            for task in version_tasks {
                let _ = task.await;
            }
            for runtime in runtimes {
                runtime.managed.wait_idle().await;
            }
        })
        .await?;
    Ok(())
}
