//! Path discovery and management for h8.

use std::env;
use std::path::PathBuf;

use crate::error::{Error, Result};

const APP_NAME: &str = "h8";

/// Application paths for config, state, and data directories.
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Global config file path (e.g., ~/.config/h8/config.toml)
    pub global_config: PathBuf,
    /// Local config file path (current directory config.toml)
    pub local_config: PathBuf,
    /// CLI-specified config file path
    pub cli_config: Option<PathBuf>,
    /// State directory for runtime data (e.g., ~/.local/state/h8)
    pub state_dir: PathBuf,
    /// Data directory for persistent data (e.g., ~/.local/share/h8)
    pub data_dir: PathBuf,
}

impl AppPaths {
    /// Discover application paths based on XDG conventions and CLI options.
    pub fn discover(cli_config: Option<PathBuf>) -> Result<Self> {
        let global_config = default_config_dir()?.join("config.toml");
        let local_config = env::current_dir()
            .map_err(|e| Error::Path(format!("determining current directory: {e}")))?
            .join("config.toml");
        let cli_config = cli_config.map(expand_path).transpose()?;
        let state_dir = default_state_dir()?;
        let data_dir = default_data_dir()?;

        Ok(Self {
            global_config,
            local_config,
            cli_config,
            state_dir,
            data_dir,
        })
    }

    /// Get the mail data directory for an account.
    pub fn mail_dir(&self, account: &str) -> PathBuf {
        self.data_dir.join("mail").join(account)
    }

    /// Get the sync database path for an account.
    pub fn sync_db_path(&self, account: &str) -> PathBuf {
        self.mail_dir(account).join(".sync.db")
    }
}

/// Expand shell variables and tilde in a path.
pub fn expand_path(path: PathBuf) -> Result<PathBuf> {
    if let Some(text) = path.to_str() {
        expand_str_path(text)
    } else {
        Ok(path)
    }
}

/// Expand shell variables and tilde in a path string.
pub fn expand_str_path(text: &str) -> Result<PathBuf> {
    let expanded =
        shellexpand::full(text).map_err(|e| Error::Path(format!("expanding path: {e}")))?;
    Ok(PathBuf::from(expanded.to_string()))
}

/// Get the default config directory following XDG conventions.
pub fn default_config_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        let mut path = PathBuf::from(dir);
        path.push(APP_NAME);
        return Ok(path);
    }
    if let Some(mut dir) = dirs::config_dir() {
        dir.push(APP_NAME);
        return Ok(dir);
    }
    dirs::home_dir()
        .map(|home| home.join(".config").join(APP_NAME))
        .ok_or_else(|| Error::Path("unable to determine configuration directory".into()))
}

/// Get the default state directory following XDG conventions.
pub fn default_state_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        let mut path = PathBuf::from(dir);
        path.push(APP_NAME);
        return Ok(path);
    }
    if let Some(mut dir) = dirs::state_dir() {
        dir.push(APP_NAME);
        return Ok(dir);
    }
    dirs::home_dir()
        .map(|home| home.join(".local").join("state").join(APP_NAME))
        .ok_or_else(|| Error::Path("unable to determine state directory".into()))
}

/// Get the default data directory following XDG conventions.
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        let mut path = PathBuf::from(dir);
        path.push(APP_NAME);
        return Ok(path);
    }
    if let Some(mut dir) = dirs::data_dir() {
        dir.push(APP_NAME);
        return Ok(dir);
    }
    dirs::home_dir()
        .map(|home| home.join(".local").join("share").join(APP_NAME))
        .ok_or_else(|| Error::Path("unable to determine data directory".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_paths_discover() {
        let paths = AppPaths::discover(None).unwrap();
        assert!(paths.global_config.ends_with("config.toml"));
        assert!(paths.local_config.ends_with("config.toml"));
    }

    #[test]
    fn test_mail_dir() {
        let paths = AppPaths::discover(None).unwrap();
        let mail_dir = paths.mail_dir("test@example.com");
        assert!(mail_dir.ends_with("mail/test@example.com"));
    }

    #[test]
    fn test_sync_db_path() {
        let paths = AppPaths::discover(None).unwrap();
        let db_path = paths.sync_db_path("test@example.com");
        assert!(db_path.ends_with("mail/test@example.com/.sync.db"));
    }

    #[test]
    fn test_expand_path_tilde() {
        let expanded = expand_str_path("~/test").unwrap();
        assert!(!expanded.to_string_lossy().contains('~'));
    }
}
