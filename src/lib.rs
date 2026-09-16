//! Rust crate for PikPak cloud storage.
//!
//! The README is the crate documentation, so its examples are compiled by
//! `cargo test --doc` and cannot drift from the API.
#![doc = include_str!("../README.md")]
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
