//! User-global CLI config: `~/.noesora/config.json` → `{"default_vault":"<absolute path>"}`.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use noesora_engine::vault::{self, VaultError};
use serde::{Deserialize, Serialize};

const CONFIG_DIRNAME: &str = ".noesora";
const CONFIG_FILENAME: &str = "config.json";

#[derive(Serialize, Deserialize)]
struct Config {
    default_vault: String,
}

#[derive(Debug)]
pub enum ConfigError {
    NoHome,
    NotAVault(PathBuf),
    NoDefault(PathBuf),
    BadConfig(PathBuf, String),
    Vault(VaultError),
    InvalidPath(PathBuf),
    Io(PathBuf, io::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NoHome => {
                write!(f, "cannot locate home directory: HOME is not set")
            }
            ConfigError::NotAVault(path) => write!(
                f,
                "no vault at {}; run `noesora init {}` first",
                path.display(),
                path.display()
            ),
            ConfigError::NoDefault(file) => write!(
                f,
                "no default vault configured ({} is missing); run `noesora vault use <path>` first",
                file.display()
            ),
            ConfigError::BadConfig(file, why) => write!(
                f,
                "invalid config {}: {why}; run `noesora vault use <path>` to rewrite it",
                file.display()
            ),
            ConfigError::Vault(err) => write!(f, "{err}"),
            ConfigError::InvalidPath(path) => {
                write!(f, "vault path {} is not valid UTF-8", path.display())
            }
            ConfigError::Io(path, err) => {
                write!(f, "could not access {}: {err}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<VaultError> for ConfigError {
    fn from(value: VaultError) -> Self {
        ConfigError::Vault(value)
    }
}

/// Home directory from the process environment (`HOME`, then `USERPROFILE`).
pub fn home_dir() -> Result<PathBuf, ConfigError> {
    let non_empty = |name: &str| std::env::var_os(name).filter(|v: &OsString| !v.is_empty());
    non_empty("HOME")
        .or_else(|| non_empty("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or(ConfigError::NoHome)
}

pub fn config_file(home: &Path) -> PathBuf {
    home.join(CONFIG_DIRNAME).join(CONFIG_FILENAME)
}

/// The path must be a vault root itself: a parent directory that holds a vault is not accepted.
pub fn validate_vault(path: &Path) -> Result<PathBuf, ConfigError> {
    let root = fs::canonicalize(path).map_err(|err| ConfigError::Io(path.to_path_buf(), err))?;
    if !vault::vault_file(&root).is_file() {
        return Err(ConfigError::NotAVault(root));
    }
    vault::read_vault(&root)?;
    Ok(root)
}

/// Validate `path` as a vault root and record its absolute path as the default vault.
/// Config is untouched when validation fails.
pub fn set_default_vault(home: &Path, path: &Path) -> Result<(PathBuf, PathBuf), ConfigError> {
    let root = validate_vault(path)?;
    let default_vault = root
        .to_str()
        .ok_or_else(|| ConfigError::InvalidPath(root.clone()))?
        .to_string();
    let file = config_file(home);
    let io_err = |err| ConfigError::Io(file.clone(), err);
    let dir = file.parent().expect("config file has a parent");
    fs::create_dir_all(dir).map_err(io_err)?;
    let encoded = serde_json::to_string_pretty(&Config { default_vault }).expect("config json");
    let tmp = dir.join(format!("{CONFIG_FILENAME}.{}.tmp", std::process::id()));
    fs::write(&tmp, encoded + "\n")
        .and_then(|()| fs::rename(&tmp, &file))
        .map_err(|err| {
            let _ = fs::remove_file(&tmp);
            io_err(err)
        })?;
    Ok((root, file))
}

/// The vault recorded by `vault use`, revalidated. A missing or malformed config, or a default
/// that is no longer a vault root, is an error; callers never fall back to another vault.
pub fn load_default_vault(home: &Path) -> Result<PathBuf, ConfigError> {
    let file = config_file(home);
    let raw = fs::read_to_string(&file).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => ConfigError::NoDefault(file.clone()),
        _ => ConfigError::Io(file.clone(), err),
    })?;
    let config: Config = serde_json::from_str(&raw)
        .map_err(|err| ConfigError::BadConfig(file.clone(), err.to_string()))?;
    let path = PathBuf::from(&config.default_vault);
    if !path.is_absolute() {
        return Err(ConfigError::BadConfig(
            file,
            format!(
                "default_vault {:?} is not an absolute path",
                config.default_vault
            ),
        ));
    }
    validate_vault(&path)
}
