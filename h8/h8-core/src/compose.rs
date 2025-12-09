//! Email compose format parser and serializer.
//!
//! This module handles the YAML frontmatter + body format used for composing emails.
//!
//! Format:
//! ```text
//! ---
//! to: alice@example.com, bob@example.com
//! cc: carol@example.com
//! subject: Re: Meeting tomorrow
//! in-reply-to: <original-message-id>
//! references: <thread-root-id> <original-message-id>
//! ---
//!
//! Hi Alice,
//!
//! Thanks for the update...
//!
//! > Original quoted text here
//! ```

use serde::{Deserialize, Serialize};

use crate::config::ComposeConfig;
use crate::error::{Error, Result};

/// Frontmatter delimiter.
const FRONTMATTER_DELIM: &str = "---";

/// Parsed email compose document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComposeDocument {
    /// Recipients (To field).
    #[serde(default)]
    pub to: Vec<String>,
    /// CC recipients.
    #[serde(default)]
    pub cc: Vec<String>,
    /// BCC recipients.
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Email subject.
    #[serde(default)]
    pub subject: String,
    /// In-Reply-To header for threading.
    #[serde(rename = "in-reply-to", default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    /// References header for threading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<String>,
    /// Message body text.
    #[serde(skip)]
    pub body: String,
}

impl ComposeDocument {
    /// Create a new empty compose document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a compose document from text.
    pub fn parse(text: &str) -> Result<Self> {
        let (frontmatter, body) = split_frontmatter(text)?;
        
        // Parse frontmatter
        let mut doc: ComposeDocument = if frontmatter.is_empty() {
            ComposeDocument::default()
        } else {
            // Handle comma-separated values for to, cc, bcc
            let yaml_value: serde_yaml::Value = serde_yaml::from_str(&frontmatter)
                .map_err(|e| Error::Config(format!("parsing frontmatter YAML: {e}")))?;
            
            let mut doc = ComposeDocument::default();
            
            if let serde_yaml::Value::Mapping(map) = yaml_value {
                for (key, value) in map {
                    let key_str = key.as_str().unwrap_or("");
                    match key_str {
                        "to" => doc.to = parse_address_list(&value),
                        "cc" => doc.cc = parse_address_list(&value),
                        "bcc" => doc.bcc = parse_address_list(&value),
                        "subject" => doc.subject = value.as_str().unwrap_or("").to_string(),
                        "in-reply-to" => doc.in_reply_to = value.as_str().map(String::from),
                        "references" => doc.references = value.as_str().map(String::from),
                        _ => {}
                    }
                }
            }
            
            doc
        };
        
        doc.body = body;
        Ok(doc)
    }

    /// Serialize the document to compose format.
    pub fn to_string(&self) -> Result<String> {
        let mut output = String::new();
        
        // Build frontmatter
        output.push_str(FRONTMATTER_DELIM);
        output.push('\n');
        
        // To
        if !self.to.is_empty() {
            output.push_str("to: ");
            output.push_str(&self.to.join(", "));
            output.push('\n');
        }
        
        // CC
        if !self.cc.is_empty() {
            output.push_str("cc: ");
            output.push_str(&self.cc.join(", "));
            output.push('\n');
        }
        
        // BCC
        if !self.bcc.is_empty() {
            output.push_str("bcc: ");
            output.push_str(&self.bcc.join(", "));
            output.push('\n');
        }
        
        // Subject - quote if it contains special characters
        output.push_str("subject: ");
        output.push_str(&yaml_quote_if_needed(&self.subject));
        output.push('\n');
        
        // In-Reply-To - quote if needed (usually contains angle brackets)
        if let Some(ref irt) = self.in_reply_to {
            output.push_str("in-reply-to: ");
            output.push_str(&yaml_quote_if_needed(irt));
            output.push('\n');
        }
        
        // References - quote if needed
        if let Some(ref refs) = self.references {
            output.push_str("references: ");
            output.push_str(&yaml_quote_if_needed(refs));
            output.push('\n');
        }
        
        output.push_str(FRONTMATTER_DELIM);
        output.push('\n');
        
        // Body
        if !self.body.is_empty() {
            output.push('\n');
            output.push_str(&self.body);
        }
        
        Ok(output)
    }

    /// Create a reply document from an original message.
    pub fn reply(
        original_from: &str,
        original_subject: &str,
        original_message_id: Option<&str>,
        original_references: Option<&str>,
        original_body: &str,
        config: &ComposeConfig,
    ) -> Self {
        let mut doc = Self::new();
        
        // Set recipient to original sender
        doc.to = vec![original_from.to_string()];
        
        // Set subject with Re: prefix if not already present
        doc.subject = if original_subject.to_lowercase().starts_with("re:") {
            original_subject.to_string()
        } else {
            format!("Re: {}", original_subject)
        };
        
        // Set threading headers
        if let Some(msg_id) = original_message_id {
            doc.in_reply_to = Some(msg_id.to_string());
            
            // Build references chain
            let refs = match original_references {
                Some(r) => format!("{} {}", r, msg_id),
                None => msg_id.to_string(),
            };
            doc.references = Some(refs);
        }
        
        // Quote original body
        doc.body = quote_text(original_body, &config.quote_style);
        
        doc
    }

    /// Create a reply-all document from an original message.
    #[allow(clippy::too_many_arguments)]
    pub fn reply_all(
        original_from: &str,
        original_to: &[String],
        original_cc: &[String],
        original_subject: &str,
        original_message_id: Option<&str>,
        original_references: Option<&str>,
        original_body: &str,
        my_email: &str,
        config: &ComposeConfig,
    ) -> Self {
        let mut doc = Self::reply(
            original_from,
            original_subject,
            original_message_id,
            original_references,
            original_body,
            config,
        );
        
        // Add original To recipients (excluding self) to CC
        let mut cc: Vec<String> = original_to
            .iter()
            .filter(|addr| !addr.eq_ignore_ascii_case(my_email))
            .cloned()
            .collect();
        
        // Add original CC recipients (excluding self)
        cc.extend(
            original_cc
                .iter()
                .filter(|addr| !addr.eq_ignore_ascii_case(my_email))
                .cloned(),
        );
        
        // Remove duplicates
        cc.sort();
        cc.dedup();
        
        // Remove the main recipient from CC if present
        cc.retain(|addr| !addr.eq_ignore_ascii_case(original_from));
        
        doc.cc = cc;
        
        doc
    }

    /// Create a forward document from an original message.
    pub fn forward(
        original_from: &str,
        original_to: &[String],
        original_subject: &str,
        original_date: Option<&str>,
        original_body: &str,
        _config: &ComposeConfig,
    ) -> Self {
        let mut doc = Self::new();
        
        // Set subject with Fwd: prefix if not already present
        doc.subject = if original_subject.to_lowercase().starts_with("fwd:") 
            || original_subject.to_lowercase().starts_with("fw:") {
            original_subject.to_string()
        } else {
            format!("Fwd: {}", original_subject)
        };
        
        // Build forwarded message body
        let mut body = String::new();
        body.push_str("\n\n---------- Forwarded message ----------\n");
        body.push_str(&format!("From: {}\n", original_from));
        if let Some(date) = original_date {
            body.push_str(&format!("Date: {}\n", date));
        }
        body.push_str(&format!("Subject: {}\n", original_subject));
        if !original_to.is_empty() {
            body.push_str(&format!("To: {}\n", original_to.join(", ")));
        }
        body.push('\n');
        body.push_str(original_body);
        
        doc.body = body;
        
        doc
    }

    /// Add signature to the body.
    pub fn add_signature(&mut self, signature: &str) {
        if signature.is_empty() {
            return;
        }
        
        // Ensure body ends with newline
        if !self.body.is_empty() && !self.body.ends_with('\n') {
            self.body.push('\n');
        }
        
        // Add signature separator if not present
        if !signature.starts_with("--") {
            self.body.push_str("\n--\n");
        } else {
            self.body.push('\n');
        }
        
        self.body.push_str(signature);
    }

    /// Validate the document for sending.
    pub fn validate(&self) -> Result<()> {
        if self.to.is_empty() {
            return Err(Error::Config("no recipients specified".into()));
        }
        
        // Validate email addresses
        for addr in self.to.iter().chain(self.cc.iter()).chain(self.bcc.iter()) {
            if !is_valid_email(addr) {
                return Err(Error::Config(format!("invalid email address: {}", addr)));
            }
        }
        
        Ok(())
    }

    /// Get all recipients (to + cc + bcc).
    pub fn all_recipients(&self) -> Vec<&String> {
        let mut recipients: Vec<&String> = Vec::new();
        recipients.extend(self.to.iter());
        recipients.extend(self.cc.iter());
        recipients.extend(self.bcc.iter());
        recipients
    }
}

/// Split text into frontmatter and body.
fn split_frontmatter(text: &str) -> Result<(String, String)> {
    let trimmed = text.trim_start();
    
    // Check if text starts with frontmatter delimiter
    if !trimmed.starts_with(FRONTMATTER_DELIM) {
        // No frontmatter, entire text is body
        return Ok((String::new(), text.to_string()));
    }
    
    // Find the closing delimiter
    let after_first = &trimmed[FRONTMATTER_DELIM.len()..];
    let after_first = after_first.trim_start_matches(['\r', '\n']);
    
    if let Some(end_pos) = after_first.find(&format!("\n{}", FRONTMATTER_DELIM)) {
        let frontmatter = &after_first[..end_pos];
        let body_start = end_pos + 1 + FRONTMATTER_DELIM.len();
        let body = after_first.get(body_start..).unwrap_or("").trim_start_matches(['\r', '\n']);
        Ok((frontmatter.to_string(), body.to_string()))
    } else if let Some(end_pos) = after_first.find(FRONTMATTER_DELIM) {
        // Delimiter on same line or at start of content
        let frontmatter = after_first[..end_pos].trim();
        let body_start = end_pos + FRONTMATTER_DELIM.len();
        let body = after_first.get(body_start..).unwrap_or("").trim_start_matches(['\r', '\n']);
        Ok((frontmatter.to_string(), body.to_string()))
    } else {
        // No closing delimiter - treat as error
        Err(Error::Config("unclosed frontmatter (missing closing ---)".into()))
    }
}

/// Parse address list from YAML value.
fn parse_address_list(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(s) => {
            // Split by comma and trim
            s.split(',')
                .map(|addr| addr.trim().to_string())
                .filter(|addr| !addr.is_empty())
                .collect()
        }
        serde_yaml::Value::Sequence(seq) => {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|addr| !addr.is_empty())
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Quote a string for YAML if it contains special characters.
fn yaml_quote_if_needed(s: &str) -> String {
    // Characters that require quoting in YAML
    let needs_quoting = s.contains(':') 
        || s.contains('#') 
        || s.contains('<') 
        || s.contains('>') 
        || s.contains('[') 
        || s.contains(']')
        || s.contains('{')
        || s.contains('}')
        || s.contains('&')
        || s.contains('*')
        || s.contains('!')
        || s.contains('|')
        || s.contains('\'')
        || s.contains('"')
        || s.starts_with(' ')
        || s.ends_with(' ')
        || s.starts_with('@')
        || s.starts_with('`');
    
    if needs_quoting {
        // Use double quotes, escaping internal double quotes
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Quote text with a prefix.
pub fn quote_text(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Basic email validation.
fn is_valid_email(email: &str) -> bool {
    let email = email.trim();
    
    // Handle "Name <email>" format
    let email = if email.contains('<') && email.contains('>') {
        if let Some(start) = email.find('<') {
            if let Some(end) = email.find('>') {
                &email[start + 1..end]
            } else {
                email
            }
        } else {
            email
        }
    } else {
        email
    };
    
    // Basic validation: must have @ and at least one char on each side
    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return false;
    }
    
    let local = parts[0];
    let domain = parts[1];
    
    !local.is_empty() && !domain.is_empty() && domain.contains('.')
}

/// Builder for compose documents.
#[derive(Debug, Default)]
pub struct ComposeBuilder {
    doc: ComposeDocument,
    signature: Option<String>,
}

impl ComposeBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set recipients.
    pub fn to(mut self, recipients: Vec<String>) -> Self {
        self.doc.to = recipients;
        self
    }

    /// Add a recipient.
    pub fn add_to(mut self, recipient: &str) -> Self {
        self.doc.to.push(recipient.to_string());
        self
    }

    /// Set CC recipients.
    pub fn cc(mut self, recipients: Vec<String>) -> Self {
        self.doc.cc = recipients;
        self
    }

    /// Set BCC recipients.
    pub fn bcc(mut self, recipients: Vec<String>) -> Self {
        self.doc.bcc = recipients;
        self
    }

    /// Set subject.
    pub fn subject(mut self, subject: &str) -> Self {
        self.doc.subject = subject.to_string();
        self
    }

    /// Set body.
    pub fn body(mut self, body: &str) -> Self {
        self.doc.body = body.to_string();
        self
    }

    /// Set in-reply-to header.
    pub fn in_reply_to(mut self, message_id: &str) -> Self {
        self.doc.in_reply_to = Some(message_id.to_string());
        self
    }

    /// Set references header.
    pub fn references(mut self, refs: &str) -> Self {
        self.doc.references = Some(refs.to_string());
        self
    }

    /// Set signature to append.
    pub fn signature(mut self, sig: &str) -> Self {
        self.signature = Some(sig.to_string());
        self
    }

    /// Build the document.
    pub fn build(mut self) -> ComposeDocument {
        if let Some(sig) = self.signature {
            self.doc.add_signature(&sig);
        }
        self.doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let text = r#"---
to: alice@example.com
subject: Hello
---

Hi Alice!"#;

        let doc = ComposeDocument::parse(text).unwrap();
        assert_eq!(doc.to, vec!["alice@example.com"]);
        assert_eq!(doc.subject, "Hello");
        assert_eq!(doc.body.trim(), "Hi Alice!");
    }

    #[test]
    fn test_parse_multiple_recipients() {
        let text = r#"---
to: alice@example.com, bob@example.com
cc: carol@example.com
---

Body"#;

        let doc = ComposeDocument::parse(text).unwrap();
        assert_eq!(doc.to, vec!["alice@example.com", "bob@example.com"]);
        assert_eq!(doc.cc, vec!["carol@example.com"]);
    }

    #[test]
    fn test_parse_with_threading() {
        // Note: In YAML, colons and angle brackets need to be quoted
        let text = r#"---
to: alice@example.com
subject: "Re: Meeting"
in-reply-to: "<original@example.com>"
references: "<root@example.com> <original@example.com>"
---

Thanks!"#;

        let doc = ComposeDocument::parse(text).unwrap();
        assert_eq!(doc.subject, "Re: Meeting");
        assert_eq!(doc.in_reply_to, Some("<original@example.com>".to_string()));
        assert_eq!(doc.references, Some("<root@example.com> <original@example.com>".to_string()));
    }

    #[test]
    fn test_serialize() {
        let doc = ComposeBuilder::new()
            .to(vec!["alice@example.com".to_string()])
            .cc(vec!["bob@example.com".to_string()])
            .subject("Test")
            .body("Hello!")
            .build();

        let text = doc.to_string().unwrap();
        assert!(text.contains("to: alice@example.com"));
        assert!(text.contains("cc: bob@example.com"));
        assert!(text.contains("subject: Test"));
        assert!(text.contains("Hello!"));
    }

    #[test]
    fn test_roundtrip() {
        let original = ComposeBuilder::new()
            .to(vec!["alice@example.com".to_string(), "bob@example.com".to_string()])
            .cc(vec!["carol@example.com".to_string()])
            .subject("Test Subject")
            .in_reply_to("<msg@example.com>")
            .body("Test body content")
            .build();

        let text = original.to_string().unwrap();
        let parsed = ComposeDocument::parse(&text).unwrap();

        assert_eq!(parsed.to, original.to);
        assert_eq!(parsed.cc, original.cc);
        assert_eq!(parsed.subject, original.subject);
        assert_eq!(parsed.in_reply_to, original.in_reply_to);
        assert!(parsed.body.contains("Test body content"));
    }

    #[test]
    fn test_reply() {
        let config = ComposeConfig::default();
        let doc = ComposeDocument::reply(
            "sender@example.com",
            "Original Subject",
            Some("<original@example.com>"),
            None,
            "Original body\nwith multiple lines",
            &config,
        );

        assert_eq!(doc.to, vec!["sender@example.com"]);
        assert_eq!(doc.subject, "Re: Original Subject");
        assert_eq!(doc.in_reply_to, Some("<original@example.com>".to_string()));
        assert!(doc.body.contains("> Original body"));
        assert!(doc.body.contains("> with multiple lines"));
    }

    #[test]
    fn test_reply_preserves_re() {
        let config = ComposeConfig::default();
        let doc = ComposeDocument::reply(
            "sender@example.com",
            "Re: Already has Re",
            None,
            None,
            "Body",
            &config,
        );

        assert_eq!(doc.subject, "Re: Already has Re");
    }

    #[test]
    fn test_reply_all() {
        let config = ComposeConfig::default();
        let doc = ComposeDocument::reply_all(
            "sender@example.com",
            &["me@example.com".to_string(), "other@example.com".to_string()],
            &["cc1@example.com".to_string()],
            "Subject",
            None,
            None,
            "Body",
            "me@example.com",
            &config,
        );

        assert_eq!(doc.to, vec!["sender@example.com"]);
        assert!(doc.cc.contains(&"other@example.com".to_string()));
        assert!(doc.cc.contains(&"cc1@example.com".to_string()));
        assert!(!doc.cc.contains(&"me@example.com".to_string()));
    }

    #[test]
    fn test_forward() {
        let config = ComposeConfig::default();
        let doc = ComposeDocument::forward(
            "sender@example.com",
            &["recipient@example.com".to_string()],
            "Original Subject",
            Some("2024-01-01 12:00"),
            "Original body",
            &config,
        );

        assert_eq!(doc.subject, "Fwd: Original Subject");
        assert!(doc.body.contains("---------- Forwarded message ----------"));
        assert!(doc.body.contains("From: sender@example.com"));
        assert!(doc.body.contains("Date: 2024-01-01 12:00"));
        assert!(doc.body.contains("Original body"));
    }

    #[test]
    fn test_add_signature() {
        let mut doc = ComposeDocument::new();
        doc.body = "Hello!".to_string();
        doc.add_signature("John Doe");

        assert!(doc.body.contains("Hello!"));
        assert!(doc.body.contains("--\n"));
        assert!(doc.body.contains("John Doe"));
    }

    #[test]
    fn test_validate() {
        let mut doc = ComposeDocument::new();
        assert!(doc.validate().is_err()); // No recipients

        doc.to = vec!["invalid-email".to_string()];
        assert!(doc.validate().is_err()); // Invalid email

        doc.to = vec!["valid@example.com".to_string()];
        assert!(doc.validate().is_ok());
    }

    #[test]
    fn test_quote_text() {
        let text = "Line 1\nLine 2\nLine 3";
        let quoted = quote_text(text, "> ");
        assert_eq!(quoted, "> Line 1\n> Line 2\n> Line 3");
    }

    #[test]
    fn test_is_valid_email() {
        assert!(is_valid_email("test@example.com"));
        assert!(is_valid_email("John Doe <john@example.com>"));
        assert!(!is_valid_email("invalid"));
        assert!(!is_valid_email("@example.com"));
        assert!(!is_valid_email("test@"));
    }

    #[test]
    fn test_builder() {
        let doc = ComposeBuilder::new()
            .add_to("alice@example.com")
            .add_to("bob@example.com")
            .subject("Test")
            .body("Hello")
            .signature("Best,\nJohn")
            .build();

        assert_eq!(doc.to.len(), 2);
        assert!(doc.body.contains("Hello"));
        assert!(doc.body.contains("John"));
    }

    #[test]
    fn test_all_recipients() {
        let doc = ComposeBuilder::new()
            .to(vec!["a@example.com".to_string()])
            .cc(vec!["b@example.com".to_string()])
            .bcc(vec!["c@example.com".to_string()])
            .build();

        let all = doc.all_recipients();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_no_frontmatter() {
        let text = "Just a plain body without frontmatter";
        let doc = ComposeDocument::parse(text).unwrap();
        assert!(doc.to.is_empty());
        assert!(doc.body.contains("Just a plain body"));
    }

    #[test]
    fn test_empty_frontmatter() {
        let text = "---\n---\n\nBody only";
        let doc = ComposeDocument::parse(text).unwrap();
        assert!(doc.to.is_empty());
        assert!(doc.body.trim().contains("Body only"));
    }
}
