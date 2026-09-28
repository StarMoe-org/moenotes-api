use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

fn config(dir: &std::path::Path, extra: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    fs::write(
        &path,
        format!(
            r#"listen="127.0.0.1:0"
api_key_file="key"
{extra}
[session]
region="test"
origin="https://game.invalid"
allowed_origins=["https://game.invalid"]
platform="android"
client_version="1"
"#
        ),
    )
    .unwrap();
    path
}
#[test]
fn operator_commands_fail_closed_without_network_or_secret_echo() {
    let dir = tempfile::tempdir().unwrap();
    let path = config(dir.path(), "");
    let binary = env!("CARGO_BIN_EXE_moenotes-server");
    let status = Command::new(binary)
        .args(["auth-status", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(status.status.success());
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("Anonymous") && text.contains("false"));
    for flags in [
        vec!["game-login"],
        vec!["game-login", "--confirm-sdk-ready"],
        vec!["sdk-login", "--unknown"],
    ] {
        let mut c = Command::new(binary);
        c.arg(flags[0]).arg(&path).args(&flags[1..]);
        let r = c.output().unwrap();
        assert!(!r.status.success());
    }
    let mut child = Command::new(binary)
        .args(["sdk-login", path.to_str().unwrap(), "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"email":"synthetic@example.invalid","password":"secret-never-print"}"#);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret-never-print"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-never-print"));
}
#[test]
fn invalid_recovery_and_response_modes_are_rejected_offline() {
    let dir = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_moenotes-server");
    for extra in [
        "response_mode=\"unknown\"",
        "response_mode=\"public\"\nenable_experimental_raw=true",
        "[recovery]\nenabled=true",
        "[recovery]\ncooldown_seconds=0",
        "[version_sync]\ninterval_seconds=0",
        "[version_sync]\ninterval_seconds=86401",
        "[version_sync]\nenabled=\"yes\"",
    ] {
        let path = config(dir.path(), extra);
        let r = Command::new(binary)
            .args(["check-config", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(!r.status.success());
    }
}
