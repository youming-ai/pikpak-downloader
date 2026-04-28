//! Domain types returned by PikPak API calls.
//!
//! Field names follow the JSON wire format used by the upstream
//! `pikpakcli` project and the `api-drive.mypikpak.com` service.
//! PikPak encodes 64-bit sizes as JSON strings (e.g. `"1234"`) to work
//! around JSON's 53-bit precision limit; custom deserializers convert
//! these to native `u64` eagerly so callers can ignore the quirk.

use serde::{Deserialize, Deserializer, Serialize};

/// Whether an entry is a regular file or a folder.
///
/// PikPak serializes this as the literal strings `"drive#file"` or
/// `"drive#folder"`; we round-trip those values via `serde(rename)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    /// A regular downloadable file.
    #[serde(rename = "drive#file")]
    File,
    /// A folder / directory.
    #[serde(rename = "drive#folder")]
    Folder,
}

impl FileKind {
    /// Return `true` if this entry is a folder.
    pub fn is_folder(self) -> bool {
        matches!(self, FileKind::Folder)
    }
}

/// A single entry returned by a file listing call.
///
/// Only the fields this crate currently needs are mapped; anything else
/// PikPak returns is silently ignored, which means we won't break when
/// the server adds new fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// PikPak-assigned identifier (opaque string).
    pub id: String,

    /// Human-readable name (may contain spaces, UTF-8).
    pub name: String,

    /// File size in bytes. Returned by PikPak as a numeric string.
    #[serde(default, deserialize_with = "deserialize_string_to_u64")]
    pub size: u64,

    /// Whether this is a file or folder.
    pub kind: FileKind,

    /// Parent folder id. Only missing for the virtual root.
    #[serde(default)]
    pub parent_id: Option<String>,

    /// MIME type if known (server-assigned; usually absent for folders).
    #[serde(default)]
    pub mime_type: Option<String>,

    /// RFC3339 creation timestamp, if reported.
    #[serde(default)]
    pub created_time: Option<String>,

    /// RFC3339 last-modified timestamp, if reported.
    #[serde(default)]
    pub modified_time: Option<String>,

    /// Optional file extension (without the leading dot).
    #[serde(default)]
    pub file_extension: Option<String>,
}

/// Storage quota snapshot.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Quota {
    /// Total bytes allocated to the account.
    pub total: u64,
    /// Bytes currently in use.
    pub used: u64,
}

impl Quota {
    /// Remaining free bytes (never negative; saturating).
    pub fn free(self) -> u64 {
        self.total.saturating_sub(self.used)
    }

    /// Used fraction in the 0.0..=1.0 range, or `None` if `total == 0`.
    pub fn ratio(self) -> Option<f64> {
        if self.total == 0 {
            None
        } else {
            Some(self.used as f64 / self.total as f64)
        }
    }
}

/// Deserialize a JSON string holding a decimal integer into `u64`.
///
/// Accepts a missing/null field or empty string as zero so that folders
/// (which PikPak sometimes returns without a `size` key) don't make the
/// whole listing fail.
pub(crate) fn deserialize_string_to_u64<'de, D>(d: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(d)?;
    match s.as_deref() {
        None | Some("") => Ok(0),
        Some(v) => v.parse::<u64>().map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_kind_roundtrips_drive_strings() {
        let file: FileKind = serde_json::from_str(r#""drive#file""#).unwrap();
        let folder: FileKind = serde_json::from_str(r#""drive#folder""#).unwrap();
        assert_eq!(file, FileKind::File);
        assert_eq!(folder, FileKind::Folder);
        assert_eq!(
            serde_json::to_string(&FileKind::File).unwrap(),
            r#""drive#file""#
        );
        assert_eq!(
            serde_json::to_string(&FileKind::Folder).unwrap(),
            r#""drive#folder""#
        );
    }

    #[test]
    fn file_kind_is_folder() {
        assert!(FileKind::Folder.is_folder());
        assert!(!FileKind::File.is_folder());
    }

    #[test]
    fn quota_free_and_ratio() {
        let q = Quota {
            total: 100,
            used: 25,
        };
        assert_eq!(q.free(), 75);
        assert_eq!(q.ratio(), Some(0.25));

        let empty = Quota { total: 0, used: 0 };
        assert_eq!(empty.free(), 0);
        assert_eq!(empty.ratio(), None);

        // Over-report (edge case): used > total, free saturates at 0.
        let over = Quota {
            total: 10,
            used: 20,
        };
        assert_eq!(over.free(), 0);
    }

    #[test]
    fn size_deserializes_from_string() {
        let json = r#"{"id":"a","name":"x","size":"1234","kind":"drive#file"}"#;
        let f: FileInfo = serde_json::from_str(json).unwrap();
        assert_eq!(f.size, 1234);
        assert_eq!(f.kind, FileKind::File);
    }

    #[test]
    fn size_deserializes_from_missing() {
        let json = r#"{"id":"a","name":"x","kind":"drive#folder"}"#;
        let f: FileInfo = serde_json::from_str(json).unwrap();
        assert_eq!(f.size, 0);
        assert_eq!(f.kind, FileKind::Folder);
    }

    #[test]
    fn size_deserializes_from_null() {
        let json = r#"{"id":"a","name":"x","size":null,"kind":"drive#folder"}"#;
        let f: FileInfo = serde_json::from_str(json).unwrap();
        assert_eq!(f.size, 0);
    }
}
