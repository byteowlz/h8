//! h8-core: Core library for h8 - shared types, config, and utilities.
//!
//! This crate provides the foundation for the h8 email client, including:
//! - Configuration management
//! - Service client for communicating with the Python backend
//! - SQLite database for sync state and ID management
//! - Human-readable ID generation (adjective-noun format)

pub mod config;
pub mod db;
pub mod error;
pub mod id;
pub mod paths;
pub mod service;
pub mod types;

pub use config::AppConfig;
pub use db::Database;
pub use error::{Error, Result};
pub use id::IdGenerator;
pub use paths::AppPaths;
pub use service::ServiceClient;
