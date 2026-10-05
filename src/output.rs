//! The CLI's output contract, and the exit codes that go with it.
//!
//! Two rules hold for every command:
//!
//! * **stdout carries data; stderr carries narration.** Progress, retries,
//!   warnings and `Saved: …` lines are for a human watching, and never pollute
//!   the stream a caller parses.
//! * **With `--json`, stdout carries exactly one JSON document per run** — the
//!   result on success, the failure on error. Narration still goes to stderr.
//!
//! The document shapes and the exit codes are documented in `AGENTS.md`; treat
//! them as a public interface. Code 2 is clap's own usage error, so it never
//! appears here — nothing we return can pre-empt argument parsing.

use anyhow::Error as AnyError;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Display;

/// The run succeeded.
pub const EXIT_OK: u8 = 0;
/// An unexpected failure: a bug, or a response we could not make sense of.
pub const EXIT_UNEXPECTED: u8 = 1;
/// The token is missing, rejected, or no longer refreshable.
pub const EXIT_AUTH: u8 = 3;
/// The path that was asked for does not exist.
pub const EXIT_NOT_FOUND: u8 = 4;
/// The network or the service stayed unreachable or overloaded after retries.
pub const EXIT_NETWORK: u8 = 5;
/// The service refused the request for a reason a retry will not fix.
pub const EXIT_REFUSED: u8 = 6;
/// A local filesystem operation failed.
pub const EXIT_IO: u8 = 7;

/// How much the run says, and in what shape.
#[derive(Debug, Clone, Copy)]
pub struct Output {
    json: bool,
    no_progress: bool,
}

impl Output {
    /// Build the output settings from the global flags.
    pub fn new(json: bool, no_progress: bool) -> Self {
        Self { json, no_progress }
    }

    /// Whether stdout should carry a JSON document.
    pub fn json(self) -> bool {
        self.json
    }

    /// Whether byte-level progress should be drawn for `jobs` transfers.
    ///
    /// Progress is a single-line redraw, so it only reads cleanly with one
    /// transfer in flight.
    pub fn progress(self, jobs: usize) -> bool {
        !self.no_progress && jobs == 1
    }

    /// Narration. Goes to stderr in both modes.
    pub fn note(self, message: impl Display) {
        eprintln!("{message}");
    }

    /// The result document. Printed only in JSON mode — the human-readable data
    /// has already been written by the command itself.
    pub fn ok<T: Serialize>(self, command: &str, result: &T) -> anyhow::Result<()> {
        if self.json {
            self.print(result_document(command, result));
        }
        Ok(())
    }

    /// The failure document, so a caller that parses stdout gets JSON even when
    /// the run fails. The human message still reaches stderr.
    pub fn fail(self, command: &str, error: &AnyError) {
        if self.json {
            self.print(error_document(command, error));
        }
    }

    fn print(self, document: Value) {
        // A `Value` of our own making always serializes; printing nothing beats
        // panicking on a document nobody asked to see.
        if let Ok(text) = serde_json::to_string(&document) {
            println!("{text}");
        }
    }
}

/// The success envelope: `{"ok":true,"command":…,"result":…}`.
pub(crate) fn result_document<T: Serialize>(command: &str, result: &T) -> Value {
    serde_json::json!({
        "ok": true,
        "command": command,
        "result": result,
    })
}

/// The failure envelope, carrying the same `exit_code` the process will use.
pub(crate) fn error_document(command: &str, error: &AnyError) -> Value {
    let (exit_code, kind) = exit_code_for(error);
    serde_json::json!({
        "ok": false,
        "command": command,
        "error": {
            "kind": kind,
            "message": format!("{error:#}"),
            "exit_code": exit_code,
        },
    })
}

/// Map a failure to the exit code and the stable `kind` a caller branches on.
///
/// Codes are a public interface: add new ones, never repurpose them.
pub fn exit_code_for(error: &AnyError) -> (u8, &'static str) {
    if let Some(source) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<pikpak::Error>())
    {
        use pikpak::Error;
        return match source {
            Error::Auth(_) | Error::TokenExpired | Error::NotConfigured(_) => (EXIT_AUTH, "auth"),
            Error::NotFound { .. } => (EXIT_NOT_FOUND, "not_found"),
            Error::Http(_) => (EXIT_NETWORK, "network"),
            Error::Api { status, .. } => match *status {
                401 | 403 => (EXIT_AUTH, "auth"),
                404 => (EXIT_NOT_FOUND, "not_found"),
                429 => (EXIT_NETWORK, "network"),
                status if status >= 500 => (EXIT_NETWORK, "network"),
                _ => (EXIT_REFUSED, "refused"),
            },
            _ => (EXIT_UNEXPECTED, "unexpected"),
        };
    }

    // The download path wraps filesystem failures in context of its own, so look
    // for the cause rather than trusting the outermost error.
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
    {
        return (EXIT_IO, "io");
    }

    (EXIT_UNEXPECTED, "unexpected")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pikpak::Error;

    fn code(error: Error) -> (u8, &'static str) {
        exit_code_for(&anyhow::Error::new(error))
    }

    #[test]
    fn exit_codes_say_what_a_caller_has_to_do_about_it() {
        assert_eq!(code(Error::TokenExpired), (EXIT_AUTH, "auth"));
        assert_eq!(
            code(Error::NotConfigured("PIKPAK_REFRESH_TOKEN")),
            (EXIT_AUTH, "auth")
        );
        assert_eq!(code(Error::Auth("rejected".into())), (EXIT_AUTH, "auth"));
        assert_eq!(
            code(Error::NotFound {
                path: "/a/b".into(),
                segment: "b".into()
            }),
            (EXIT_NOT_FOUND, "not_found")
        );
        // A status the service chose still maps by what the caller can do.
        assert_eq!(
            code(Error::Api {
                status: 401,
                message: "nope".into()
            }),
            (EXIT_AUTH, "auth")
        );
        assert_eq!(
            code(Error::Api {
                status: 404,
                message: "gone".into()
            }),
            (EXIT_NOT_FOUND, "not_found")
        );
        assert_eq!(
            code(Error::Api {
                status: 429,
                message: "slow down".into()
            }),
            (EXIT_NETWORK, "network")
        );
        assert_eq!(
            code(Error::Api {
                status: 503,
                message: "down".into()
            }),
            (EXIT_NETWORK, "network")
        );
        assert_eq!(
            code(Error::Api {
                status: 400,
                message: "bad request".into()
            }),
            (EXIT_REFUSED, "refused")
        );
        assert_eq!(
            code(Error::InvalidPath("empty")),
            (EXIT_UNEXPECTED, "unexpected")
        );
    }

    #[test]
    fn a_wrapped_filesystem_failure_still_reports_io() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let wrapped = anyhow::Error::new(io).context("failed to write output file");
        assert_eq!(exit_code_for(&wrapped), (EXIT_IO, "io"));

        assert_eq!(
            exit_code_for(&anyhow::anyhow!("something else")),
            (EXIT_UNEXPECTED, "unexpected")
        );
    }

    #[test]
    fn the_failure_document_carries_the_same_code_as_the_exit() {
        let error = anyhow::Error::new(Error::TokenExpired).context("ls failed");
        let document = error_document("ls", &error);

        assert_eq!(document["ok"], Value::Bool(false));
        assert_eq!(document["command"], "ls");
        assert_eq!(document["error"]["kind"], "auth");
        assert_eq!(document["error"]["exit_code"], EXIT_AUTH);
        // The context chain reaches the message, not just the innermost cause.
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap()
                .contains("ls failed"),
            "{document}"
        );
    }

    #[test]
    fn the_result_document_wraps_the_command_payload() {
        let document = result_document("quota", &serde_json::json!({ "total": 10 }));

        assert_eq!(document["ok"], Value::Bool(true));
        assert_eq!(document["command"], "quota");
        assert_eq!(document["result"]["total"], 10);
    }

    #[test]
    fn progress_is_off_when_asked_for_or_when_transfers_overlap() {
        assert!(Output::new(false, false).progress(1));
        assert!(!Output::new(false, false).progress(4));
        assert!(!Output::new(false, true).progress(1));
        assert!(!Output::new(true, false).progress(4));
    }
}
