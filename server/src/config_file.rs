//! Operator-visible configuration location and private, no-overwrite creation.
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const TEMPLATE: &str = include_str!("../../config.example.toml");

pub fn resolve(explicit: Option<String>) -> PathBuf {
    if let Some(path) = explicit {
        return path.into();
    }
    if let Some(path) = std::env::var_os("MOENOTES_CONFIG") {
        return path.into();
    }
    let legacy = Path::new("/etc/moenotes/config.toml");
    if std::fs::symlink_metadata(legacy).is_ok() {
        return legacy.into();
    }
    std::env::var_os("MOENOTES_DEFAULT_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

/// Returns false if anything already occupies the path, including a symlink.
pub fn create(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(TEMPLATE.as_bytes())?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(true),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.error),
    }
}

pub fn absolute(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
