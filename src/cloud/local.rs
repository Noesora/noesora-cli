use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use noesora_engine::Store;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::CloudError;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn cloud_root(store: &Store) -> Result<PathBuf, CloudError> {
    let path = store.root().join("cloud");
    ensure_private_directory(&path)?;
    Ok(path)
}

pub(crate) fn account_path(store: &Store) -> Result<PathBuf, CloudError> {
    Ok(cloud_root(store)?.join("account.json"))
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, CloudError> {
    let bytes = match read_private_file(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(CloudError::local_store(
                "Could not read Cloud sidecar data.",
            ))
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| CloudError::integrity("Cloud sidecar data is malformed."))
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CloudError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| CloudError::local_store("Could not encode Cloud sidecar data."))?;
    write_atomic(path, &bytes)
}

pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, CloudError> {
    read_private_file(path)
        .map_err(|_| CloudError::local_store("Could not read a pending Cloud outbox object."))
}

pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<(), CloudError> {
    write_atomic(path, bytes)
}

pub(crate) fn remove_file(path: &Path) -> Result<bool, CloudError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(CloudError::local_store(
            "Could not inspect Cloud sidecar data.",
        )),
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            Err(CloudError::local_store(
                "Cloud sidecar path is not a regular file.",
            ))
        }
        Ok(_) => {
            fs::remove_file(path)
                .map_err(|_| CloudError::local_store("Could not remove Cloud sidecar data."))?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(true)
        }
    }
}

pub(crate) fn outbox_root(store: &Store) -> Result<PathBuf, CloudError> {
    let path = cloud_root(store)?.join("outbox");
    ensure_private_directory(&path)?;
    Ok(path)
}

pub(crate) fn outbox_directory(store: &Store, preview_sha256: &str) -> Result<PathBuf, CloudError> {
    let key = preview_sha256
        .strip_prefix("sha256:")
        .ok_or_else(|| CloudError::integrity("Pending preview digest is malformed."))?;
    if key.len() != 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CloudError::integrity(
            "Pending preview digest is malformed.",
        ));
    }
    let path = outbox_root(store)?.join(key);
    ensure_private_directory(&path)?;
    Ok(path)
}

pub(crate) fn checkpoint_path(store: &Store, org_id: &str) -> Result<PathBuf, CloudError> {
    let root = cloud_root(store)?.join("checkpoints");
    ensure_private_directory(&root)?;
    Ok(root.join(format!("{}.json", hash_name(org_id.as_bytes()))))
}

pub(crate) fn hash_name(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn ensure_private_directory(path: &Path) -> Result<(), CloudError> {
    if let Some(parent) = path.parent() {
        if parent.exists() {
            validate_directory(parent)?;
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| {
                    CloudError::local_store("Could not secure Cloud sidecar directory permissions.")
                })?;
            }
            Ok(())
        }
        Ok(_) => Err(CloudError::local_store(
            "Cloud sidecar directory is not a private directory.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| {
                CloudError::local_store("Could not create Cloud sidecar directory.")
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| {
                    CloudError::local_store("Could not secure Cloud sidecar directory permissions.")
                })?;
            }
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(_) => Err(CloudError::local_store(
            "Could not inspect Cloud sidecar directory.",
        )),
    }
}

fn validate_directory(path: &Path) -> Result<(), CloudError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| CloudError::local_store("Could not inspect Cloud sidecar directory."))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(CloudError::local_store(
            "Cloud sidecar directory is not a private directory.",
        ));
    }
    Ok(())
}

fn read_private_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Cloud sidecar has excessive permissions",
            ));
        }
    }
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CloudError> {
    let parent = path
        .parent()
        .ok_or_else(|| CloudError::local_store("Cloud sidecar path has no parent."))?;
    ensure_private_directory(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(CloudError::local_store(
                "Cloud sidecar destination is not a regular file.",
            ));
        }
    }
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".noesora-cloud-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
        .map_err(|_| CloudError::local_store("Could not atomically persist Cloud sidecar data."))?;
    sync_directory(parent)
}

fn sync_directory(path: &Path) -> Result<(), CloudError> {
    let directory = File::open(path)
        .map_err(|_| CloudError::local_store("Could not sync Cloud sidecar directory."))?;
    directory
        .sync_all()
        .map_err(|_| CloudError::local_store("Could not sync Cloud sidecar directory."))
}
