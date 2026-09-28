use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    time::Duration,
};

#[test]
fn cli_validates_starts_and_shuts_down_without_upstream_calls() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    let key = directory.path().join("key");
    std::fs::write(&key, "synthetic-http-key-not-for-production-123456789\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(
        &config,
        r#"
listen = "127.0.0.1:0"
response_mode = "disabled"
api_key_file = "key"
[version_sync]
enabled = false
[session]
region = "test"
origin = "https://game.example.invalid"
allowed_origins = ["https://game.example.invalid"]
platform = "android"
client_version = "1.0.1"
"#,
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_moenotes-server");
    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("moenotes-server {}", env!("CARGO_PKG_VERSION"))
    );
    let output = Command::new(binary)
        .arg("check-config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("no upstream requests"));
    let mut child = Command::new(binary)
        .arg("serve")
        .arg(&config)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let address = line.split_whitespace().nth(3).expect("startup address");
    let response = request(address, "/healthz");
    assert!(response.contains("200 OK"));
    assert!(response.contains("\"status\":\"ok\""));
    assert!(request(address, "/health").contains("200 OK"));
    assert!(request(address, "/openapi.json").contains("401 Unauthorized"));
    assert!(request(address, "/v1/announcements").contains("404 Not Found"));
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
    }
    #[cfg(not(unix))]
    child.kill().unwrap();
    let status = child.wait().unwrap();
    #[cfg(unix)]
    assert!(status.success());
    #[cfg(not(unix))]
    let _ = status;
}

fn request(address: &str, path: &str) -> String {
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}
