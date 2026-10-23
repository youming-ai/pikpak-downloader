//! Detached download jobs.
//!
//! `download --detach` writes a record and starts a copy of this process; that
//! copy updates the record as it runs, so any later invocation — possibly in a
//! different shell, after the agent's tool call returned — can list, inspect,
//! cancel or wait on the job.
//!
//! Everything lives under the state directory (`PIKPAK_STATE_DIR`, else
//! `.pikpak` beside the `.env` that was loaded, else in the working directory):
//!
//! ```text
//! .pikpak/jobs/<id>.json     the record
//! .pikpak/jobs/<id>.cancel   present once a cancel has been asked for
//! .pikpak/logs/<id>.log      what the worker printed
//! ```

use std::path::{Path as StdPath, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::download::{DownloadSummary, Tally};
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

/// Where jobs are stored: `PIKPAK_STATE_DIR`, else `.pikpak` beside `env_file`
/// (the `.env` that was loaded), else `.pikpak` in the working directory.
///
/// Anchored to the `.env` because dotenvy finds that file from any directory
/// below it: a job started from a subdirectory must be found again from its
/// parent, and the other way round.
pub(crate) fn state_dir(env_file: Option<&StdPath>) -> PathBuf {
    if let Some(dir) = std::env::var_os("PIKPAK_STATE_DIR") {
        return PathBuf::from(dir);
    }
    env_file
        .and_then(StdPath::parent)
        .map_or_else(|| PathBuf::from(".pikpak"), |dir| dir.join(".pikpak"))
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
    /// Files the run planned to transfer; 0 until planning is done.
    #[serde(default)]
    pub files: usize,
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

impl JobRecord {
    /// Copy the run's counts into the record.
    fn count(&mut self, tally: &Tally) {
        self.files = tally.files.load(Ordering::Relaxed);
        self.downloaded = tally.downloaded.load(Ordering::Relaxed);
        self.failed = tally.failed.load(Ordering::Relaxed);
        self.bytes = tally.bytes.load(Ordering::Relaxed);
    }
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
        // Ancestors of a `PIKPAK_STATE_DIR` get the usual permissions; they are
        // not ours to lock down.
        if let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        // The state directory itself is private: a worker's log can hold a
        // rotated refresh token it had nowhere else to put, and records carry
        // the service's error text.
        let mut dirs = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            dirs.mode(0o700);
        }
        for dir in [root.to_path_buf(), root.join("jobs"), root.join("logs")] {
            match dirs.create(&dir) {
                Ok(()) => {}
                // `mkdir` says this for a plain file too; only a directory will do.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => {
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to create {}", dir.display()))
                }
            }
        }
        // `jobs` and `logs` are this tool's own, so ones an older version created
        // world-readable are tightened too — and a store that cannot be made
        // private is not used, since its logs can hold a token. A symlink is
        // left alone: tightening would change whatever it points at. The root
        // may be a directory the user pointed `PIKPAK_STATE_DIR` at; that one is
        // left as it is.
        #[cfg(unix)]
        for dir in [root.join("jobs"), root.join("logs")] {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                    .with_context(|| format!("failed to make {} private", dir.display()))?;
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// The store under `root`, as it is: nothing is created. For the `jobs`
    /// commands — asking what is running must not create a state tree (perhaps
    /// beside a `.env` in a parent directory) or fail where that is read-only.
    /// The one write among them, `cancel`'s marker, happens only for a job whose
    /// record exists, so its directory does too.
    pub(crate) fn at(root: &StdPath) -> Self {
        Self {
            root: root.to_path_buf(),
        }
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
        // replace it in one step — through a temp file of this process's own, so
        // two writers never share one.
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        // Owner-only, like the log: a record carries the service's error text.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&tmp)
            .and_then(|mut file| std::io::Write::write_all(&mut file, &body))
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("failed to replace {}", path.display()))
    }

    /// Read one record, reporting a missing job as `not_found`.
    pub(crate) fn get(&self, id: &str) -> Result<JobRecord> {
        let mut record = self.read(id)?;
        self.sweep(&mut record);
        Ok(record)
    }

    /// Read one record as stored. The worker reads this way: it is the live
    /// worker, so it must never mistake its own record for a lost one.
    fn read(&self, id: &str) -> Result<JobRecord> {
        let path = self.record_path(id);
        let body =
            std::fs::read(&path).map_err(|_| crate::output::Missing(format!("no job {id}")))?;
        serde_json::from_slice(&body).with_context(|| format!("failed to read {}", path.display()))
    }

    /// The worker's heartbeat: refresh `updated_at`, record its pid and the
    /// run's counts so far. Only a running record is touched, so a beat can
    /// never undo an outcome.
    pub(crate) fn beat(&self, id: &str, tally: &Tally) -> Result<()> {
        let mut record = self.read(id)?;
        if record.state == JobState::Running {
            record.pid = Some(std::process::id());
            record.count(tally);
            self.save(&mut record)?;
        }
        Ok(())
    }

    /// Every record, oldest first. Unreadable records are skipped rather than
    /// failing the listing — one corrupt file should not hide the others.
    pub(crate) fn list(&self) -> Result<Vec<JobRecord>> {
        let dir = self.root.join("jobs");
        let mut records = Vec::new();
        // No store yet means no jobs yet.
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", dir.display()))
            }
        };
        for entry in entries.flatten() {
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

    /// Ask a job to stop. Returns the record so the caller can report it, and
    /// whether a cancel was requested: a job that already finished is left
    /// alone, with no marker written.
    pub(crate) fn request_cancel(&self, id: &str) -> Result<(JobRecord, bool)> {
        // Decide on the stored state, not the swept one: a worker that stalled
        // past the window reads as `lost` but may resume, and must still see the
        // cancel when it does. A marker beside a truly dead job is harmless.
        let mut record = self.read(id)?;
        let requested = !record.state.is_terminal();
        if requested {
            std::fs::write(self.cancel_path(id), b"cancel\n")
                .with_context(|| format!("failed to write the cancel request for {id}"))?;
        }
        self.sweep(&mut record);
        Ok((record, requested))
    }

    /// Drop a cancel request that arrived too late to matter.
    fn clear_cancel(&self, id: &str) {
        let _ = std::fs::remove_file(self.cancel_path(id));
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
///
/// `token_from_file` says the token came from a `.env` this process could write
/// back to; the worker is then made to load it from that file too, so it writes
/// its rotations back there instead of only printing them.
pub(crate) fn start_detached(
    args: &DownloadArgs,
    ui: Output,
    token_from_file: bool,
    verbose: bool,
    state: &StdPath,
) -> Result<DetachedJob> {
    let store = JobStore::open(state)?;
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
        files: 0,
        downloaded: 0,
        failed: 0,
        bytes: 0,
        error: None,
        exit_code: None,
    };
    store.create(&record)?;

    let child = spawn_worker(&id, &store, token_from_file, verbose).inspect_err(|_| {
        // Nothing is running, so leave nothing behind that claims otherwise.
        let _ = std::fs::remove_file(store.record_path(&id));
    })?;
    // Reported, not saved: once the worker runs it is the record's only writer,
    // and a save from here could land after its outcome and undo it. Its first
    // heartbeat stores the pid.
    record.pid = Some(child.id());

    let log = store.log_path(&id);
    if !ui.json() {
        println!("{}", record.id);
        eprintln!("job {} started; log: {}", record.id, log.display());
    }
    Ok(DetachedJob {
        path: record.path.clone(),
        output: record.output.clone(),
        started_at: record.created_at,
        log: log.display().to_string(),
        job: record,
    })
}

/// The worker's command line: the job and where its record lives. Everything
/// else it needs is in the record.
fn worker_command(
    exe: PathBuf,
    id: &str,
    state_root: &StdPath,
    token_from_file: bool,
    verbose: bool,
) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    if verbose {
        command.arg("--verbose");
    }
    command
        .arg("__worker")
        .arg("--state")
        .arg(state_root)
        .arg(id);
    // dotenvy copied the file's token into this environment. Inherited, it
    // would look to the worker like a token from the real environment — one it
    // must not write back — and every rotation would be lost with the worker.
    if token_from_file {
        command.env_remove("PIKPAK_REFRESH_TOKEN");
    }
    command
}

/// Start a copy of this binary to run the job, detached from this process and
/// with its output collected in the job's log.
fn spawn_worker(
    id: &str,
    store: &JobStore,
    token_from_file: bool,
    verbose: bool,
) -> Result<std::process::Child> {
    use std::process::Stdio;

    let exe = std::env::current_exe().context("cannot locate this executable to detach")?;
    // Owner-only, even inside a state directory created by an older version
    // without private permissions: the log can hold a refresh token.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options
        .open(store.log_path(id))
        .with_context(|| format!("failed to create {}", store.log_path(id).display()))?;
    let log_err = log
        .try_clone()
        .context("failed to duplicate the log handle")?;

    let mut command = worker_command(exe, id, store.root(), token_from_file, verbose);
    command
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
    /// Ask a running job to stop; its worker exits within a few seconds.
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

impl JobsCommand {
    /// The name reported in JSON documents, on success and failure alike.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            JobsCommand::List => "jobs.list",
            JobsCommand::Status { .. } => "jobs.status",
            JobsCommand::Cancel { .. } => "jobs.cancel",
            JobsCommand::Wait { .. } => "jobs.wait",
        }
    }
}

/// Arguments of the hidden worker command.
#[derive(Debug, clap::Args)]
pub(crate) struct WorkerArgs {
    /// State directory the job's record lives in.
    #[arg(long)]
    pub state: PathBuf,
    /// The job this process runs.
    pub id: String,
}

/// `pikpak jobs …`.
pub(crate) async fn cmd_jobs(command: JobsCommand, ui: Output, state: &StdPath) -> Result<Value> {
    // A job that exists has its directory; one that does not is `not_found`.
    let store = JobStore::at(state);

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
                        job.files,
                        job.path
                    );
                }
            }
            Ok(json!({ "jobs": jobs }))
        }
        JobsCommand::Status { id } => {
            let job = store.get(&id)?;
            if !ui.json() {
                println!("{}", serde_json::to_string_pretty(&job)?);
            }
            Ok(json!({ "job": job }))
        }
        JobsCommand::Cancel { id } => {
            let (job, cancelled) = store.request_cancel(&id)?;
            if cancelled {
                eprintln!(
                    "cancel requested for {}; the worker stops within a few seconds",
                    job.id
                );
            } else {
                eprintln!(
                    "job {} already {}; nothing to cancel",
                    job.id,
                    state_label(job.state)
                );
            }
            Ok(json!({ "cancelled": cancelled, "job": job }))
        }
        JobsCommand::Wait {
            id,
            timeout_seconds,
        } => wait_for(&store, &id, timeout_seconds, ui).await,
    }
}

/// Poll a job until it is no longer running.
async fn wait_for(store: &JobStore, id: &str, timeout_seconds: u64, ui: Output) -> Result<Value> {
    let started = now_secs();
    loop {
        let job = store.get(id)?;
        if job.state.is_terminal() {
            if !ui.json() {
                println!("{}", serde_json::to_string_pretty(&job)?);
            }
            // Report the code the worker's own failure produced, so the caller
            // branches exactly as it would on a synchronous run.
            return match job.state {
                JobState::Succeeded => Ok(json!({ "job": job })),
                JobState::Cancelled => Err(crate::output::Coded {
                    code: crate::output::EXIT_CANCELLED,
                    message: format!("job {id} was cancelled"),
                }
                .into()),
                JobState::Lost => Err(crate::output::Coded {
                    code: crate::output::EXIT_UNEXPECTED,
                    message: format!("job {id} lost its worker"),
                }
                .into()),
                _ => Err(crate::output::Coded {
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
///
/// The arguments come from the record `download --detach` wrote. `client` is
/// taken unbuilt, so a client that cannot be built — a bad proxy, a token gone
/// from the `.env` — is recorded as the job's failure rather than leaving it
/// to be found lost.
pub(crate) async fn run_worker(
    client: Result<pikpak::Client>,
    worker: &WorkerArgs,
) -> Result<DownloadSummary> {
    let store = JobStore::open(&worker.state)?;
    let id = worker.id.clone();
    let record = store.read(&id)?;
    let args = DownloadArgs {
        path: record.path,
        output: record.output,
        jobs: record.jobs,
        detach: false,
    };

    // The watcher is the worker's only tie to the outside world: it beats so
    // `jobs list` can tell a live worker from a dead one, and it honours a
    // cancel request. A cancel ends the process at the next check, which leaves
    // the partial in place — the resume machinery already treats a torn
    // partial as a valid resume point.
    let tally = Arc::new(Tally::default());
    let watcher = {
        let tally = tally.clone();
        let store_id = id.clone();
        let store_root = store.root().to_path_buf();
        tokio::spawn(async move {
            let Ok(store) = JobStore::open(&store_root) else {
                return;
            };
            loop {
                if store.cancel_requested(&store_id) {
                    if let Ok(mut record) = store.read(&store_id) {
                        record.state = JobState::Cancelled;
                        record.error = Some("cancelled on request".to_string());
                        record.exit_code = Some(crate::output::EXIT_CANCELLED);
                        let _ = store.save(&mut record);
                    }
                    // The part file stays for a later resume; nothing else here
                    // is worth waiting for.
                    std::process::exit(crate::output::EXIT_OK.into());
                }
                let _ = store.beat(&store_id, &tally);
                tokio::time::sleep(HEARTBEAT_EVERY).await;
            }
        })
    };

    // A log is no terminal: narration only, no progress redraws.
    let ui = Output::new(false, true);
    let outcome = match client {
        Ok(client) => crate::download::cmd_download(&client, args, ui, &tally).await,
        Err(error) => Err(error),
    };
    // Abort only takes effect at the watcher's next await; wait for it, so no
    // beat or cancel of its is still in flight when the outcome is written.
    watcher.abort();
    let _ = watcher.await;
    // A cancel that arrived after the transfer finished changes nothing.
    store.clear_cancel(&id);

    let mut record = store.read(&id)?;
    // Failed or not, the record keeps what the run got done.
    record.count(&tally);
    match outcome {
        Ok(summary) => {
            record.state = JobState::Succeeded;
            record.error = None;
            record.exit_code = Some(crate::output::EXIT_OK);
            store.save(&mut record)?;
            Ok(summary)
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
mod tests;
