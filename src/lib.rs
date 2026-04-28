//! Rust crate for PikPak cloud storage.

#![forbid(unsafe_code)]

pub mod auth;
pub mod captcha;
pub mod client;
pub mod error;
pub mod types;

pub use client::{Client, ClientBuilder, DownloadInfo};
pub use error::{Error, Result};
pub use types::{FileInfo, FileKind, Quota};
