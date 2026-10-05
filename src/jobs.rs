//! Detached download jobs.
//!
//! `download --detach` writes a record and starts a copy of this process; that
//! copy updates the record as it runs, so any later invocation — possibly in a
//! different shell, after the agent's tool call returned — can list, inspect,
//! cancel or wait on the job.
//!
//! Everything lives under the state directory (`PIKPAK_STATE_DIR`, else
//! `.pikpak` in the working directory):
//!
//! ```text
//! .pikpak/jobs/<id>.json     the record
//! .pikpak/jobs/<id>.cancel   present once a cancel has been asked for
//! .pikpak/logs/<id>.log      what the worker printed
//! ```

use std::path::{Path as StdPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::output::Output;
use crate::DownloadArgs;

/// How long a running job may go without a heartbeat before its worker is
/// presumed gone. The worker beats well inside this, so a gap means it died.
const STALE_AFTER_SECS: u64 = 30;
/// How often the worker's watcher looks for a cancel request and beats.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
/// How often `jobs wait` re-reads the record.
const POLL_EVERY: Duration = Duration::from_millis(500);

/// Seconds since the Unix epoch, which is what the records carry.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Where jobs are stored: `PIKPAK_STATE_DIR`, else `.pikpak` in the working
/// directory. Relative on purpose — a job belongs to the directory it was
/// started from, alongside the `.env` and the output it writes.
pub(crate) fn state_dir() -> PathBuf {
    std::env::var_os("PIKPAK_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".pikpak"))
}

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobState {
    /// The worker is (or was) running.
    Running,
    /// It finished everything it was asked for.
    Succeeded,
    /// It gave up; `error` says why.
    Failed,
    /// A cancel was honoured.
    Cancelled,
    /// Its worker vanished without recording an outcome.
    Lost,
}

impl JobState {
    pub(crate) fn is_terminal(self) -> bool {
        !matches!(self, JobState::Running)
    }
}

/// One detached download, as stored and as reported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JobRecord {
    /// Sortable, unique per job.
    pub id: String,
    pub state: JobState,
    /// Remote path the job was asked for.
    pub path: String,
    /// Local directory it writes into.
    pub output: String,
    /// Concurrency it was started with.
    pub jobs: usize,
    /// Worker process id, once known.
    pub pid: Option<u32>,
    /// Epoch seconds.
    pub created_at: u64,
    /// Epoch seconds; the worker refreshes this as a heartbeat.
    pub updated_at: u64,
    /// Files finished so far.
    pub downloaded: usize,
    /// Files that failed.
    pub failed: usize,
    /// Bytes across the files that finished.
    pub bytes: u64,
    /// Why it failed, or why it was lost.
    pub error: Option<String>,
    /// The exit code the worker's own failure maps to, so `jobs wait` can exit
    /// with what a synchronous run would have.
    #[serde(default)]
    pub exit_code: Option<u8>,
}

/// What `download --detach` reports.
#[derive(Debug, Serialize)]
pub(crate) struct DetachedJob {
    /// The remote path the job was asked for.
    pub path: String,
    /// Local directory it writes into.
    pub output: String,
    /// Epoch seconds.
    pub started_at: u64,
    /// Where the worker's output goes.
    pub log: String,
    /// The record, as stored.
    pub job: JobRecord,
}

/// Record files under a state directory.
pub(crate) struct JobStore {
    root: PathBuf,
}

impl JobStore {
    /// Open (creating if needed) the store under `root`.
    pub(crate) fn open(root: &StdPath) -> Result<Self> {
        std::fs::create_dir_all(root.join("jobs"))
            .with_context(|| format!("failed to create {}", root.join("jobs").display()))?;
        std::fs::create_dir_all(root.join("logs"))
            .with_context(|| format!("failed to create {}", root.join("logs").display()))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// The state directory this store lives in, for passing to a worker.
    pub(crate) fn root(&self) -> &StdPath {
        &self.root
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.root.join("jobs").join(format!("{id}.json"))
    }

    fn cancel_path(&self, id: &str) -> PathBuf {
        self.root.join("jobs").join(format!("{id}.cancel"))
    }

    /// Where the worker's stdout and stderr are collected.
    pub(crate) fn log_path(&self, id: &str) -> PathBuf {
        self.root.join("logs").join(format!("{id}.log"))
    }

    /// Write a new record. Refuses to overwrite one that exists.
    pub(crate) fn create(&self, record: &JobRecord) -> Result<()> {
        let path = self.record_path(&record.id);
        if path.exists() {
            bail!("job {} already exists", record.id);
        }
        self.write(record)
    }

    /// Overwrite a record, refreshing its heartbeat.
    pub(crate) fn save(&self, record: &mut JobRecord) -> Result<()> {
        record.updated_at = now_secs();
        self.write(record)
    }

    fn write(&self, record: &JobRecord) -> Result<()> {
        let path = self.record_path(&record.id);
        let body = serde_json::to_vec_pretty(record).context("failed to encode the job record")?;
        // A half-written record would be unreadable to every later caller, so
        // replace it in one step.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &body)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("failed to replace {}", path.display()))
    }

    /// Read one record, reporting a missing job as `not_found`.
    pub(crate) fn get(&self, id: &str) -> Result<JobRecord> {
        let path = self.record_path(id);
        let body =
            std::fs::read(&path).map_err(|_| crate::output::Missing(format!("no job {id}")))?;
        let mut record: JobRecord = serde_json::from_slice(&body)
            .with_context(|| format!("failed to read {}", path.display()))?;
        self.sweep(&mut record);
        Ok(record)
    }

    /// Every record, oldest first. Unreadable records are skipped rather than
    /// failing the listing — one corrupt file should not hide the others.
    pub(crate) fn list(&self) -> Result<Vec<JobRecord>> {
        let dir = self.root.join("jobs");
        let mut records = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read {}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(body) = std::fs::read(&path) else {
                continue;
            };
            match serde_json::from_slice::<JobRecord>(&body) {
                Ok(mut record) => {
                    self.sweep(&mut record);
                    records.push(record);
                }
                Err(error) => {
                    tracing::debug!(path = %path.display(), %error, "skipping an unreadable job record")
                }
            }
        }
        records.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(records)
    }

    /// Ask a job to stop. Returns the record so the caller can report it.
    pub(crate) fn request_cancel(&self, id: &str) -> Result<JobRecord> {
        let record = self.get(id)?;
        std::fs::write(self.cancel_path(id), b"cancel\n")
            .with_context(|| format!("failed to write the cancel request for {id}"))?;
        Ok(record)
    }

    /// Has a cancel been asked for? The worker's watcher calls this.
    pub(crate) fn cancel_requested(&self, id: &str) -> bool {
        self.cancel_path(id).exists()
    }

    /// A running job whose worker stopped beating is reported as lost, rather
    /// than running forever.
    fn sweep(&self, record: &mut JobRecord) {
        if record.state == JobState::Running
            && now_secs().saturating_sub(record.updated_at) > STALE_AFTER_SECS
        {
            record.state = JobState::Lost;
            record.error = Some(format!(
                "the worker stopped reporting ~{STALE_AFTER_SECS}s ago (killed, or the machine slept)"
            ));
        }
    }
}

/// A unique, sortable id: epoch seconds plus microseconds.
pub(crate) fn new_id() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.subsec_micros())
        .unwrap_or(0);
    format!("{}-{:06}", now_secs(), micros)
}

/// Start a detached download and report it.
pub(crate) fn start_detached(args: &DownloadArgs, ui: Output) -> Result<()> {
    let store = JobStore::open(&state_dir())?;
    let id = new_id();
    let mut record = JobRecord {
        id: id.clone(),
        state: JobState::Running,
        path: args.path.clone(),
        output: args.output.clone(),
        jobs: args.jobs,
        pid: None,
        created_at: now_secs(),
        updated_at: now_secs(),
        downloaded: 0,
        failed: 0,
        bytes: 0,
        error: None,
        exit_code: None,
    };
    store.create(&record)?;

    let child = spawn_worker(&id, &store).inspect_err(|_| {
        // Nothing is running, so leave nothing behind that claims otherwise.
        let _ = std::fs::remove_file(store.record_path(&id));
    })?;
    record.pid = Some(child.id());
    store.save(&mut record)?;

    let log = store.log_path(&id);
    if !ui.json() {
        println!("{}", record.id);
        ui.note(format!("job {} started; log: {}", record.id, log.display()));
    }
    ui.ok(
        "download",
        &DetachedJob {
            path: record.path.clone(),
            output: record.output.clone(),
            started_at: record.created_at,
            log: log.display().to_string(),
            job: record,
        },
    )
}

/// Start a copy of this binary to run the job, detached from this process and
/// with its output collected in the job's log.
fn spawn_worker(id: &str, store: &JobStore) -> Result<std::process::Child> {
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().context("cannot locate this executable to detach")?;
    let log = std::fs::File::create(store.log_path(id))
        .with_context(|| format!("failed to create {}", store.log_path(id).display()))?;
    let log_err = log
        .try_clone()
        .context("failed to duplicate the log handle")?;

    let mut command = Command::new(exe);
    command
        .args(std::env::args_os().skip(1))
        .arg("--internal-job")
        .arg(id)
        .arg("--internal-state")
        .arg(store.root())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    // Detach where the platform lets us, so the worker outlives this process
    // and does not share its terminal.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }

    command
        .spawn()
        .context("failed to start the background download")
}

/// The `jobs` subcommand group.
#[derive(Debug, clap::Args)]
pub(crate) struct JobsArgs {
    #[command(subcommand)]
    pub command: JobsCommand,
}

/// Branches of `jobs`.
#[derive(Debug, Subcommand)]
pub(crate) enum JobsCommand {
    /// List known jobs, oldest first.
    List,
    /// Show one job.
    Status {
        /// Job id, as printed by `download --detach`.
        id: String,
    },
    /// Ask a running job to stop at its next chunk boundary.
    Cancel {
        /// Job id.
        id: String,
    },
    /// Block until a job finishes, then exit with its outcome.
    Wait {
        /// Job id.
        id: String,
        /// Give up after this many seconds (0 waits indefinitely).
        #[arg(long, default_value_t = 0)]
        timeout_seconds: u64,
    },
}

/// `pikpak jobs …`.
pub(crate) async fn cmd_jobs(command: JobsCommand, ui: Output) -> Result<()> {
    let store = JobStore::open(&state_dir())?;

    match command {
        JobsCommand::List => {
            let jobs = store.list()?;
            if !ui.json() {
                if jobs.is_empty() {
                    println!("(no jobs)");
                }
                for job in &jobs {
                    println!(
                        "{:<20} {:<10} {:>4}/{:<4} {}",
                        job.id,
                        state_label(job.state),
                        job.downloaded,
                        job.downloaded + job.failed,
                        job.path
                    );
                }
            }
            ui.ok("jobs", &serde_json::json!({ "jobs": jobs }))
        }
        JobsCommand::Status { id } => {
            let job = store.get(&id)?;
            if !ui.json() {
                println!("{}", serde_json::to_string_pretty(&job)?);
            }
            ui.ok("jobs.status", &serde_json::json!({ "job": job }))
        }
        JobsCommand::Cancel { id } => {
            let job = store.request_cancel(&id)?;
            ui.note(format!(
                "cancel requested for {}; the worker stops at its next chunk boundary",
                job.id
            ));
            ui.ok(
                "jobs.cancel",
                &serde_json::json!({ "cancelled": true, "job": job }),
            )
        }
        JobsCommand::Wait {
            id,
            timeout_seconds,
        } => wait_for(&store, &id, timeout_seconds, ui).await,
    }
}

/// Poll a job until it is no longer running.
async fn wait_for(store: &JobStore, id: &str, timeout_seconds: u64, ui: Output) -> Result<()> {
    let started = now_secs();
    loop {
        let job = store.get(id)?;
        if job.state.is_terminal() {
            if !ui.json() {
                println!("{}", serde_json::to_string_pretty(&job)?);
            }
            // One document per run: the success envelope only ever comes from the
            // Succeeded arm, because every other arm returns an error and the
            // failure envelope is emitted once, by `main`.
            //
            // Report the code the worker's own failure produced, so the caller
            // branches exactly as it would on a synchronous run.
            return match job.state {
                JobState::Succeeded => {
                    ui.ok("jobs.wait", &serde_json::json!({ "job": job }))?;
                    Ok(())
                }
                JobState::Cancelled => Err(crate::output::JobFailed {
                    code: crate::output::EXIT_CANCELLED,
                    message: format!("job {id} was cancelled"),
                }
                .into()),
                JobState::Lost => Err(crate::output::JobFailed {
                    code: crate::output::EXIT_UNEXPECTED,
                    message: format!("job {id} lost its worker"),
                }
                .into()),
                _ => Err(crate::output::JobFailed {
                    code: job.exit_code.unwrap_or(crate::output::EXIT_UNEXPECTED),
                    message: format!(
                        "job {id} failed: {}",
                        job.error
                            .unwrap_or_else(|| "no reason recorded".to_string())
                    ),
                }
                .into()),
            };
        }
        if timeout_seconds > 0 && now_secs().saturating_sub(started) >= timeout_seconds {
            return Err(crate::output::Timeout(format!(
                "job {id} is still running after {timeout_seconds}s"
            ))
            .into());
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

fn state_label(state: JobState) -> &'static str {
    match state {
        JobState::Running => "running",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
        JobState::Lost => "lost",
    }
}

/// Run the download for a detached job, keeping its record current.
pub(crate) async fn run_worker_download(
    client: &pikpak::Client,
    args: DownloadArgs,
    ui: Output,
    id: String,
    state_root: Option<PathBuf>,
) -> Result<()> {
    let store = JobStore::open(state_root.as_deref().unwrap_or(&state_dir()))?;
    let stop = Arc::new(AtomicBool::new(false));

    // The watcher is the worker's only tie to the outside world: it beats so
    // `jobs list` can tell a live worker from a dead one, and it honours a
    // cancel request. A cancel ends the process at the next check, which leaves
    // the `.part` file in place — the resume machinery already treats a torn
    // partial as a valid resume point.
    let watcher = {
        let store_id = id.clone();
        let store_root = store.root().to_path_buf();
        let stop = stop.clone();
        tokio::spawn(async move {
            let Ok(store) = JobStore::open(&store_root) else {
                return;
            };
            loop {
                if store.cancel_requested(&store_id) {
                    stop.store(true, Ordering::Relaxed);
                    if let Ok(mut record) = store.get(&store_id) {
                        record.state = JobState::Cancelled;
                        record.error = Some("cancelled on request".to_string());
                        record.exit_code = Some(crate::output::EXIT_CANCELLED);
                        let _ = store.save(&mut record);
                    }
                    // The part file stays for a later resume; nothing else here
                    // is worth waiting for.
                    std::process::exit(crate::output::EXIT_OK.into());
                }
                if let Ok(mut record) = store.get(&store_id) {
                    let _ = store.save(&mut record);
                }
                tokio::time::sleep(HEARTBEAT_EVERY).await;
            }
        })
    };

    let outcome = crate::download::cmd_download(client, args, ui).await;
    watcher.abort();

    let mut record = store.get(&id)?;
    match outcome {
        Ok(summary) => {
            record.state = JobState::Succeeded;
            record.downloaded = summary.downloaded;
            record.failed = summary.failed;
            record.bytes = summary.bytes;
            record.error = None;
            record.exit_code = Some(crate::output::EXIT_OK);
            store.save(&mut record)?;
            Ok(())
        }
        Err(error) => {
            record.state = JobState::Failed;
            record.error = Some(format!("{error:#}"));
            record.exit_code = Some(crate::output::exit_code_for(&error).0);
            let _ = store.save(&mut record);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
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
}
