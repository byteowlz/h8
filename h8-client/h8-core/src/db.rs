//! SQLite database for sync state and ID management.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::error::{Error, Result};
use crate::types::{CalendarEventSync, FolderSync, MessageSync};

/// Database handle for h8 sync state.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create a database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Initialize the database schema.
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                local_id TEXT PRIMARY KEY,
                remote_id TEXT UNIQUE NOT NULL,
                change_key TEXT,
                folder TEXT NOT NULL,
                subject TEXT,
                from_addr TEXT,
                received_at TEXT,
                is_read INTEGER DEFAULT 0,
                is_draft INTEGER DEFAULT 0,
                has_attachments INTEGER DEFAULT 0,
                synced_at TEXT,
                local_hash TEXT
            );

            CREATE TABLE IF NOT EXISTS sync_state (
                folder TEXT PRIMARY KEY,
                last_sync TEXT,
                sync_token TEXT
            );

            CREATE TABLE IF NOT EXISTS id_pool (
                short_id TEXT PRIMARY KEY,
                status TEXT DEFAULT 'free',
                assigned_at TEXT,
                message_remote_id TEXT
            );

            CREATE TABLE IF NOT EXISTS calendar_events (
                local_id TEXT PRIMARY KEY,
                remote_id TEXT UNIQUE NOT NULL,
                change_key TEXT,
                subject TEXT,
                location TEXT,
                start TEXT,
                end TEXT,
                is_all_day INTEGER DEFAULT 0,
                synced_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_messages_remote_id ON messages(remote_id);
            CREATE INDEX IF NOT EXISTS idx_messages_folder ON messages(folder);
            CREATE INDEX IF NOT EXISTS idx_id_pool_status ON id_pool(status);
            CREATE INDEX IF NOT EXISTS idx_calendar_remote_id ON calendar_events(remote_id);
            CREATE INDEX IF NOT EXISTS idx_calendar_start ON calendar_events(start);
            "#,
        )?;

        // Migration: add has_attachments column if it doesn't exist
        let _ = self.conn.execute(
            "ALTER TABLE messages ADD COLUMN has_attachments INTEGER DEFAULT 0",
            [],
        );

        Ok(())
    }

    /// Insert or update a message sync record.
    pub fn upsert_message(&self, msg: &MessageSync) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO messages (local_id, remote_id, change_key, folder, subject, from_addr, received_at, is_read, is_draft, has_attachments, synced_at, local_hash)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(local_id) DO UPDATE SET
                remote_id = excluded.remote_id,
                change_key = excluded.change_key,
                folder = excluded.folder,
                subject = excluded.subject,
                from_addr = excluded.from_addr,
                received_at = excluded.received_at,
                is_read = excluded.is_read,
                is_draft = excluded.is_draft,
                has_attachments = excluded.has_attachments,
                synced_at = excluded.synced_at,
                local_hash = excluded.local_hash
            "#,
            params![
                msg.local_id,
                msg.remote_id,
                msg.change_key,
                msg.folder,
                msg.subject,
                msg.from_addr,
                msg.received_at,
                msg.is_read,
                msg.is_draft,
                msg.has_attachments,
                msg.synced_at,
                msg.local_hash,
            ],
        )?;
        Ok(())
    }

    /// Get a message by local ID.
    pub fn get_message(&self, local_id: &str) -> Result<Option<MessageSync>> {
        let mut stmt = self.conn.prepare(
            "SELECT local_id, remote_id, change_key, folder, subject, from_addr, received_at, is_read, is_draft, has_attachments, synced_at, local_hash FROM messages WHERE local_id = ?1",
        )?;
        let mut rows = stmt.query(params![local_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(MessageSync {
                local_id: row.get(0)?,
                remote_id: row.get(1)?,
                change_key: row.get(2)?,
                folder: row.get(3)?,
                subject: row.get(4)?,
                from_addr: row.get(5)?,
                received_at: row.get(6)?,
                is_read: row.get(7)?,
                is_draft: row.get(8)?,
                has_attachments: row.get(9)?,
                synced_at: row.get(10)?,
                local_hash: row.get(11)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get a message by remote ID.
    pub fn get_message_by_remote_id(&self, remote_id: &str) -> Result<Option<MessageSync>> {
        let mut stmt = self.conn.prepare(
            "SELECT local_id, remote_id, change_key, folder, subject, from_addr, received_at, is_read, is_draft, has_attachments, synced_at, local_hash FROM messages WHERE remote_id = ?1",
        )?;
        let mut rows = stmt.query(params![remote_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(MessageSync {
                local_id: row.get(0)?,
                remote_id: row.get(1)?,
                change_key: row.get(2)?,
                folder: row.get(3)?,
                subject: row.get(4)?,
                from_addr: row.get(5)?,
                received_at: row.get(6)?,
                is_read: row.get(7)?,
                is_draft: row.get(8)?,
                has_attachments: row.get(9)?,
                synced_at: row.get(10)?,
                local_hash: row.get(11)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// List messages in a folder.
    pub fn list_messages(&self, folder: &str, limit: usize) -> Result<Vec<MessageSync>> {
        let mut stmt = self.conn.prepare(
            "SELECT local_id, remote_id, change_key, folder, subject, from_addr, received_at, is_read, is_draft, has_attachments, synced_at, local_hash FROM messages WHERE folder = ?1 ORDER BY received_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![folder, limit], |row| {
            Ok(MessageSync {
                local_id: row.get(0)?,
                remote_id: row.get(1)?,
                change_key: row.get(2)?,
                folder: row.get(3)?,
                subject: row.get(4)?,
                from_addr: row.get(5)?,
                received_at: row.get(6)?,
                is_read: row.get(7)?,
                is_draft: row.get(8)?,
                has_attachments: row.get(9)?,
                synced_at: row.get(10)?,
                local_hash: row.get(11)?,
            })
        })?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }

    /// Delete a message by local ID.
    pub fn delete_message(&self, local_id: &str) -> Result<bool> {
        let count = self.conn.execute(
            "DELETE FROM messages WHERE local_id = ?1",
            params![local_id],
        )?;
        Ok(count > 0)
    }

    /// Update folder sync state.
    pub fn upsert_sync_state(&self, state: &FolderSync) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO sync_state (folder, last_sync, sync_token)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(folder) DO UPDATE SET
                last_sync = excluded.last_sync,
                sync_token = excluded.sync_token
            "#,
            params![state.folder, state.last_sync, state.sync_token],
        )?;
        Ok(())
    }

    /// Get folder sync state.
    pub fn get_sync_state(&self, folder: &str) -> Result<Option<FolderSync>> {
        let mut stmt = self
            .conn
            .prepare("SELECT folder, last_sync, sync_token FROM sync_state WHERE folder = ?1")?;
        let mut rows = stmt.query(params![folder])?;
        if let Some(row) = rows.next()? {
            Ok(Some(FolderSync {
                folder: row.get(0)?,
                last_sync: row.get(1)?,
                sync_token: row.get(2)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Seed the ID pool with adjective-noun combinations.
    pub fn seed_id_pool(&self, adjectives: &[&str], nouns: &[&str]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        for adj in adjectives {
            for noun in nouns {
                // Skip same-word pairs
                if adj == noun {
                    continue;
                }
                let short_id = format!("{}-{}", adj, noun);
                tx.execute(
                    "INSERT OR IGNORE INTO id_pool (short_id, status) VALUES (?1, 'free')",
                    params![short_id],
                )?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    /// Allocate a free ID from the pool.
    pub fn allocate_id(&self, remote_id: &str) -> Result<String> {
        let now = chrono::Utc::now().to_rfc3339();

        // Try to find a free ID
        let mut stmt = self.conn.prepare(
            "SELECT short_id FROM id_pool WHERE status = 'free' ORDER BY RANDOM() LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;

        if let Some(row) = rows.next()? {
            let short_id: String = row.get(0)?;
            drop(rows);
            drop(stmt);

            self.conn.execute(
                "UPDATE id_pool SET status = 'used', assigned_at = ?1, message_remote_id = ?2 WHERE short_id = ?3",
                params![now, remote_id, short_id],
            )?;
            Ok(short_id)
        } else {
            Err(Error::IdPoolExhausted)
        }
    }

    /// Free an ID back to the pool.
    pub fn free_id(&self, short_id: &str) -> Result<bool> {
        let count = self.conn.execute(
            "UPDATE id_pool SET status = 'free', assigned_at = NULL, message_remote_id = NULL WHERE short_id = ?1",
            params![short_id],
        )?;
        Ok(count > 0)
    }

    /// Get ID by remote message ID.
    pub fn get_id_by_remote(&self, remote_id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT short_id FROM id_pool WHERE message_remote_id = ?1")?;
        let mut rows = stmt.query(params![remote_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Get remote ID by short ID.
    pub fn get_remote_by_id(&self, short_id: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT message_remote_id FROM id_pool WHERE short_id = ?1 AND status = 'used'",
        )?;
        let mut rows = stmt.query(params![short_id])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }

    /// Count free IDs in the pool.
    pub fn count_free_ids(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM id_pool WHERE status = 'free'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Count used IDs in the pool.
    pub fn count_used_ids(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM id_pool WHERE status = 'used'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    // Calendar event methods

    /// Insert or update a calendar event sync record.
    pub fn upsert_calendar_event(&self, event: &CalendarEventSync) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO calendar_events (local_id, remote_id, change_key, subject, location, start, end, is_all_day, synced_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(local_id) DO UPDATE SET
                remote_id = excluded.remote_id,
                change_key = excluded.change_key,
                subject = excluded.subject,
                location = excluded.location,
                start = excluded.start,
                end = excluded.end,
                is_all_day = excluded.is_all_day,
                synced_at = excluded.synced_at
            "#,
            params![
                event.local_id,
                event.remote_id,
                event.change_key,
                event.subject,
                event.location,
                event.start,
                event.end,
                event.is_all_day,
                event.synced_at
            ],
        )?;
        Ok(())
    }

    /// Get a calendar event by local ID.
    pub fn get_calendar_event(&self, local_id: &str) -> Result<Option<CalendarEventSync>> {
        let mut stmt = self.conn.prepare(
            "SELECT local_id, remote_id, change_key, subject, location, start, end, is_all_day, synced_at FROM calendar_events WHERE local_id = ?1",
        )?;
        let mut rows = stmt.query(params![local_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(CalendarEventSync {
                local_id: row.get(0)?,
                remote_id: row.get(1)?,
                change_key: row.get(2)?,
                subject: row.get(3)?,
                location: row.get(4)?,
                start: row.get(5)?,
                end: row.get(6)?,
                is_all_day: row.get(7)?,
                synced_at: row.get(8)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get a calendar event by remote ID.
    pub fn get_calendar_event_by_remote_id(&self, remote_id: &str) -> Result<Option<CalendarEventSync>> {
        let mut stmt = self.conn.prepare(
            "SELECT local_id, remote_id, change_key, subject, location, start, end, is_all_day, synced_at FROM calendar_events WHERE remote_id = ?1",
        )?;
        let mut rows = stmt.query(params![remote_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(CalendarEventSync {
                local_id: row.get(0)?,
                remote_id: row.get(1)?,
                change_key: row.get(2)?,
                subject: row.get(3)?,
                location: row.get(4)?,
                start: row.get(5)?,
                end: row.get(6)?,
                is_all_day: row.get(7)?,
                synced_at: row.get(8)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// List calendar events in a date range.
    pub fn list_calendar_events(&self, start_after: Option<&str>, limit: usize) -> Result<Vec<CalendarEventSync>> {
        let (query, use_param) = if start_after.is_some() {
            (format!(
                "SELECT local_id, remote_id, change_key, subject, location, start, end, is_all_day, synced_at \
                 FROM calendar_events WHERE start >= ?1 ORDER BY start ASC LIMIT {}",
                limit
            ), true)
        } else {
            (format!(
                "SELECT local_id, remote_id, change_key, subject, location, start, end, is_all_day, synced_at \
                 FROM calendar_events ORDER BY start ASC LIMIT {}",
                limit
            ), false)
        };

        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = if use_param {
            stmt.query(params![start_after.unwrap()])?
        } else {
            stmt.query([])?
        };

        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            events.push(CalendarEventSync {
                local_id: row.get(0)?,
                remote_id: row.get(1)?,
                change_key: row.get(2)?,
                subject: row.get(3)?,
                location: row.get(4)?,
                start: row.get(5)?,
                end: row.get(6)?,
                is_all_day: row.get(7)?,
                synced_at: row.get(8)?,
            });
        }
        Ok(events)
    }

    /// Delete a calendar event by local ID.
    pub fn delete_calendar_event(&self, local_id: &str) -> Result<bool> {
        let count = self.conn.execute(
            "DELETE FROM calendar_events WHERE local_id = ?1",
            params![local_id],
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_creation() {
        let db = Database::open_memory().unwrap();
        assert_eq!(db.count_free_ids().unwrap(), 0);
    }

    #[test]
    fn test_message_crud() {
        let db = Database::open_memory().unwrap();

        let msg = MessageSync {
            local_id: "test-local".to_string(),
            remote_id: "remote-123".to_string(),
            change_key: Some("key-1".to_string()),
            folder: "inbox".to_string(),
            subject: Some("Test Subject".to_string()),
            from_addr: Some("sender@example.com".to_string()),
            received_at: Some("2024-01-01T00:00:00Z".to_string()),
            is_read: false,
            is_draft: false,
            has_attachments: true,
            synced_at: None,
            local_hash: None,
        };

        db.upsert_message(&msg).unwrap();

        let retrieved = db.get_message("test-local").unwrap().unwrap();
        assert_eq!(retrieved.remote_id, "remote-123");
        assert_eq!(retrieved.subject, Some("Test Subject".to_string()));
        assert!(retrieved.has_attachments);

        let by_remote = db.get_message_by_remote_id("remote-123").unwrap().unwrap();
        assert_eq!(by_remote.local_id, "test-local");
        assert!(by_remote.has_attachments);

        let messages = db.list_messages("inbox", 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].has_attachments);

        assert!(db.delete_message("test-local").unwrap());
        assert!(db.get_message("test-local").unwrap().is_none());
    }

    #[test]
    fn test_sync_state() {
        let db = Database::open_memory().unwrap();

        let state = FolderSync {
            folder: "inbox".to_string(),
            last_sync: Some("2024-01-01T00:00:00Z".to_string()),
            sync_token: Some("token-123".to_string()),
        };

        db.upsert_sync_state(&state).unwrap();

        let retrieved = db.get_sync_state("inbox").unwrap().unwrap();
        assert_eq!(retrieved.sync_token, Some("token-123".to_string()));
    }

    #[test]
    fn test_id_pool() {
        let db = Database::open_memory().unwrap();

        let adjectives = ["cold", "blue", "fast"];
        let nouns = ["lamp", "frog", "cold"]; // "cold" is in both to test overlap

        let count = db.seed_id_pool(&adjectives, &nouns).unwrap();
        // 3 adj * 3 nouns - 1 same-word pair (cold-cold) = 8
        assert_eq!(count, 8);

        // Allocate an ID
        let id = db.allocate_id("remote-msg-1").unwrap();
        assert!(id.contains('-'));

        // Verify it's marked as used
        let remote = db.get_remote_by_id(&id).unwrap();
        assert_eq!(remote, Some("remote-msg-1".to_string()));

        // Free the ID
        assert!(db.free_id(&id).unwrap());
        assert!(db.get_remote_by_id(&id).unwrap().is_none());
    }

    #[test]
    fn test_id_pool_exhaustion() {
        let db = Database::open_memory().unwrap();

        let adjectives = ["cold"];
        let nouns = ["lamp"];

        db.seed_id_pool(&adjectives, &nouns).unwrap();

        // Allocate the only ID
        let id = db.allocate_id("remote-1").unwrap();
        assert_eq!(id, "cold-lamp");

        // Try to allocate another - should fail
        let result = db.allocate_id("remote-2");
        assert!(matches!(result, Err(Error::IdPoolExhausted)));
    }
}
