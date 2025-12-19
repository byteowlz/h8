//! Configuration management for h8.

use std::fs;
use std::path::Path;

use config::{Config, Environment, File, FileFormat};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths::AppPaths;

const APP_NAME: &str = "h8";

/// Calendar view mode.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CalendarView {
    /// Detailed list view with times and locations
    #[default]
    List,
    /// Gantt-style timeline chart
    Gantt,
    /// Compact view grouped by date
    Compact,
}

impl std::fmt::Display for CalendarView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalendarView::List => write!(f, "list"),
            CalendarView::Gantt => write!(f, "gantt"),
            CalendarView::Compact => write!(f, "compact"),
        }
    }
}

impl std::str::FromStr for CalendarView {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "list" => Ok(CalendarView::List),
            "gantt" => Ok(CalendarView::Gantt),
            "compact" => Ok(CalendarView::Compact),
            _ => Err(format!("unknown view: {} (valid: list, gantt, compact)", s)),
        }
    }
}

/// Calendar display configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarConfig {
    /// Default view mode for calendar displays.
    pub default_view: CalendarView,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            default_view: CalendarView::List,
        }
    }
}

/// Application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Primary email account.
    pub account: String,
    /// Timezone for display (e.g., "Europe/Berlin").
    pub timezone: String,
    /// URL of the Python EWS service.
    pub service_url: String,
    /// Free slots configuration.
    pub free_slots: FreeSlotsConfig,
    /// Mail configuration.
    #[serde(default)]
    pub mail: MailConfig,
    /// Calendar display configuration.
    #[serde(default)]
    pub calendar: CalendarConfig,
    /// People aliases (name -> email).
    #[serde(default)]
    pub people: std::collections::HashMap<String, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            account: "your.email@example.com".to_string(),
            timezone: "Europe/Berlin".to_string(),
            service_url: "http://127.0.0.1:8787".to_string(),
            free_slots: FreeSlotsConfig::default(),
            mail: MailConfig::default(),
            calendar: CalendarConfig::default(),
            people: std::collections::HashMap::new(),
        }
    }
}

/// Free slots finder configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FreeSlotsConfig {
    /// Start hour of working day (0-23).
    pub start_hour: u8,
    /// End hour of working day (0-23).
    pub end_hour: u8,
    /// Whether to exclude weekends from free slots.
    pub exclude_weekends: bool,
}

impl Default for FreeSlotsConfig {
    fn default() -> Self {
        Self {
            start_hour: 9,
            end_hour: 17,
            exclude_weekends: true,
        }
    }
}

/// Mail-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MailConfig {
    /// Override data directory for mail storage.
    pub data_dir: Option<String>,
    /// Editor command (defaults to $EDITOR).
    pub editor: Option<String>,
    /// Pager command for viewing messages.
    pub pager: String,
    /// Folders to sync.
    pub sync_folders: Vec<String>,
    /// Email signature.
    pub signature: String,
    /// Compose settings.
    #[serde(default)]
    pub compose: ComposeConfig,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            editor: None,
            pager: "less -R".to_string(),
            sync_folders: vec![
                "inbox".to_string(),
                "sent".to_string(),
                "drafts".to_string(),
            ],
            signature: String::new(),
            compose: ComposeConfig::default(),
        }
    }
}

/// Compose settings for email composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ComposeConfig {
    /// Format for composing (text or html).
    pub format: String,
    /// Quote prefix style for replies.
    pub quote_style: String,
    /// Whether to include signature in new messages.
    pub include_signature: bool,
}

impl Default for ComposeConfig {
    fn default() -> Self {
        Self {
            format: "text".to_string(),
            quote_style: "> ".to_string(),
            include_signature: true,
        }
    }
}

impl AppConfig {
    /// Resolve a person alias to email address.
    ///
    /// If the alias is found in config, returns the mapped email.
    /// If the alias contains '@', returns it as-is (assumed to be an email).
    /// Otherwise, returns an error.
    pub fn resolve_person(&self, alias: &str) -> std::result::Result<String, String> {
        // Case-insensitive lookup
        for (name, email) in &self.people {
            if name.eq_ignore_ascii_case(alias) {
                return Ok(email.clone());
            }
        }

        // If it looks like an email, use it directly
        if alias.contains('@') {
            return Ok(alias.to_string());
        }

        // Not found
        let available: Vec<&str> = self.people.keys().map(|s| s.as_str()).collect();
        if available.is_empty() {
            Err(format!(
                "unknown person '{}' (no aliases configured in [people])",
                alias
            ))
        } else {
            Err(format!(
                "unknown person '{}' (available: {})",
                alias,
                available.join(", ")
            ))
        }
    }

    /// Load configuration from paths with environment overlay.
    pub fn load(paths: &AppPaths, account_override: Option<&str>) -> Result<Self> {
        let env_prefix = env_prefix();
        let mut builder = Config::builder()
            .add_source(
                File::from(paths.global_config.as_path())
                    .format(FileFormat::Toml)
                    .required(false),
            )
            .add_source(
                File::from(paths.local_config.as_path())
                    .format(FileFormat::Toml)
                    .required(false),
            )
            .add_source(Environment::with_prefix(&env_prefix).separator("__"));

        if let Some(cli_cfg) = &paths.cli_config {
            builder = builder.add_source(
                File::from(cli_cfg.as_path())
                    .format(FileFormat::Toml)
                    .required(true),
            );
        }

        builder = builder
            .set_default("account", AppConfig::default().account)?
            .set_default("timezone", AppConfig::default().timezone)?
            .set_default("service_url", AppConfig::default().service_url)?
            .set_default("free_slots.start_hour", 9)?
            .set_default("free_slots.end_hour", 17)?
            .set_default("free_slots.exclude_weekends", true)?
            .set_default("mail.pager", "less -R")?
            .set_default("mail.compose.format", "text")?
            .set_default("mail.compose.quote_style", "> ")?
            .set_default("mail.compose.include_signature", true)?;

        let mut config: AppConfig = builder.build()?.try_deserialize()?;

        if let Some(account) = account_override {
            config.account = account.to_string();
        }

        Ok(config)
    }

    /// Write default config to a path.
    pub fn write_default(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::Config(format!("creating config directory {parent:?}: {e}")))?;
        }
        let cfg = AppConfig::default();
        let toml = toml::to_string_pretty(&cfg)
            .map_err(|e| Error::Config(format!("serializing default config: {e}")))?;
        let mut content = String::new();
        content.push_str("# h8 configuration\n");
        content.push_str(
            "# Place this file at $XDG_CONFIG_HOME/h8/config.toml (or ~/.config/h8/config.toml)\n\n",
        );
        content.push_str(&toml);
        content.push('\n');
        fs::write(path, content)
            .map_err(|e| Error::Config(format!("writing config file to {}: {e}", path.display())))
    }

    /// Ensure default config exists, creating it if necessary.
    pub fn ensure_default(path: &Path) -> Result<()> {
        if path.exists() {
            return Ok(());
        }
        Self::write_default(path)
    }
}

/// Generate environment variable prefix from app name.
fn env_prefix() -> String {
    APP_NAME
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.timezone, "Europe/Berlin");
        assert_eq!(config.service_url, "http://127.0.0.1:8787");
        assert_eq!(config.free_slots.start_hour, 9);
        assert_eq!(config.free_slots.end_hour, 17);
        assert!(config.free_slots.exclude_weekends);
    }

    #[test]
    fn test_mail_config_defaults() {
        let config = MailConfig::default();
        assert_eq!(config.pager, "less -R");
        assert_eq!(config.sync_folders, vec!["inbox", "sent", "drafts"]);
        assert!(config.signature.is_empty());
    }

    #[test]
    fn test_compose_config_defaults() {
        let config = ComposeConfig::default();
        assert_eq!(config.format, "text");
        assert_eq!(config.quote_style, "> ");
        assert!(config.include_signature);
    }

    #[test]
    fn test_write_default_config() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.toml");
        AppConfig::write_default(&config_path).unwrap();
        assert!(config_path.exists());
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("account"));
        assert!(content.contains("timezone"));
    }

    #[test]
    fn test_env_prefix() {
        assert_eq!(env_prefix(), "H8");
    }
}
