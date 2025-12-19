//! h8-core: Core library for h8 - shared types, config, and utilities.
//!
//! This crate provides the foundation for the h8 email client, including:
//! - Configuration management
//! - Service client for communicating with the Python backend
//! - SQLite database for sync state and ID management
//! - Human-readable ID generation (adjective-noun format)
//! - Local Maildir storage
//! - Email compose format parsing

pub mod compose;
pub mod config;
pub mod db;
pub mod error;
pub mod id;
pub mod maildir;
pub mod paths;
pub mod service;
pub mod types;

pub use compose::{ComposeBuilder, ComposeDocument};
pub use config::{AppConfig, CalendarConfig, CalendarView};
pub use db::Database;
pub use error::{Error, Result};
pub use id::IdGenerator;
pub use maildir::Maildir;
pub use paths::AppPaths;
pub use service::ServiceClient;

/// Convert HTML content to readable plain text.
///
/// This handles common HTML email formatting and produces clean, readable output.
pub fn html_to_text(html: &str, width: usize) -> String {
    html2text::from_read(html.as_bytes(), width)
}
