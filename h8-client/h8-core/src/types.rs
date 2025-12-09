//! Common types used across h8.

use serde::{Deserialize, Serialize};

/// Fetch format for mail export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchFormat {
    Maildir,
    Mbox,
}

impl FetchFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            FetchFormat::Maildir => "maildir",
            FetchFormat::Mbox => "mbox",
        }
    }
}

/// Calendar event creation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarCreate {
    pub subject: String,
    pub start: String,
    pub end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Email send request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendEmail {
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub html: bool,
}

/// Contact creation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactCreate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_title: Option<String>,
}

/// Mail fetch request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchMail {
    pub folder: String,
    pub output: String,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Draft save request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftSave {
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub html: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<String>,
}

/// Draft update request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<bool>,
}

/// Sync state for a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSync {
    pub local_id: String,
    pub remote_id: String,
    pub change_key: Option<String>,
    pub folder: String,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    pub received_at: Option<String>,
    pub is_read: bool,
    pub is_draft: bool,
    pub has_attachments: bool,
    pub synced_at: Option<String>,
    pub local_hash: Option<String>,
}

/// Folder sync state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderSync {
    pub folder: String,
    pub last_sync: Option<String>,
    pub sync_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetch_format_as_str() {
        assert_eq!(FetchFormat::Maildir.as_str(), "maildir");
        assert_eq!(FetchFormat::Mbox.as_str(), "mbox");
    }

    #[test]
    fn test_send_email_serialization() {
        let email = SendEmail {
            to: vec!["test@example.com".to_string()],
            cc: vec![],
            subject: "Test".to_string(),
            body: "Hello".to_string(),
            html: false,
        };
        let json = serde_json::to_string(&email).unwrap();
        assert!(json.contains("test@example.com"));
    }

    #[test]
    fn test_draft_save_serialization() {
        let draft = DraftSave {
            to: vec!["test@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "Draft".to_string(),
            body: "Draft body".to_string(),
            html: false,
            in_reply_to: None,
            references: None,
        };
        let json = serde_json::to_string(&draft).unwrap();
        assert!(json.contains("Draft"));
    }
}
