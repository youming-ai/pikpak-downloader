//! Tests for detached-job records, their store, and the worker.

use super::*;

fn temp_state(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pikpak-jobs-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn record(id: &str, state: JobState, updated_at: u64) -> JobRecord {
    JobRecord {
        id: id.to_string(),
        state,
        path: "/My Pack".to_string(),
        output: "./downloads".to_string(),
        jobs: 2,
        pid: Some(4242),
        created_at: updated_at,
        updated_at,
        files: 1,
        downloaded: 1,
        failed: 0,
        bytes: 10,
        error: None,
        exit_code: None,
    }
}

#[test]
fn ids_sort_in_creation_order() {
    let first = new_id();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = new_id();
    assert!(second > first, "{second} should sort after {first}");
}

#[test]
fn a_record_round_trips_through_the_store() {
    let root = temp_state("roundtrip");
    let store = JobStore::open(&root).unwrap();

    store
        .create(&record("1-000001", JobState::Running, now_secs()))
        .unwrap();

    let read = store.get("1-000001").unwrap();
    assert_eq!(read.path, "/My Pack");
    assert_eq!(read.jobs, 2);
    assert_eq!(read.state, JobState::Running);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn creating_a_job_twice_is_refused() {
    let root = temp_state("duplicate");
    let store = JobStore::open(&root).unwrap();
    store
        .create(&record("1-000001", JobState::Running, now_secs()))
        .unwrap();

    assert!(store
        .create(&record("1-000001", JobState::Running, now_secs()))
        .is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unknown_job_is_a_not_found_failure() {
    let root = temp_state("missing");
    let store = JobStore::open(&root).unwrap();

    let error = store.get("nope").unwrap_err();
    let missing = error
        .downcast_ref::<crate::output::Missing>()
        .expect("an unknown job reports itself as missing");
    assert!(missing.0.contains("nope"), "{missing}");
    assert_eq!(
        crate::output::exit_code_for(&error),
        (crate::output::EXIT_NOT_FOUND, "not_found")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_running_job_that_stopped_beating_is_reported_lost() {
    let root = temp_state("stale");
    let store = JobStore::open(&root).unwrap();

    // Fresh beat: still running.
    let id = "1-000001";
    store
        .create(&record(id, JobState::Running, now_secs()))
        .unwrap();
    assert_eq!(store.get(id).unwrap().state, JobState::Running);

    // Silent for longer than the stale window: reported lost, with a reason.
    let old = now_secs() - STALE_AFTER_SECS - 1;
    store
        .create(&record("2-000002", JobState::Running, old))
        .unwrap();
    let lost = store.get("2-000002").unwrap();
    assert_eq!(lost.state, JobState::Lost);
    assert!(lost.error.unwrap().contains("stopped reporting"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_terminal_job_is_never_reported_lost() {
    let root = temp_state("terminal");
    let store = JobStore::open(&root).unwrap();
    let old = now_secs() - STALE_AFTER_SECS - 1;

    store
        .create(&record("1-000001", JobState::Succeeded, old))
        .unwrap();
    store
        .create(&record("2-000002", JobState::Failed, old))
        .unwrap();
    store
        .create(&record("3-000003", JobState::Cancelled, old))
        .unwrap();

    assert_eq!(store.get("1-000001").unwrap().state, JobState::Succeeded);
    assert_eq!(store.get("2-000002").unwrap().state, JobState::Failed);
    assert_eq!(store.get("3-000003").unwrap().state, JobState::Cancelled);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancel_requests_are_recorded_and_noticed() {
    let root = temp_state("cancel");
    let store = JobStore::open(&root).unwrap();
    store
        .create(&record("1-000001", JobState::Running, now_secs()))
        .unwrap();

    assert!(!store.cancel_requested("1-000001"));
    store.request_cancel("1-000001").unwrap();
    assert!(store.cancel_requested("1-000001"));
    // Asking to cancel an unknown job is a not-found, not a silent success.
    assert!(store.request_cancel("nope").is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancellation_keeps_counts_completed_since_the_last_heartbeat() {
    let root = temp_state("cancel-counts");
    let store = JobStore::open(&root).unwrap();
    let id = "1-000001";
    store
        .create(&record(id, JobState::Running, now_secs()))
        .unwrap();
    let tally = Tally::default();
    store.beat(id, &tally).unwrap();
    tally.files.store(5, Ordering::Relaxed);
    tally.downloaded.store(2, Ordering::Relaxed);
    tally.failed.store(1, Ordering::Relaxed);
    tally.bytes.store(4096, Ordering::Relaxed);
    store.request_cancel(id).unwrap();
    store
        .finish(
            id,
            &tally,
            JobState::Cancelled,
            Some("cancelled on request".to_string()),
            crate::output::EXIT_CANCELLED,
        )
        .unwrap();

    let job = store.get(id).unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(job.exit_code, Some(crate::output::EXIT_CANCELLED));
    assert_eq!(
        (job.files, job.downloaded, job.failed, job.bytes),
        (5, 2, 1, 4096)
    );
}

#[cfg(unix)]
#[test]
fn the_state_directory_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_state("private");
    JobStore::open(&root).unwrap();

    for dir in [&root, &root.join("jobs"), &root.join("logs")] {
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", dir.display());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn an_older_world_readable_store_is_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_state("tighten");
    for dir in ["jobs", "logs"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::set_permissions(root.join(dir), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let store = JobStore::open(&root).unwrap();
    store
        .create(&record("1-000001", JobState::Running, now_secs()))
        .unwrap();

    let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(root.join("jobs")), 0o700);
    assert_eq!(mode(root.join("logs")), 0o700);
    assert_eq!(mode(root.join("jobs").join("1-000001.json")), 0o600);
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn a_symlinked_logs_directory_is_not_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_state("symlinked");
    let elsewhere = temp_state("symlinked-target");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("logs")).unwrap();

    JobStore::open(&root).unwrap();

    let mode = std::fs::metadata(&elsewhere).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "the link's target is not ours to lock down");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&elsewhere);
}

#[test]
fn a_file_where_the_jobs_directory_belongs_is_an_error() {
    let root = temp_state("not-a-dir");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("jobs"), b"").unwrap();
    assert!(JobStore::open(&root).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn ancestors_of_the_state_directory_keep_their_usual_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let base = temp_state("ancestors");
    let mode = |dir: &StdPath| std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;

    JobStore::open(&base.join("a").join("state")).unwrap();
    // What a plain mkdir gives under this umask, for comparison.
    std::fs::create_dir_all(base.join("plain")).unwrap();

    assert_eq!(mode(&base.join("a")), mode(&base.join("plain")));
    assert_eq!(mode(&base.join("a").join("state")), 0o700);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_heartbeat_keeps_a_stalled_worker_running_and_never_undoes_an_outcome() {
    let root = temp_state("beat");
    let store = JobStore::open(&root).unwrap();
    let old = now_secs() - STALE_AFTER_SECS - 1;

    // The worker stalled past the window (the machine slept) but is alive:
    // its next beat revives the record instead of writing `lost` back.
    store
        .create(&record("1-000001", JobState::Running, old))
        .unwrap();
    let tally = Tally::default();
    tally.files.store(3, Ordering::Relaxed);
    tally.downloaded.store(2, Ordering::Relaxed);
    tally.bytes.store(4096, Ordering::Relaxed);
    store.beat("1-000001", &tally).unwrap();
    let revived = store.get("1-000001").unwrap();
    assert_eq!(revived.state, JobState::Running);
    assert_eq!(revived.pid, Some(std::process::id()));
    // ...and carries the run's counts so far.
    assert_eq!(
        (revived.files, revived.downloaded, revived.bytes),
        (3, 2, 4096)
    );

    // A beat after the outcome was written leaves the outcome alone.
    store
        .create(&record("2-000002", JobState::Succeeded, old))
        .unwrap();
    store.beat("2-000002", &tally).unwrap();
    let done = store.get("2-000002").unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert_eq!(done.updated_at, old);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelling_a_finished_job_leaves_no_marker() {
    let root = temp_state("cancel-done");
    let store = JobStore::open(&root).unwrap();
    store
        .create(&record("1-000001", JobState::Succeeded, now_secs()))
        .unwrap();

    let (job, requested) = store.request_cancel("1-000001").unwrap();
    assert_eq!(job.state, JobState::Succeeded);
    assert!(!requested);
    assert!(!store.cancel_requested("1-000001"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_stalled_worker_can_still_be_cancelled() {
    let root = temp_state("cancel-stalled");
    let store = JobStore::open(&root).unwrap();
    let old = now_secs() - STALE_AFTER_SECS - 1;
    store
        .create(&record("1-000001", JobState::Running, old))
        .unwrap();

    // Reported lost, but the marker is written for when the worker wakes.
    let (job, requested) = store.request_cancel("1-000001").unwrap();
    assert_eq!(job.state, JobState::Lost);
    assert!(requested);
    assert!(store.cancel_requested("1-000001"));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_worker_whose_client_cannot_be_built_records_the_failure() {
    let root = temp_state("unbuildable");
    let store = JobStore::open(&root).unwrap();
    store
        .create(&record("1", JobState::Running, now_secs()))
        .unwrap();

    let worker = WorkerArgs {
        state: root.clone(),
        id: "1".to_string(),
    };
    let missing = Err(pikpak::Error::NotConfigured("PIKPAK_REFRESH_TOKEN").into());
    run_worker(missing, &worker)
        .await
        .expect_err("no client, no download");

    // Failed with the code a synchronous run would exit with — not left
    // `running` to be found lost.
    let job = store.get("1").unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.exit_code, Some(crate::output::EXIT_AUTH));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_worker_is_told_only_its_job_not_the_original_arguments() {
    let command = worker_command("pikpak".into(), "1", StdPath::new("/s"), false, true);
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["--verbose", "__worker", "--state", "/s", "1"]);
}

#[test]
fn a_worker_reloads_a_file_token_so_it_can_persist_rotations() {
    let removed = |command: &std::process::Command| {
        command
            .get_envs()
            .any(|(key, value)| key == "PIKPAK_REFRESH_TOKEN" && value.is_none())
    };
    let root = StdPath::new(".pikpak");

    assert!(removed(&worker_command(
        "pikpak".into(),
        "1",
        root,
        true,
        false
    )));
    // A token from the real environment is the only copy; it must be passed on.
    assert!(!removed(&worker_command(
        "pikpak".into(),
        "1",
        root,
        false,
        false
    )));
}

#[test]
fn listing_skips_unreadable_records_and_sorts_the_rest() {
    let root = temp_state("list");
    let store = JobStore::open(&root).unwrap();
    store
        .create(&record("2-000002", JobState::Succeeded, now_secs()))
        .unwrap();
    store
        .create(&record("1-000001", JobState::Running, now_secs()))
        .unwrap();
    std::fs::write(root.join("jobs").join("3-000003.json"), b"{ not json").unwrap();

    let jobs = store.list().unwrap();
    let ids: Vec<&str> = jobs.iter().map(|job| job.id.as_str()).collect();
    assert_eq!(ids, vec!["1-000001", "2-000002"]);
    let _ = std::fs::remove_dir_all(&root);
}
