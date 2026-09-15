//! Rust crate for PikPak cloud storage.

#![forbid(unsafe_code)]

pub mod auth;
pub mod captcha;
pub mod client;
pub mod error;
pub mod types;

/// Primitives shared by this crate's hand-rolled HTTP mocks.
#[cfg(test)]
pub(crate) mod test_http;

pub use client::{
    exponential_backoff, is_drive_root, is_retryable_status, jittered_backoff, Client,
    ClientBuilder, DownloadInfo,
};
pub use error::{Error, Result};
pub use types::{FileInfo, FileKind, Quota};
