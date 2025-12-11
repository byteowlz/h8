//! Maildir storage implementation.
//!
//! This module provides local email storage using the Maildir format.
//!
//! Directory structure:
//! ```text
//! $XDG_DATA_HOME/h8/mail/<account>/
//!   inbox/
//!     cur/    # read messages
//!     new/    # unread messages
//!     tmp/    # temp during write
//!   sent/
//!   drafts/
//!   trash/
//!   .sync.db  # SQLite sync state
//! ```

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;

/// Standard Maildir folder names.
pub const FOLDER_INBOX: &str = "inbox";
pub const FOLDER_SENT: &str = "sent";
pub const FOLDER_DRAFTS: &str = "drafts";
pub const FOLDER_TRASH: &str = "trash";

/// Maildir subdirectory names.
const SUBDIR_NEW: &str = "new";
const SUBDIR_CUR: &str = "cur";
const SUBDIR_TMP: &str = "tmp";

/// Message flags for Maildir filename encoding.
#[derive(Debug, Clone, Default)]
pub struct MessageFlags {
    /// P: Passed (forwarded)
    pub passed: bool,
    /// R: Replied
    pub replied: bool,
    /// S: Seen (read)
    pub seen: bool,
    /// T: Trashed
    pub trashed: bool,
    /// D: Draft
    pub draft: bool,
    /// F: Flagged (starred/important)
    pub flagged: bool,
}

impl MessageFlags {
    /// Create flags from a Maildir info string (e.g., "2,RS").
    pub fn from_info(info: &str) -> Self {
        let mut flags = Self::default();
        if let Some(flag_str) = info.strip_prefix("2,") {
            for c in flag_str.chars() {
                match c {
                    'P' => flags.passed = true,
                    'R' => flags.replied = true,
                    'S' => flags.seen = true,
                    'T' => flags.trashed = true,
                    'D' => flags.draft = true,
                    'F' => flags.flagged = true,
                    _ => {}
                }
            }
        }
        flags
    }

    /// Convert flags to Maildir info string (e.g., "2,RS").
    pub fn to_info(&self) -> String {
        let mut flags = String::new();
        if self.draft {
            flags.push('D');
        }
        if self.flagged {
            flags.push('F');
        }
        if self.passed {
            flags.push('P');
        }
        if self.replied {
            flags.push('R');
        }
        if self.seen {
            flags.push('S');
        }
        if self.trashed {
            flags.push('T');
        }
        if flags.is_empty() {
            String::new()
        } else {
            format!("2,{}", flags)
        }
    }

    /// Check if message is read.
    pub fn is_read(&self) -> bool {
        self.seen
    }

    /// Mark as read.
    pub fn mark_read(&mut self) {
        self.seen = true;
    }

    /// Mark as unread.
    pub fn mark_unread(&mut self) {
        self.seen = false;
    }
}

/// A message stored in Maildir format.
#[derive(Debug, Clone)]
pub struct MaildirMessage {
    /// Unique message ID (filename without info suffix).
    pub id: String,
    /// Message flags.
    pub flags: MessageFlags,
    /// Full path to the message file.
    pub path: PathBuf,
    /// Folder containing the message.
    pub folder: String,
    /// Whether the message is in new/ (unread) or cur/ (read).
    pub is_new: bool,
}

impl MaildirMessage {
    /// Read the message content.
    pub fn read_content(&self) -> Result<String> {
        let mut file = File::open(&self.path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Ok(content)
    }

    /// Read the message content as bytes.
    pub fn read_bytes(&self) -> Result<Vec<u8>> {
        let mut file = File::open(&self.path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        Ok(content)
    }
}

/// Maildir storage manager for an account.
pub struct Maildir {
    /// Base path for the account's mail storage.
    base_path: PathBuf,
    /// Account email address.
    account: String,
    /// Hostname for generating unique IDs.
    hostname: String,
}

impl Maildir {
    /// Create a new Maildir manager for an account.
    pub fn new(base_path: PathBuf, account: &str) -> Result<Self> {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "localhost".to_string());

        Ok(Self {
            base_path,
            account: account.to_string(),
            hostname,
        })
    }

    /// Get the base path for this Maildir.
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    /// Get the account name.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// Initialize the Maildir structure, creating all necessary directories.
    pub fn init(&self) -> Result<()> {
        self.init_folder(FOLDER_INBOX)?;
        self.init_folder(FOLDER_SENT)?;
        self.init_folder(FOLDER_DRAFTS)?;
        self.init_folder(FOLDER_TRASH)?;
        Ok(())
    }

    /// Initialize a specific folder.
    pub fn init_folder(&self, folder: &str) -> Result<()> {
        let folder_path = self.folder_path(folder);
        fs::create_dir_all(folder_path.join(SUBDIR_NEW))?;
        fs::create_dir_all(folder_path.join(SUBDIR_CUR))?;
        fs::create_dir_all(folder_path.join(SUBDIR_TMP))?;
        Ok(())
    }

    /// Get the path to a folder.
    pub fn folder_path(&self, folder: &str) -> PathBuf {
        self.base_path.join(folder)
    }

    /// Generate a unique message ID.
    fn generate_unique_id(&self) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0);
        let pid = std::process::id();
        let random: u32 = rand::random();
        format!("{}.{}.{:x}.{}", timestamp, pid, random, self.hostname)
    }

    /// Store a new message in a folder.
    ///
    /// Uses the Maildir delivery protocol:
    /// 1. Write to tmp/ with unique filename
    /// 2. Move to new/ or cur/ depending on flags
    pub fn store(
        &self,
        folder: &str,
        content: &[u8],
        flags: &MessageFlags,
    ) -> Result<MaildirMessage> {
        self.init_folder(folder)?;

        let folder_path = self.folder_path(folder);
        let unique_id = self.generate_unique_id();

        // Write to tmp first
        let tmp_path = folder_path.join(SUBDIR_TMP).join(&unique_id);
        let mut file = File::create(&tmp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);

        // Determine destination (new/ or cur/)
        let (dest_subdir, is_new) = if flags.seen {
            (SUBDIR_CUR, false)
        } else {
            (SUBDIR_NEW, true)
        };

        // Build filename with flags
        let info = flags.to_info();
        let filename = if info.is_empty() {
            unique_id.clone()
        } else {
            format!("{}:{}", unique_id, info)
        };

        let dest_path = folder_path.join(dest_subdir).join(&filename);
        fs::rename(&tmp_path, &dest_path)?;

        Ok(MaildirMessage {
            id: unique_id,
            flags: flags.clone(),
            path: dest_path,
            folder: folder.to_string(),
            is_new,
        })
    }

    /// Store a message with a specific ID (for sync operations).
    pub fn store_with_id(
        &self,
        folder: &str,
        content: &[u8],
        flags: &MessageFlags,
        id: &str,
    ) -> Result<MaildirMessage> {
        self.init_folder(folder)?;

        let folder_path = self.folder_path(folder);

        // Write to tmp first
        let tmp_path = folder_path.join(SUBDIR_TMP).join(id);
        let mut file = File::create(&tmp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);

        // Determine destination (new/ or cur/)
        let (dest_subdir, is_new) = if flags.seen {
            (SUBDIR_CUR, false)
        } else {
            (SUBDIR_NEW, true)
        };

        // Build filename with flags
        let info = flags.to_info();
        let filename = if info.is_empty() {
            id.to_string()
        } else {
            format!("{}:{}", id, info)
        };

        let dest_path = folder_path.join(dest_subdir).join(&filename);
        fs::rename(&tmp_path, &dest_path)?;

        Ok(MaildirMessage {
            id: id.to_string(),
            flags: flags.clone(),
            path: dest_path,
            folder: folder.to_string(),
            is_new,
        })
    }

    /// Get a message by ID from a folder.
    pub fn get(&self, folder: &str, id: &str) -> Result<Option<MaildirMessage>> {
        // Search in new/ first, then cur/
        for (subdir, is_new) in [(SUBDIR_NEW, true), (SUBDIR_CUR, false)] {
            let dir_path = self.folder_path(folder).join(subdir);
            if !dir_path.exists() {
                continue;
            }

            for entry in fs::read_dir(&dir_path)? {
                let entry = entry?;
                let filename = entry.file_name();
                let filename_str = filename.to_string_lossy();

                // Extract base ID (before colon)
                let base_id = filename_str.split(':').next().unwrap_or(&filename_str);

                if base_id == id {
                    let flags = self.parse_flags_from_filename(&filename_str);
                    return Ok(Some(MaildirMessage {
                        id: id.to_string(),
                        flags,
                        path: entry.path(),
                        folder: folder.to_string(),
                        is_new,
                    }));
                }
            }
        }

        Ok(None)
    }

    /// List all messages in a folder.
    pub fn list(&self, folder: &str) -> Result<Vec<MaildirMessage>> {
        let mut messages = Vec::new();

        for (subdir, is_new) in [(SUBDIR_NEW, true), (SUBDIR_CUR, false)] {
            let dir_path = self.folder_path(folder).join(subdir);
            if !dir_path.exists() {
                continue;
            }

            for entry in fs::read_dir(&dir_path)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }

                let filename = entry.file_name();
                let filename_str = filename.to_string_lossy();

                // Extract base ID (before colon)
                let base_id = filename_str.split(':').next().unwrap_or(&filename_str);
                let flags = self.parse_flags_from_filename(&filename_str);

                messages.push(MaildirMessage {
                    id: base_id.to_string(),
                    flags,
                    path: entry.path(),
                    folder: folder.to_string(),
                    is_new,
                });
            }
        }

        Ok(messages)
    }

    /// Delete a message by ID from a folder.
    pub fn delete(&self, folder: &str, id: &str) -> Result<bool> {
        if let Some(msg) = self.get(folder, id)? {
            fs::remove_file(&msg.path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Update message flags (moves between new/cur as needed).
    pub fn update_flags(
        &self,
        folder: &str,
        id: &str,
        flags: &MessageFlags,
    ) -> Result<Option<MaildirMessage>> {
        if let Some(msg) = self.get(folder, id)? {
            let folder_path = self.folder_path(folder);

            // Determine new location
            let (new_subdir, is_new) = if flags.seen {
                (SUBDIR_CUR, false)
            } else {
                (SUBDIR_NEW, true)
            };

            // Build new filename
            let info = flags.to_info();
            let new_filename = if info.is_empty() {
                id.to_string()
            } else {
                format!("{}:{}", id, info)
            };

            let new_path = folder_path.join(new_subdir).join(&new_filename);

            // Move if needed (even if same dir, filename might change)
            if msg.path != new_path {
                fs::rename(&msg.path, &new_path)?;
            }

            Ok(Some(MaildirMessage {
                id: id.to_string(),
                flags: flags.clone(),
                path: new_path,
                folder: folder.to_string(),
                is_new,
            }))
        } else {
            Ok(None)
        }
    }

    /// Move a message to another folder.
    pub fn move_to(
        &self,
        folder: &str,
        id: &str,
        dest_folder: &str,
    ) -> Result<Option<MaildirMessage>> {
        if let Some(msg) = self.get(folder, id)? {
            // Read content
            let content = msg.read_bytes()?;

            // Store in new folder
            let new_msg = self.store_with_id(dest_folder, &content, &msg.flags, id)?;

            // Delete from old folder
            fs::remove_file(&msg.path)?;

            Ok(Some(new_msg))
        } else {
            Ok(None)
        }
    }

    /// Parse flags from a Maildir filename.
    fn parse_flags_from_filename(&self, filename: &str) -> MessageFlags {
        if let Some(info_start) = filename.find(':') {
            MessageFlags::from_info(&filename[info_start + 1..])
        } else {
            MessageFlags::default()
        }
    }

    /// Count messages in a folder.
    pub fn count(&self, folder: &str) -> Result<(usize, usize)> {
        let mut new_count = 0;
        let mut cur_count = 0;

        let new_path = self.folder_path(folder).join(SUBDIR_NEW);
        if new_path.exists() {
            for entry in fs::read_dir(&new_path)? {
                if entry?.file_type()?.is_file() {
                    new_count += 1;
                }
            }
        }

        let cur_path = self.folder_path(folder).join(SUBDIR_CUR);
        if cur_path.exists() {
            for entry in fs::read_dir(&cur_path)? {
                if entry?.file_type()?.is_file() {
                    cur_count += 1;
                }
            }
        }

        Ok((new_count, cur_count))
    }

    /// List all folders.
    pub fn list_folders(&self) -> Result<Vec<String>> {
        let mut folders = Vec::new();

        if !self.base_path.exists() {
            return Ok(folders);
        }

        for entry in fs::read_dir(&self.base_path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // Skip hidden files/dirs like .sync.db
                if !name_str.starts_with('.') {
                    // Verify it's a valid Maildir (has cur, new, tmp)
                    let path = entry.path();
                    if path.join(SUBDIR_CUR).exists() || path.join(SUBDIR_NEW).exists() {
                        folders.push(name_str.to_string());
                    }
                }
            }
        }

        folders.sort();
        Ok(folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_maildir() -> (TempDir, Maildir) {
        let temp = TempDir::new().unwrap();
        let maildir = Maildir::new(temp.path().to_path_buf(), "test@example.com").unwrap();
        (temp, maildir)
    }

    #[test]
    fn test_message_flags() {
        let mut flags = MessageFlags::default();
        assert!(!flags.is_read());

        flags.mark_read();
        assert!(flags.is_read());
        assert_eq!(flags.to_info(), "2,S");

        flags.flagged = true;
        assert_eq!(flags.to_info(), "2,FS");

        flags.replied = true;
        assert_eq!(flags.to_info(), "2,FRS");
    }

    #[test]
    fn test_flags_from_info() {
        let flags = MessageFlags::from_info("2,RS");
        assert!(flags.replied);
        assert!(flags.seen);
        assert!(!flags.flagged);
    }

    #[test]
    fn test_init_maildir() {
        let (_temp, maildir) = test_maildir();
        maildir.init().unwrap();

        assert!(maildir.folder_path(FOLDER_INBOX).join(SUBDIR_NEW).exists());
        assert!(maildir.folder_path(FOLDER_INBOX).join(SUBDIR_CUR).exists());
        assert!(maildir.folder_path(FOLDER_INBOX).join(SUBDIR_TMP).exists());
        assert!(maildir.folder_path(FOLDER_DRAFTS).exists());
    }

    #[test]
    fn test_store_and_get() {
        let (_temp, maildir) = test_maildir();

        let content = b"From: test@example.com\r\nSubject: Test\r\n\r\nHello, world!";
        let flags = MessageFlags::default();

        let msg = maildir.store(FOLDER_INBOX, content, &flags).unwrap();
        assert!(msg.is_new);
        assert!(msg.path.exists());

        let retrieved = maildir.get(FOLDER_INBOX, &msg.id).unwrap().unwrap();
        assert_eq!(retrieved.id, msg.id);

        let retrieved_content = retrieved.read_content().unwrap();
        assert!(retrieved_content.contains("Hello, world!"));
    }

    #[test]
    fn test_store_seen_message() {
        let (_temp, maildir) = test_maildir();

        let content = b"Test message";
        let mut flags = MessageFlags::default();
        flags.seen = true;

        let msg = maildir.store(FOLDER_INBOX, content, &flags).unwrap();
        assert!(!msg.is_new);
        assert!(msg.path.to_string_lossy().contains("/cur/"));
    }

    #[test]
    fn test_list_messages() {
        let (_temp, maildir) = test_maildir();

        let flags = MessageFlags::default();
        maildir.store(FOLDER_INBOX, b"Message 1", &flags).unwrap();
        maildir.store(FOLDER_INBOX, b"Message 2", &flags).unwrap();

        let messages = maildir.list(FOLDER_INBOX).unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_delete_message() {
        let (_temp, maildir) = test_maildir();

        let msg = maildir
            .store(FOLDER_INBOX, b"To delete", &MessageFlags::default())
            .unwrap();
        assert!(maildir.get(FOLDER_INBOX, &msg.id).unwrap().is_some());

        let deleted = maildir.delete(FOLDER_INBOX, &msg.id).unwrap();
        assert!(deleted);
        assert!(maildir.get(FOLDER_INBOX, &msg.id).unwrap().is_none());
    }

    #[test]
    fn test_update_flags() {
        let (_temp, maildir) = test_maildir();

        let msg = maildir
            .store(FOLDER_INBOX, b"Test", &MessageFlags::default())
            .unwrap();
        assert!(msg.is_new);

        let mut new_flags = MessageFlags::default();
        new_flags.seen = true;
        new_flags.flagged = true;

        let updated = maildir
            .update_flags(FOLDER_INBOX, &msg.id, &new_flags)
            .unwrap()
            .unwrap();
        assert!(!updated.is_new);
        assert!(updated.flags.seen);
        assert!(updated.flags.flagged);
    }

    #[test]
    fn test_move_message() {
        let (_temp, maildir) = test_maildir();

        let msg = maildir
            .store(FOLDER_INBOX, b"To move", &MessageFlags::default())
            .unwrap();

        let moved = maildir
            .move_to(FOLDER_INBOX, &msg.id, FOLDER_TRASH)
            .unwrap()
            .unwrap();
        assert_eq!(moved.folder, FOLDER_TRASH);

        assert!(maildir.get(FOLDER_INBOX, &msg.id).unwrap().is_none());
        assert!(maildir.get(FOLDER_TRASH, &msg.id).unwrap().is_some());
    }

    #[test]
    fn test_count_messages() {
        let (_temp, maildir) = test_maildir();

        let flags = MessageFlags::default();
        maildir.store(FOLDER_INBOX, b"New 1", &flags).unwrap();
        maildir.store(FOLDER_INBOX, b"New 2", &flags).unwrap();

        let mut seen_flags = MessageFlags::default();
        seen_flags.seen = true;
        maildir.store(FOLDER_INBOX, b"Read 1", &seen_flags).unwrap();

        let (new_count, cur_count) = maildir.count(FOLDER_INBOX).unwrap();
        assert_eq!(new_count, 2);
        assert_eq!(cur_count, 1);
    }

    #[test]
    fn test_list_folders() {
        let (_temp, maildir) = test_maildir();
        maildir.init().unwrap();

        let folders = maildir.list_folders().unwrap();
        assert!(folders.contains(&FOLDER_INBOX.to_string()));
        assert!(folders.contains(&FOLDER_SENT.to_string()));
        assert!(folders.contains(&FOLDER_DRAFTS.to_string()));
        assert!(folders.contains(&FOLDER_TRASH.to_string()));
    }

    #[test]
    fn test_store_with_id() {
        let (_temp, maildir) = test_maildir();

        let custom_id = "custom-message-id";
        let msg = maildir
            .store_with_id(
                FOLDER_INBOX,
                b"Custom ID message",
                &MessageFlags::default(),
                custom_id,
            )
            .unwrap();

        assert_eq!(msg.id, custom_id);

        let retrieved = maildir.get(FOLDER_INBOX, custom_id).unwrap().unwrap();
        assert_eq!(retrieved.id, custom_id);
    }
}
