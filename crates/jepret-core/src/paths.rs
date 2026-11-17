//! Well-known locations. All functions are cheap and do not create dirs;
//! call [`ensure_dir`] before writing.

use std::path::{Path, PathBuf};

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `%APPDATA%\Jepret`
pub fn config_dir() -> PathBuf {
    env_dir("APPDATA").join(crate::APP_NAME)
}

/// `%LOCALAPPDATA%\Jepret`
pub fn data_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").join(crate::APP_NAME)
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn state_file() -> PathBuf {
    config_dir().join("state.toml")
}

pub fn history_db() -> PathBuf {
    data_dir().join("history.db")
}

pub fn thumbs_dir() -> PathBuf {
    data_dir().join("thumbs")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Temp folder for captures that must exist as a file but were not saved.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(crate::APP_NAME)
}

/// `%USERPROFILE%\Pictures\Jepret`
pub fn default_output_dir() -> PathBuf {
    env_dir("USERPROFILE").join("Pictures").join(crate::APP_NAME)
}

pub fn ensure_dir(p: &Path) -> crate::Result<()> {
    std::fs::create_dir_all(p)?;
    Ok(())
}
