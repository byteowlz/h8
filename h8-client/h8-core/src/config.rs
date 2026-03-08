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

/// A resource entry in a resource group.
///
/// Supports two config formats:
/// - Simple string: `vw1 = "resource@example.com"` (email only)
/// - Inline table: `vw1 = { email = "resource@example.com", desc = "VW ID7" }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceEntry {
    /// Simple email string.
    Simple(String),
    /// Table with email and optional description.
    Detailed {
        email: String,
        #[serde(default)]
        desc: Option<String>,
    },
}

impl ResourceEntry {
    /// Get the email address.
    pub fn email(&self) -> &str {
        match self {
            ResourceEntry::Simple(e) => e,
            ResourceEntry::Detailed { email, .. } => email,
        }
    }

    /// Get the optional description.
    pub fn desc(&self) -> Option<&str> {
        match self {
            ResourceEntry::Simple(_) => None,
            ResourceEntry::Detailed { desc, .. } => desc.as_deref(),
        }
    }
}

/// A resource group maps alias names to ResourceEntry values.
pub type ResourceGroup = std::collections::HashMap<String, ResourceEntry>;

/// A named location with coordinates for trip planning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    /// Human-readable address.
    pub address: String,
    /// Latitude.
    pub lat: f64,
    /// Longitude.
    pub lon: f64,
    /// Optional train station name (for DB routing).
    #[serde(default)]
    pub station: Option<String>,
}

/// Trip planning configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TripConfig {
    /// Default origin alias (e.g., "work").
    pub default_origin: String,
    /// Buffer minutes to add before/after calculated travel time.
    pub buffer_minutes: u32,
    /// Round travel durations up to the nearest N minutes (0 = no rounding).
    /// E.g., 15 rounds 4h22m -> 4h30m; 30 rounds 4h22m -> 4h30m; 60 rounds -> 5h.
    pub round_minutes: u32,
    /// Car routing provider: "osrm" (free, global, default) or "openrouteservice".
    pub car_provider: String,
    /// Transit routing provider: "db" (Deutsche Bahn), "sbb" (Swiss), etc.
    pub transit_provider: String,
    /// Optional country code (ISO 3166-1 alpha-2) to bias geocoding results.
    /// None = worldwide search. E.g., "de", "us", "ch".
    #[serde(default)]
    pub country: Option<String>,
    /// OpenRouteService API key (only needed if car_provider = "openrouteservice").
    #[serde(default)]
    pub openrouteservice_key: Option<String>,
    /// Named locations (alias -> Location).
    #[serde(default)]
    pub locations: std::collections::HashMap<String, Location>,
}

impl Default for TripConfig {
    fn default() -> Self {
        Self {
            default_origin: "work".to_string(),
            buffer_minutes: 15,
            round_minutes: 15,
            car_provider: "osrm".to_string(),
            transit_provider: "db".to_string(),
            country: None,
            openrouteservice_key: None,
            locations: std::collections::HashMap::new(),
        }
    }
}

impl TripConfig {
    /// Resolve a location alias or return None.
    pub fn resolve_location(&self, alias: &str) -> Option<&Location> {
        for (name, loc) in &self.locations {
            if name.eq_ignore_ascii_case(alias) {
                return Some(loc);
            }
        }
        None
    }

    /// Get the default origin location.
    pub fn default_origin_location(&self) -> Option<&Location> {
        self.resolve_location(&self.default_origin)
    }
}

/// Unsubscribe configuration for bulk email unsubscribe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UnsubscribeConfig {
    /// Safe senders - never unsubscribe from these (substring match).
    pub safe_senders: Vec<String>,
    /// Auto-approve patterns - trust these domains for auto-confirm.
    pub trusted_unsubscribe_domains: Vec<String>,
    /// Block patterns - never visit these domains.
    pub blocked_patterns: Vec<String>,
    /// Default to dry run (require --execute to actually unsubscribe).
    pub default_dry_run: bool,
    /// Seconds to wait between HTTP requests.
    pub rate_limit_seconds: f64,
    /// Maximum emails to process per run.
    pub max_emails_per_run: usize,
}

impl Default for UnsubscribeConfig {
    fn default() -> Self {
        Self {
            safe_senders: Vec::new(),
            trusted_unsubscribe_domains: Vec::new(),
            blocked_patterns: Vec::new(),
            default_dry_run: true,
            rate_limit_seconds: 2.0,
            max_emails_per_run: 50,
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
    /// Trip planning configuration.
    #[serde(default)]
    pub trip: TripConfig,
    /// Unsubscribe configuration.
    #[serde(default)]
    pub unsubscribe: UnsubscribeConfig,
    /// People aliases (name -> email).
    #[serde(default)]
    pub people: std::collections::HashMap<String, String>,
    /// Resource groups (group_name -> { alias -> email/entry }).
    #[serde(default)]
    pub resources: std::collections::HashMap<String, ResourceGroup>,
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
            trip: TripConfig::default(),
            unsubscribe: UnsubscribeConfig::default(),
            people: std::collections::HashMap::new(),
            resources: std::collections::HashMap::new(),
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

    /// Get a resource group by name (case-insensitive).
    pub fn resource_group(&self, name: &str) -> Option<(&str, &ResourceGroup)> {
        for (group_name, group) in &self.resources {
            if group_name.eq_ignore_ascii_case(name) {
                return Some((group_name, group));
            }
        }
        None
    }

    /// Resolve a resource alias within a group. Returns (email, desc).
    pub fn resolve_resource(
        &self,
        group_name: &str,
        alias: &str,
    ) -> std::result::Result<(String, Option<String>), String> {
        let (_, group) = self.resource_group(group_name).ok_or_else(|| {
            let available: Vec<&str> = self.resources.keys().map(|s| s.as_str()).collect();
            if available.is_empty() {
                format!("unknown resource group '{}' (no groups configured in [resources])", group_name)
            } else {
                format!("unknown resource group '{}' (available: {})", group_name, available.join(", "))
            }
        })?;
        for (entry_alias, entry) in group {
            if entry_alias.eq_ignore_ascii_case(alias) {
                return Ok((entry.email().to_string(), entry.desc().map(|s| s.to_string())));
            }
        }
        let available: Vec<&str> = group.keys().map(|s| s.as_str()).collect();
        Err(format!(
            "unknown resource '{}' in group '{}' (available: {})",
            alias, group_name, available.join(", ")
        ))
    }

    /// Find which resource group contains a given alias. Returns (group_name, alias, email, desc).
    pub fn find_resource_by_alias(&self, alias: &str) -> Option<(String, String, String, Option<String>)> {
        for (group_name, group) in &self.resources {
            for (entry_alias, entry) in group {
                if entry_alias.eq_ignore_ascii_case(alias) {
                    return Some((
                        group_name.clone(),
                        entry_alias.clone(),
                        entry.email().to_string(),
                        entry.desc().map(|s| s.to_string()),
                    ));
                }
            }
        }
        None
    }

    /// Get all resource group names.
    pub fn resource_group_names(&self) -> Vec<&str> {
        self.resources.keys().map(|s| s.as_str()).collect()
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
