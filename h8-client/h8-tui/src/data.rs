//! Data layer for integrating with h8-core.
//!
//! Provides email loading from Maildir and sync database.

use std::path::PathBuf;

use h8_core::types::MessageSync;
use h8_core::{AppPaths, Database, Maildir};

use crate::app::FolderInfo;

/// Result type for data operations.
pub type Result<T> = std::result::Result<T, DataError>;

/// Errors that can occur during data operations.
#[derive(Debug)]
#[allow(dead_code)]
pub enum DataError {
    /// h8-core error.
    Core(h8_core::Error),
    /// No account configured.
    NoAccount,
    /// Database not found.
    DatabaseNotFound(PathBuf),
    /// Maildir not found.
    MaildirNotFound(PathBuf),
    /// Generic I/O error.
    Io(std::io::Error),
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataError::Core(e) => write!(f, "Core error: {}", e),
            DataError::NoAccount => write!(f, "No account configured"),
            DataError::DatabaseNotFound(p) => write!(f, "Database not found: {}", p.display()),
            DataError::MaildirNotFound(p) => write!(f, "Maildir not found: {}", p.display()),
            DataError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for DataError {}

impl From<h8_core::Error> for DataError {
    fn from(e: h8_core::Error) -> Self {
        DataError::Core(e)
    }
}

impl From<std::io::Error> for DataError {
    fn from(e: std::io::Error) -> Self {
        DataError::Io(e)
    }
}

/// Data source for the TUI, wrapping h8-core components.
pub struct DataSource {
    /// Application paths.
    paths: AppPaths,
    /// Optional override for the mail data directory root.
    mail_root_override: Option<PathBuf>,
    /// Current account email address.
    account: Option<String>,
    /// Database handle (lazy initialized).
    db: Option<Database>,
    /// Maildir handle (lazy initialized).
    maildir: Option<Maildir>,
}

impl DataSource {
    /// Create a new data source.
    pub fn new() -> Result<Self> {
        let paths = AppPaths::discover(None).map_err(DataError::from)?;
        Ok(Self::with_paths(paths))
    }

    /// Create a data source from pre-discovered paths.
    pub fn with_paths(paths: AppPaths) -> Self {
        Self {
            paths,
            mail_root_override: None,
            account: None,
            db: None,
            maildir: None,
        }
    }

    /// Set the current account.
    pub fn set_account(&mut self, account: &str) -> Result<()> {
        self.account = Some(account.to_string());
        self.db = None;
        self.maildir = None;
        Ok(())
    }

    /// Override the root directory used for mail data.
    pub fn set_mail_data_dir(&mut self, dir: PathBuf) {
        self.mail_root_override = Some(dir);
        self.db = None;
        self.maildir = None;
    }

    /// Get the root directory for mail storage.
    fn mail_root(&self) -> PathBuf {
        self.mail_root_override
            .clone()
            .unwrap_or_else(|| self.paths.data_dir.join("mail"))
    }

    /// Get the current account, if any.
    #[allow(dead_code)]
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    /// Detect available accounts from the mail directory.
    pub fn detect_accounts(&self) -> Result<Vec<String>> {
        let mail_base = self.mail_root();
        if !mail_base.exists() {
            return Ok(Vec::new());
        }

        let mut accounts = Vec::new();
        for entry in std::fs::read_dir(&mail_base)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // Skip hidden directories
                if !name_str.starts_with('.') {
                    accounts.push(name_str.to_string());
                }
            }
        }
        accounts.sort();
        Ok(accounts)
    }

    /// Get or initialize the database.
    fn get_db(&mut self) -> Result<&Database> {
        if self.db.is_none() {
            let account = self.account.as_ref().ok_or(DataError::NoAccount)?;
            let account_dir = self.mail_root().join(account);

            // Ensure account directory exists
            std::fs::create_dir_all(&account_dir)?;
            let db_path = account_dir.join(".sync.db");

            let db = Database::open(&db_path)?;
            self.db = Some(db);
        }
        Ok(self.db.as_ref().unwrap())
    }

    /// Get or initialize the maildir.
    fn get_maildir(&mut self) -> Result<&Maildir> {
        if self.maildir.is_none() {
            let account = self.account.as_ref().ok_or(DataError::NoAccount)?;
            let mail_dir = self.mail_root().join(account);

            // Ensure directory exists before initializing maildir
            std::fs::create_dir_all(&mail_dir)?;
            let maildir = Maildir::new(mail_dir, account)?;
            // Initialize if needed
            maildir.init()?;
            self.maildir = Some(maildir);
        }
        Ok(self.maildir.as_ref().unwrap())
    }

    /// Load emails from the sync database for a folder.
    pub fn load_emails(&mut self, folder: &str, limit: usize) -> Result<Vec<MessageSync>> {
        let db = self.get_db()?;
        let messages = db.list_messages(folder, limit)?;
        Ok(messages)
    }

    /// Load folder information.
    pub fn load_folders(&mut self) -> Result<Vec<FolderInfo>> {
        let maildir = self.get_maildir()?;
        let folder_names = maildir.list_folders()?;

        let mut folders = Vec::new();
        for name in folder_names {
            let (unread, read) = maildir.count(&name)?;
            let display_name = folder_display_name(&name);
            folders.push(FolderInfo {
                name: name.clone(),
                display_name,
                unread_count: unread,
                total_count: unread + read,
            });
        }

        // Ensure standard folders are first in consistent order
        let order = ["inbox", "sent", "drafts", "trash", "archive"];
        folders.sort_by(|a, b| {
            let a_pos = order.iter().position(|&o| o == a.name).unwrap_or(999);
            let b_pos = order.iter().position(|&o| o == b.name).unwrap_or(999);
            a_pos.cmp(&b_pos).then_with(|| a.name.cmp(&b.name))
        });

        Ok(folders)
    }

    /// Get a single email by local ID.
    #[allow(dead_code)]
    pub fn get_email(&mut self, local_id: &str) -> Result<Option<MessageSync>> {
        let db = self.get_db()?;
        let msg = db.get_message(local_id)?;
        Ok(msg)
    }

    /// Get email content from Maildir.
    pub fn get_email_content(&mut self, folder: &str, local_id: &str) -> Result<Option<String>> {
        let maildir = self.get_maildir()?;
        if let Some(msg) = maildir.get(folder, local_id)? {
            let content = msg.read_content()?;
            Ok(Some(content))
        } else {
            Ok(None)
        }
    }

    /// Delete emails by local IDs (permanent deletion).
    #[allow(dead_code)]
    pub fn delete_emails(&mut self, folder: &str, local_ids: &[&str]) -> Result<usize> {
        let maildir = self.get_maildir()?;
        let mut deleted = 0;

        for id in local_ids {
            if maildir.delete(folder, id)? {
                deleted += 1;
            }
        }

        // Also remove from database
        let db = self.get_db()?;
        for id in local_ids {
            db.delete_message(id)?;
        }

        Ok(deleted)
    }

    /// Move emails to trash.
    pub fn trash_emails(&mut self, folder: &str, local_ids: &[&str]) -> Result<usize> {
        let maildir = self.get_maildir()?;
        let mut moved = 0;

        for id in local_ids {
            if maildir.move_to(folder, id, "trash")?.is_some() {
                moved += 1;
            }
        }

        // Update folder in database
        let db = self.get_db()?;
        for id in local_ids {
            if let Some(mut msg) = db.get_message(id)? {
                msg.folder = "trash".to_string();
                db.upsert_message(&msg)?;
            }
        }

        Ok(moved)
    }

    /// Mark emails as read.
    pub fn mark_read(&mut self, folder: &str, local_ids: &[&str]) -> Result<usize> {
        let maildir = self.get_maildir()?;
        let mut marked = 0;

        for id in local_ids {
            if let Some(msg) = maildir.get(folder, id)? {
                let mut flags = msg.flags.clone();
                flags.mark_read();
                if maildir.update_flags(folder, id, &flags)?.is_some() {
                    marked += 1;
                }
            }
        }

        // Update in database
        let db = self.get_db()?;
        for id in local_ids {
            if let Some(mut msg) = db.get_message(id)? {
                msg.is_read = true;
                db.upsert_message(&msg)?;
            }
        }

        Ok(marked)
    }

    /// Mark emails as unread.
    pub fn mark_unread(&mut self, folder: &str, local_ids: &[&str]) -> Result<usize> {
        let maildir = self.get_maildir()?;
        let mut marked = 0;

        for id in local_ids {
            if let Some(msg) = maildir.get(folder, id)? {
                let mut flags = msg.flags.clone();
                flags.mark_unread();
                if maildir.update_flags(folder, id, &flags)?.is_some() {
                    marked += 1;
                }
            }
        }

        // Update in database
        let db = self.get_db()?;
        for id in local_ids {
            if let Some(mut msg) = db.get_message(id)? {
                msg.is_read = false;
                db.upsert_message(&msg)?;
            }
        }

        Ok(marked)
    }

    /// Search emails in current folder.
    pub fn search_emails(
        &mut self,
        folder: &str,
        query: &str,
        mode: SearchMode,
        limit: usize,
    ) -> Result<Vec<MessageSync>> {
        let all = self.load_emails(folder, limit * 10)?; // Load more to filter
        let query_lower = query.to_lowercase();

        let filtered: Vec<MessageSync> = all
            .into_iter()
            .filter(|msg| match mode {
                SearchMode::Subject => msg
                    .subject
                    .as_ref()
                    .map(|s| s.to_lowercase().contains(&query_lower))
                    .unwrap_or(false),
                SearchMode::From => msg
                    .from_addr
                    .as_ref()
                    .map(|s| s.to_lowercase().contains(&query_lower))
                    .unwrap_or(false),
                SearchMode::All => {
                    msg.subject
                        .as_ref()
                        .map(|s| s.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
                        || msg
                            .from_addr
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&query_lower))
                            .unwrap_or(false)
                }
            })
            .take(limit)
            .collect();

        Ok(filtered)
    }
}

impl Default for DataSource {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| {
            let paths = AppPaths::discover(None).expect("AppPaths discovery failed");
            Self::with_paths(paths)
        })
    }
}

/// Search mode for filtering emails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Subject,
    From,
    All,
}

/// Get a display name for a folder.
fn folder_display_name(name: &str) -> String {
    match name {
        "inbox" => "Inbox".to_string(),
        "sent" => "Sent".to_string(),
        "drafts" => "Drafts".to_string(),
        "trash" => "Trash".to_string(),
        "archive" => "Archive".to_string(),
        _ => {
            // Capitalize first letter
            let mut chars = name.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => name.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_folder_display_name() {
        assert_eq!(folder_display_name("inbox"), "Inbox");
        assert_eq!(folder_display_name("sent"), "Sent");
        assert_eq!(folder_display_name("custom"), "Custom");
        assert_eq!(folder_display_name("my-folder"), "My-folder");
    }

    #[test]
    fn test_data_source_new() {
        // This should work even without an account
        let ds = DataSource::new();
        assert!(ds.is_ok());
    }

    #[test]
    fn test_data_source_no_account() {
        let mut ds = DataSource::new().unwrap();
        let result = ds.load_emails("inbox", 100);
        assert!(matches!(result, Err(DataError::NoAccount)));
    }

    #[test]
    fn test_detect_accounts_empty() {
        let ds = DataSource::new().unwrap();
        // Just verify it doesn't crash - actual accounts depend on filesystem
        let _ = ds.detect_accounts();
    }
}
