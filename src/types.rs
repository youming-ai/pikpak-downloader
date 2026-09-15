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
/// `"drive#folder"`; we round-trip those values via `serde(rename)`. Any other
/// value maps to [`FileKind::Unknown`] instead of failing the parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    /// A regular downloadable file.
    #[serde(rename = "drive#file")]
    File,
    /// A folder / directory.
    #[serde(rename = "drive#folder")]
    Folder,
    /// A kind this crate does not model (PikPak adds them over time).
    ///
    /// Deserialization-only in practice: without this catch-all a single
    /// unknown value would fail the whole listing and break every operation.
    /// Callers should treat it as neither a file nor a folder.
    #[serde(other)]
    Unknown,
}

impl FileKind {
    /// Return `true` if this entry is a folder.
    pub fn is_folder(self) -> bool {
        matches!(self, FileKind::Folder)
    }

    /// Return `true` if this entry is a regular file.
    pub fn is_file(self) -> bool {
        matches!(self, FileKind::File)
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

    /// File size in bytes. PikPak normally sends a numeric string; both that
    /// and a plain JSON number are accepted.
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
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

/// Deserialize a size that PikPak may encode either as a decimal string
/// (`"1234"`) or as a plain JSON number, into `u64`.
///
/// Accepts a missing/null field or an empty string as zero, so folders (which
/// PikPak sometimes returns without a `size` key) don't make the whole listing
/// fail. The wire format is documented as strings, but a server-side switch to
/// numbers would otherwise break every operation — accepting both costs nothing.
pub(crate) fn deserialize_lenient_u64<'de, D>(d: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(u64),
    }

    match Option::<StringOrNumber>::deserialize(d)? {
        None => Ok(0),
        Some(StringOrNumber::Number(n)) => Ok(n),
        Some(StringOrNumber::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                Ok(0)
            } else {
                s.parse::<u64>().map_err(serde::de::Error::custom)
            }
        }
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

    #[test]
    fn size_deserializes_from_number() {
        // PikPak normally sends sizes as strings; a switch to numbers must not
        // break every listing.
        let json = r#"{"id":"a","name":"x","size":1234,"kind":"drive#file"}"#;
        let f: FileInfo = serde_json::from_str(json).unwrap();
        assert_eq!(f.size, 1234);
    }

    #[test]
    fn size_deserializes_from_padded_string() {
        let json = r#"{"id":"a","name":"x","size":" 42 ","kind":"drive#file"}"#;
        let f: FileInfo = serde_json::from_str(json).unwrap();
        assert_eq!(f.size, 42);
    }

    #[test]
    fn file_kind_accepts_unknown_variants() {
        // A kind this crate does not model must not fail the whole listing.
        let kind: FileKind = serde_json::from_str(r#""drive#shortcut""#).unwrap();
        assert_eq!(kind, FileKind::Unknown);
        assert!(!kind.is_file() && !kind.is_folder());

        let json = r#"{"id":"a","name":"x","size":"1","kind":"drive#shortcut"}"#;
        let f: FileInfo = serde_json::from_str(json).unwrap();
        assert_eq!(f.kind, FileKind::Unknown);
    }

    #[test]
    fn unknown_file_kind_never_serializes_as_a_real_kind() {
        // The catch-all exists for deserialization. Serializing one must not
        // emit a tag the server would treat as a genuine file/folder, so a
        // consumer round-tripping a listing cannot silently produce a
        // valid-looking kind.
        let json = serde_json::to_string(&FileKind::Unknown).unwrap();
        assert_eq!(json, r#""Unknown""#);
        assert_ne!(json, r#""drive#file""#);
        assert_ne!(json, r#""drive#folder""#);
    }
}
