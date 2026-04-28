//! Rust client for the PikPak personal cloud storage API.
//!
//! Ported from the upstream Go implementation at <https://github.com/52funny/pikpakcli>.
//! PikPak does not publish an official API specification; the endpoints used
//! here are reverse-engineered and may change without notice.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
mod captcha;
pub mod client;
pub mod error;
pub mod types;

pub use client::{Client, ClientBuilder, DownloadInfo};
pub use error::{Error, Result};
pub use types::{FileInfo, FileKind, Quota};
