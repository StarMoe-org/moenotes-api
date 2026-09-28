use crate::{ClientError, ErrorKind};
use std::{
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

const LIMIT: usize = 64 * 1024;

fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidConfig)
}

pub(crate) fn read(path: &Path) -> Result<Zeroizing<Vec<u8>>, ClientError> {
    if !std::fs::symlink_metadata(path)
        .map_err(|_| invalid())?
        .is_file()
    {
        return Err(invalid());
    }
    let file = std::fs::File::open(path).map_err(|_| invalid())?;
    let meta = file.metadata().map_err(|_| invalid())?;
    if !meta.is_file() || meta.len() > LIMIT as u64 {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    #[cfg(not(unix))]
    return Err(invalid());
    #[cfg(unix)]
    {
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() > LIMIT {
            return Err(invalid());
        }
        Ok(bytes)
    }
}

pub(crate) fn create(path: &Path, bytes: &[u8]) -> Result<(), ClientError> {
    if bytes.len() > LIMIT {
        return Err(invalid());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata = std::fs::symlink_metadata(parent).map_err(|_| invalid())?;
    if !metadata.is_dir() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    #[cfg(not(unix))]
    return Err(invalid());
    #[cfg(unix)]
    {
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| invalid())?;
        temporary.write_all(bytes).map_err(|_| invalid())?;
        temporary.as_file().sync_all().map_err(|_| invalid())?;
        temporary.persist_noclobber(path).map_err(|_| invalid())?;
        Ok(())
    }
}

pub(crate) fn check_new(path: &Path) -> Result<(), ClientError> {
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(invalid());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let meta = std::fs::symlink_metadata(parent).map_err(|_| invalid())?;
    if !meta.is_dir() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    #[cfg(not(unix))]
    return Err(invalid());
    #[cfg(unix)]
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn private_bounded_files_never_overwrite_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("secret.json");
        create(&path, b"synthetic").unwrap();
        assert_eq!(&**read(&path).unwrap(), b"synthetic");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
        assert!(create(&path, b"overwrite").is_err());
        let link = dir.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(create(&link, b"overwrite").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic");
        assert!(create(&dir.path().join("large"), &vec![0; LIMIT + 1]).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&path).is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(create(&dir.path().join("public-parent"), b"no").is_err());
    }
}
