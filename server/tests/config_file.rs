use std::{fs, process::Command};

#[test]
fn init_and_path_are_private_idempotent_and_env_selectable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persistent/config.toml");
    let binary = env!("CARGO_BIN_EXE_moenotes-server");
    let output = Command::new(binary)
        .arg("config-path")
        .env("MOENOTES_CONFIG", &path)
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["config_path"], path.to_str().unwrap());
    assert!(!path.exists());
    let output = Command::new(binary)
        .arg("init-config")
        .env("MOENOTES_CONFIG", &path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        moenotes_server::config_file::TEMPLATE
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    fs::write(&path, "private-preserve-existing").unwrap();
    let output = Command::new(binary)
        .arg("init-config")
        .arg(&path)
        .env("MOENOTES_CONFIG", dir.path().join("wrong.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "private-preserve-existing"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-preserve-existing"));
}

#[cfg(unix)]
#[test]
fn init_does_not_follow_or_replace_dangling_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("missing");
    let path = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(!moenotes_server::config_file::create(&path).unwrap());
    assert!(!target.exists());
    assert!(fs::symlink_metadata(path).unwrap().is_symlink());
}
