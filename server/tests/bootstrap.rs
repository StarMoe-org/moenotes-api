use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn request(address: &str, path: &str) -> String {
    request_with_auth(address, path, false)
}
fn request_with_auth(address: &str, path: &str, auth: bool) -> String {
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{}\r\n",
        if auth {
            "Authorization: Bearer synthetic-bootstrap-api-key-1234567890\r\n"
        } else {
            ""
        }
    )
    .unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}

#[cfg(unix)]
#[test]
fn account_deployment_starts_empty_and_reloads_without_logging_in() {
    account_deployment(false);
}
#[cfg(unix)]
#[test]
fn inline_deployment_needs_no_other_configuration_files() {
    account_deployment(true);
}
#[cfg(unix)]
fn account_deployment(inline: bool) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let common: std::collections::BTreeMap<_, _> = moenotes_client::sdk_http::COMMON_FIELDS
        .iter()
        .map(|key| (*key, Some("synthetic")))
        .collect();
    let sdk = serde_json::to_vec(&serde_json::json!({"base_url":"https://sdk.invalid","allowed_base_urls":["https://sdk.invalid"],"app_key":"synthetic-key","country_id":1,"common":common})).unwrap();
    for (name, bytes) in [
        ("key",b"synthetic-bootstrap-api-key-1234567890".as_slice()),
        ("context.json",br#"{"device_model":"synthetic","operating_system":"synthetic","device_identifier":"synthetic","global_channel_id":1,"brand_id":1,"area_id":1}"#),
        ("sdk.json",sdk.as_slice()),
    ] {
        let path=dir.path().join(name);
        fs::write(&path,bytes).unwrap();
        fs::set_permissions(path,fs::Permissions::from_mode(0o600)).unwrap();
    }
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"listen="127.0.0.1:0"
api_key_file="key"
access_log=false
[version_sync]
enabled=false
[accounts]
directory="accounts"
[login]
context_file="context.json"
sdk_http_file="sdk.json"
state_dir="."
[session]
region="test"
origin="https://game.invalid"
allowed_origins=["https://game.invalid"]
platform="android"
client_version="1"
"#,
    )
    .unwrap();
    if inline {
        fs::write(&path, include_str!("fixtures/inline-config.toml")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        for name in ["key", "context.json", "sdk.json"] {
            fs::remove_file(dir.path().join(name)).unwrap();
        }
    }
    let binary = env!("CARGO_BIN_EXE_moenotes-server");
    for command in ["check-config", "auth-status"] {
        let output = Command::new(binary)
            .arg(command)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut process = Process(
        Command::new(binary)
            .arg("serve")
            .arg(&path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = process.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        tx.send(line).unwrap();
    });
    let line = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    reader.join().unwrap();
    let address = line.split_whitespace().nth(3).unwrap();
    for phase in 0..3 {
        assert!(request(address, "/health").contains("200 OK"));
        assert!(request_with_auth(address, "/v1/status", true).contains("200 OK"));
        assert!(request_with_auth(address, "/readyz", true).contains("503 Service Unavailable"));
        if phase == 0 {
            fs::create_dir(dir.path().join("accounts")).unwrap();
            fs::set_permissions(
                dir.path().join("accounts"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        } else if phase == 1 {
            fs::write(
                dir.path().join("accounts/one.json"),
                br#"{"user":"synthetic-never-log","password":"synthetic-never-log"}"#,
            )
            .unwrap();
            fs::set_permissions(
                dir.path().join("accounts/one.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
    }
    Command::new("kill")
        .args(["-HUP", &process.0.id().to_string()])
        .status()
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert!(request_with_auth(address, "/v1/status", true).contains("unverified"));
    let output = Command::new(binary)
        .arg("sdk-login")
        .arg(&path)
        .arg("--password-stdin")
        .output()
        .unwrap();
    assert!(!output.status.success());
    Command::new("kill")
        .args(["-TERM", &process.0.id().to_string()])
        .status()
        .unwrap();
    assert!(process.0.wait().unwrap().success());
    let mut error = String::new();
    process
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut error)
        .unwrap();
    assert!(error.contains("account_source") && error.contains("session_reload"));
    assert!(!error.contains("account_load") && !error.contains("synthetic-never-log"));
    assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("account-")
    }));
}
#[test]
fn empty_deployment_stays_alive_but_check_config_is_strict() {
    for content in [
        None,
        Some(""),
        Some("listen=\"127.0.0.1:0\"\napi_key=\"\"\n[login.context]\n[login.sdk_http]\n"),
        Some("listen=\"127.0.0.1:0\"\n[session]\nregion=\"synthetic-do-not-log\"\n"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        if let Some(content) = content {
            fs::write(&path, content).unwrap();
        }
        let binary = env!("CARGO_BIN_EXE_moenotes-server");
        let mut process = Process(
            Command::new(binary)
                .arg("serve")
                .arg(&path)
                .env("MOENOTES_BOOTSTRAP_LISTEN", "127.0.0.1:0")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let stdout = process.0.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            tx.send(line).unwrap();
        });
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("health server did not start");
        reader.join().unwrap();
        let address = line.split_whitespace().nth(3).unwrap();
        for route in ["/health", "/healthz"] {
            let result = request(address, route);
            assert!(result.contains("200 OK"));
            assert!(result.contains("\"status\":\"ok\""));
        }
        assert!(request(address, "/readyz").contains("503 Service Unavailable"));
        let query = request(address, "/v1/profile?playerProfileId=1");
        assert!(query.contains("503 Service Unavailable") && query.contains("unconfigured"));
        assert!(process.0.try_wait().unwrap().is_none());
        assert!(
            !Command::new(binary)
                .arg("check-config")
                .arg(&path)
                .output()
                .unwrap()
                .status
                .success()
        );
        #[cfg(unix)]
        {
            Command::new("kill")
                .args(["-TERM", &process.0.id().to_string()])
                .status()
                .unwrap();
            assert!(process.0.wait().unwrap().success());
        }
        #[cfg(not(unix))]
        {
            process.0.kill().unwrap();
            process.0.wait().unwrap();
        }
        let mut error = String::new();
        process
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
        let log = error
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|value| value["event"] == "configuration_missing")
            .unwrap();
        assert_eq!(log["event"], "configuration_missing");
        let fields = log["missing"].as_array().unwrap();
        assert!(
            fields.contains(&serde_json::json!("api_key_file"))
                || fields.contains(&serde_json::json!("api_key"))
        );
        assert!(fields.contains(&serde_json::json!("session.origin")));
        assert!(!error.contains("synthetic-do-not-log"));
    }
}

#[test]
fn distributed_blank_template_is_health_only() {
    let template = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("config.example.toml"),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, template).unwrap();
    let moenotes_server::startup::Startup::Unconfigured { missing, .. } =
        moenotes_server::startup::inspect(&path, "127.0.0.1:0".parse().unwrap()).unwrap()
    else {
        panic!("blank template must bootstrap")
    };
    for key in [
        "api_key",
        "session.origin",
        "login.context.device_identifier",
        "login.sdk_http.app_key",
    ] {
        assert!(missing.contains(&key));
    }
}
