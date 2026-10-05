//! End-to-end checks of the `jobs` surface, driven through the real binary.
//!
//! None of these need credentials: listing, inspecting, cancelling and the
//! credential check that guards `--detach` are all local operations on the state
//! directory.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is set")
        .as_secs()
}

/// An empty state directory, and the directory the binary runs in — a `.env`
/// lookup must not pick one up from the repository.
fn state_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pikpak-it-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("jobs")).expect("the state directory is writable");
    dir
}

fn write_record(state: &Path, id: &str, job_state: &str, updated_at: u64) {
    let record = format!(
        r#"{{"id":"{id}","state":"{job_state}","path":"/My Pack","output":"./downloads","jobs":1,"pid":null,"created_at":{updated_at},"updated_at":{updated_at},"downloaded":1,"failed":0,"bytes":10,"error":null}}"#
    );
    std::fs::write(state.join("jobs").join(format!("{id}.json")), record)
        .expect("the record is writable");
}

/// Run the CLI in `state`, with no credentials in reach.
fn run(state: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_pikpak"))
        .args(args)
        .current_dir(state)
        .env("PIKPAK_STATE_DIR", state)
        .env_remove("PIKPAK_REFRESH_TOKEN")
        .env_remove("PIKPAK_PROXY")
        .output()
        .expect("the binary runs");

    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn document(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|error| panic!("stdout was not one JSON document ({error}): {stdout:?}"))
}

#[test]
fn listing_jobs_needs_no_credentials() {
    let state = state_dir("list");
    write_record(&state, "1-000001", "succeeded", now());

    let (code, stdout, stderr) = run(&state, &["--json", "jobs", "list"]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let doc = document(&stdout);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["command"], "jobs.list");
    assert_eq!(doc["result"]["jobs"][0]["id"], "1-000001");
    assert_eq!(doc["result"]["jobs"][0]["state"], "succeeded");
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn debug_logging_never_reaches_stdout() {
    let state = state_dir("verbose");
    // An unreadable record is skipped with a debug log line.
    std::fs::write(state.join("jobs").join("1-000001.json"), b"{ not json").unwrap();

    let (code, stdout, stderr) = run(&state, &["--json", "--verbose", "jobs", "list"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stderr.contains("skipping"),
        "the log line goes to stderr: {stderr}"
    );
    assert_eq!(
        document(&stdout)["result"]["jobs"].as_array().map(Vec::len),
        Some(0)
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn the_state_directory_sits_beside_the_env_file_not_the_working_directory() {
    let base = state_dir("anchored");
    std::fs::write(base.join(".env"), "PIKPAK_PROXY=\n").unwrap();
    let sub = base.join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    // dotenvy finds the parent's `.env` from `sub`; the jobs must live beside it,
    // so a job started here is found again from the parent.
    let output = Command::new(env!("CARGO_BIN_EXE_pikpak"))
        .args(["--json", "jobs", "list"])
        .current_dir(&sub)
        .env_remove("PIKPAK_STATE_DIR")
        .env_remove("PIKPAK_REFRESH_TOKEN")
        .output()
        .expect("the binary runs");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    assert!(base.join(".pikpak").join("jobs").is_dir());
    assert!(!sub.join(".pikpak").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_empty_state_directory_lists_nothing_and_still_succeeds() {
    let state = state_dir("empty");

    let (code, stdout, _) = run(&state, &["--json", "jobs", "list"]);
    assert_eq!(code, 0);
    let doc = document(&stdout);
    assert_eq!(doc["result"]["jobs"].as_array().map(Vec::len), Some(0));
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn a_silent_worker_is_reported_lost_rather_than_running() {
    let state = state_dir("lost");
    write_record(&state, "1-000001", "running", now() - 3600);

    let (code, stdout, _) = run(&state, &["--json", "jobs", "status", "1-000001"]);
    assert_eq!(code, 0);

    let doc = document(&stdout);
    assert_eq!(doc["result"]["job"]["state"], "lost");
    assert!(
        doc["result"]["job"]["error"]
            .as_str()
            .expect("a reason is recorded")
            .contains("stopped reporting"),
        "{doc}"
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn an_unknown_job_is_a_not_found_with_the_matching_exit_code() {
    let state = state_dir("unknown");

    let (code, stdout, stderr) = run(&state, &["--json", "jobs", "status", "nope"]);
    assert_eq!(code, 4, "stderr: {stderr}");

    let doc = document(&stdout);
    assert_eq!(doc["ok"], false);
    // The same name a successful `jobs status` reports.
    assert_eq!(doc["command"], "jobs.status");
    assert_eq!(doc["error"]["kind"], "not_found");
    assert_eq!(doc["error"]["exit_code"], 4);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn cancelling_writes_the_marker_the_worker_watches() {
    let state = state_dir("cancel");
    write_record(&state, "1-000001", "running", now());

    let (code, stdout, _) = run(&state, &["--json", "jobs", "cancel", "1-000001"]);
    assert_eq!(code, 0);

    let doc = document(&stdout);
    assert_eq!(doc["result"]["cancelled"], true);
    assert!(
        state.join("jobs").join("1-000001.cancel").exists(),
        "the worker looks for this file"
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn cancelling_a_finished_job_reports_that_nothing_was_cancelled() {
    let state = state_dir("cancel-done");
    write_record(&state, "1-000001", "succeeded", now());

    let (code, stdout, _) = run(&state, &["--json", "jobs", "cancel", "1-000001"]);
    assert_eq!(code, 0);

    let doc = document(&stdout);
    assert_eq!(doc["result"]["cancelled"], false);
    assert_eq!(doc["result"]["job"]["state"], "succeeded");
    assert!(!state.join("jobs").join("1-000001.cancel").exists());
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn waiting_on_a_finished_job_reports_its_outcome() {
    let state = state_dir("wait");
    write_record(&state, "1-000001", "succeeded", now());
    write_record(&state, "2-000002", "failed", now());

    let (code, stdout, _) = run(&state, &["--json", "jobs", "wait", "1-000001"]);
    assert_eq!(code, 0);
    assert_eq!(document(&stdout)["result"]["job"]["state"], "succeeded");

    // A failed job keeps a non-zero exit code.
    let (code, stdout, _) = run(&state, &["--json", "jobs", "wait", "2-000002"]);
    assert_ne!(code, 0);
    assert_eq!(document(&stdout)["ok"], false);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn detaching_without_credentials_fails_before_a_job_exists() {
    let state = state_dir("detach");

    let (code, stdout, _) = run(
        &state,
        &["--json", "download", "--detach", "--path", "/My Pack"],
    );
    assert_eq!(
        code, 3,
        "auth is the honest answer, not a job that cannot run"
    );

    let doc = document(&stdout);
    assert_eq!(doc["error"]["kind"], "auth");

    let left_behind: Vec<_> = std::fs::read_dir(state.join("jobs"))
        .expect("the jobs directory exists")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        left_behind.is_empty(),
        "nothing should claim to be running: {left_behind:?}"
    );
    let _ = std::fs::remove_dir_all(&state);
}
