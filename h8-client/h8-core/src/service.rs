//! HTTP client for communicating with the Python EWS service.

use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{DraftSave, DraftUpdate, FetchFormat, FetchMail};

/// Client for the Python EWS service.
#[derive(Debug, Clone)]
pub struct ServiceClient {
    http: Client,
    base_url: String,
}

impl ServiceClient {
    /// Create a new service client.
    pub fn new(base_url: &str, timeout: Option<Duration>) -> Result<Self> {
        let mut builder = Client::builder();
        if let Some(t) = timeout {
            builder = builder.timeout(t);
        }
        let http = builder.build()?;
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(Self { http, base_url })
    }

    /// Check service health.
    pub fn health(&self) -> Result<Value> {
        self.get("/health", &[])
    }

    /// List calendar events.
    pub fn calendar_list(
        &self,
        account: &str,
        days: i64,
        from_date: Option<&str>,
        to_date: Option<&str>,
    ) -> Result<Value> {
        let days_str = days.to_string();
        let from_date_owned = from_date.map(|s| s.to_string());
        let to_date_owned = to_date.map(|s| s.to_string());

        let mut params: Vec<(&str, &str)> = vec![("account", account), ("days", &days_str)];

        if let Some(ref f) = from_date_owned {
            params.push(("from_date", f));
        }
        if let Some(ref t) = to_date_owned {
            params.push(("to_date", t));
        }

        self.get("/calendar", &params)
    }

    /// Create a calendar event.
    pub fn calendar_create(&self, account: &str, payload: Value) -> Result<Value> {
        self.post_json(&format!("/calendar?account={}", account), payload)
    }

    /// Delete a calendar event.
    pub fn calendar_delete(
        &self,
        account: &str,
        id: &str,
        change_key: Option<&str>,
    ) -> Result<Value> {
        let mut url = format!("/calendar/{}?account={}", id, account);
        if let Some(ck) = change_key {
            url.push_str(&format!("&changekey={}", ck));
        }
        self.delete(&url)
    }

    /// List mail messages.
    pub fn mail_list(
        &self,
        account: &str,
        folder: &str,
        limit: usize,
        unread: bool,
    ) -> Result<Value> {
        let limit_str = limit.to_string();
        let unread_str = unread.to_string();
        let params = [
            ("account", account),
            ("folder", folder),
            ("limit", &limit_str),
            ("unread", &unread_str),
        ];
        self.get("/mail", &params)
    }

    /// Get a single mail message.
    pub fn mail_get(&self, account: &str, folder: &str, id: &str) -> Result<Value> {
        let params = [("account", account), ("folder", folder)];
        self.get(&format!("/mail/{}", id), &params)
    }

    /// Send an email.
    pub fn mail_send(&self, account: &str, payload: Value) -> Result<Value> {
        self.post_json(&format!("/mail/send?account={}", account), payload)
    }

    /// Fetch messages to local storage.
    pub fn mail_fetch(
        &self,
        account: &str,
        folder: &str,
        output: &Path,
        format: FetchFormat,
        limit: Option<usize>,
    ) -> Result<Value> {
        let body = FetchMail {
            folder: folder.to_string(),
            output: output.display().to_string(),
            format: format.as_str().to_string(),
            limit,
        };
        let payload = serde_json::to_value(&body)?;
        self.post_json(&format!("/mail/fetch?account={}", account), payload)
    }

    /// Save a draft to Exchange.
    pub fn draft_save(&self, account: &str, draft: &DraftSave) -> Result<Value> {
        let payload = serde_json::to_value(draft)?;
        self.post_json(&format!("/mail/draft?account={}", account), payload)
    }

    /// Update an existing draft.
    pub fn draft_update(&self, account: &str, id: &str, update: &DraftUpdate) -> Result<Value> {
        let payload = serde_json::to_value(update)?;
        self.put_json(&format!("/mail/draft/{}?account={}", id, account), payload)
    }

    /// Delete a draft.
    pub fn draft_delete(&self, account: &str, id: &str) -> Result<Value> {
        self.delete(&format!("/mail/draft/{}?account={}", id, account))
    }

    /// List attachments for a message.
    pub fn mail_attachments_list(&self, account: &str, folder: &str, id: &str) -> Result<Value> {
        let params = [("account", account), ("folder", folder)];
        self.get(&format!("/mail/{}/attachments", id), &params)
    }

    /// Download a specific attachment.
    pub fn mail_attachment_download(
        &self,
        account: &str,
        folder: &str,
        id: &str,
        index: usize,
        output_path: &Path,
    ) -> Result<Value> {
        let payload = serde_json::json!({
            "index": index,
            "output_path": output_path.display().to_string(),
        });
        self.post_json(
            &format!(
                "/mail/{}/attachments/download?account={}&folder={}",
                id, account, folder
            ),
            payload,
        )
    }

    /// List contacts.
    pub fn contacts_list(
        &self,
        account: &str,
        limit: usize,
        search: Option<&str>,
    ) -> Result<Value> {
        let limit_str = limit.to_string();
        let mut params = vec![("account", account), ("limit", &limit_str)];
        let search_owned;
        if let Some(s) = search {
            search_owned = s.to_string();
            params.push(("search", &search_owned));
        }
        self.get("/contacts", &params)
    }

    /// Get a single contact.
    pub fn contacts_get(&self, account: &str, id: &str) -> Result<Value> {
        let params = [("account", account)];
        self.get(&format!("/contacts/{}", id), &params)
    }

    /// Create a contact.
    pub fn contacts_create(&self, account: &str, payload: Value) -> Result<Value> {
        self.post_json(&format!("/contacts?account={}", account), payload)
    }

    /// Delete a contact.
    pub fn contacts_delete(&self, account: &str, id: &str) -> Result<Value> {
        self.delete(&format!("/contacts/{}?account={}", id, account))
    }

    /// Find free calendar slots.
    pub fn free_slots(
        &self,
        account: &str,
        weeks: u8,
        duration: u32,
        limit: Option<usize>,
    ) -> Result<Value> {
        let weeks_str = weeks.to_string();
        let duration_str = duration.to_string();
        let mut params = vec![
            ("account", account),
            ("weeks", &weeks_str),
            ("duration", &duration_str),
        ];
        let limit_str;
        if let Some(l) = limit {
            limit_str = l.to_string();
            params.push(("limit", &limit_str));
        }
        self.get("/free", &params)
    }

    // Internal HTTP methods

    fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.get(&url).query(params).send()?;
        self.handle_response(resp)
    }

    fn post_json(&self, path: &str, payload: Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.post(&url).json(&payload).send()?;
        self.handle_response(resp)
    }

    fn put_json(&self, path: &str, payload: Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.put(&url).json(&payload).send()?;
        self.handle_response(resp)
    }

    fn delete(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.delete(&url).send()?;
        self.handle_response(resp)
    }

    fn handle_response(&self, resp: reqwest::blocking::Response) -> Result<Value> {
        let status = resp.status();
        let text = resp.text()?;

        if !status.is_success() {
            // Try to extract error detail from JSON response
            if let Ok(val) = serde_json::from_str::<Value>(&text)
                && let Some(detail) = val
                    .as_object()
                    .and_then(|m| m.get("detail"))
                    .and_then(|d| d.as_str())
            {
                return Err(Error::Service(format!("service error: {}", detail)));
            }
            let snippet: String = text.chars().take(400).collect();
            return Err(Error::Service(format!(
                "service error ({}): {}",
                status, snippet
            )));
        }

        serde_json::from_str(&text).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_client_creation() {
        let client = ServiceClient::new("http://localhost:8787", None).unwrap();
        assert_eq!(client.base_url, "http://localhost:8787");
    }

    #[test]
    fn test_service_client_url_normalization() {
        let client = ServiceClient::new("http://localhost:8787/", None).unwrap();
        assert_eq!(client.base_url, "http://localhost:8787");
    }

    #[test]
    fn test_fetch_mail_serialization() {
        let fetch = FetchMail {
            folder: "inbox".to_string(),
            output: "/tmp/mail".to_string(),
            format: "maildir".to_string(),
            limit: Some(100),
        };
        let json = serde_json::to_value(&fetch).unwrap();
        assert_eq!(json["folder"], "inbox");
        assert_eq!(json["limit"], 100);
    }
}
