# Driving `pikpak` from an agent

This file is the machine contract. It is what a program — an agent, a script, a
CI job — may rely on. Anything not stated here is an implementation detail.

## Two rules

1. **stdout carries data, stderr carries narration.** Transfer progress, retry
   notices, `Downloading:`/`Saved:` lines and warnings all go to stderr. stdout
   is what you parse.
2. **With `--json`, stdout carries exactly one JSON document per run** — the
   result on success, the failure on error. One line, one document. A run that
   fails emits only the failure; for a download that lost files the counts are
   in `error.message`, because two documents on one stream is not a contract.

The tool never prompts and never reads stdin, so it is safe to run unattended.

## Invocation

```bash
pikpak [--json] [--no-progress] [--verbose] <command> [options]
```

| flag | effect |
| --- | --- |
| `--json` | Emit the JSON document on stdout. Global: it may precede or follow the subcommand. |
| `--no-progress` | Never draw byte-level progress (it is stderr-only either way). |
| `--verbose` | Debug logging on stderr. |

## Exit codes

Branch on these rather than on the message text. They are append-only.

| code | `error.kind` | meaning | what to do |
| --- | --- | --- | --- |
| 0 | — | success | parse `result` |
| 1 | `unexpected` | a bug, or a response we could not parse | report it; retrying rarely helps |
| 2 | — | usage error (clap) | fix the arguments |
| 3 | `auth` | token missing, blank, rejected or no longer refreshable | supply a fresh `PIKPAK_REFRESH_TOKEN` |
| 4 | `not_found` | the remote path does not exist | fix the path |
| 5 | `network` | transport failure, or 429/5xx after retries | retry later |
| 6 | `refused` | the service refused it for another reason | report it; do not retry blindly |
| 7 | `io` | a local filesystem operation failed | fix permissions/space |
| 8 | `timeout` | `jobs wait` hit the deadline you gave it | the job is still running; wait again or cancel |
| 9 | `cancelled` | the job was cancelled before it finished | nothing to fix |

## Documents

Failure — the process exit code equals `error.exit_code`:

```json
{"ok":false,"command":"ls","error":{"kind":"auth","message":"not configured: missing PIKPAK_REFRESH_TOKEN","exit_code":3}}
```

Success — always `{"ok":true,"command":<name>,"result":<payload>}`:

```bash
$ pikpak --json quota
{"ok":true,"command":"quota","result":{"total":10995116277760,"used":4650000000000,"free":6345116277760,"usage_percent":42.3}}
```

`result` payloads:

- **`quota`** — `{total, used, free, usage_percent}`. Bytes, always: `--raw`
  and the human formatting only shape the text a person reads.
  `usage_percent` is `null` when the account reports no capacity.
- **`ls`** — `{path, entries: [{id, name, kind, size, modified_time, mime_type}]}`.
  `kind` is `"file"`, `"folder"` or `"unknown"`; `modified_time` and `mime_type`
  are `null` when the server did not report them. `path` is what you asked for.
  An empty listing is `entries: []`, not an error.
- **`download`** — `{output, files, downloaded, failed, bytes}`. Reported when the
  run finished everything it planned. If some files failed, the run emits the
  *failure* document instead, with the counts in `error.message`; there is never
  a second document on stdout.

## Credentials

Read from the environment, or from the first `.env` in the working directory or
its parents: `PIKPAK_REFRESH_TOKEN` (required), and optionally `PIKPAK_PROXY`,
`PIKPAK_CLIENT_ID`, `PIKPAK_CLIENT_SECRET`.

Refresh tokens are **single-use**: every login rotates them, and this tool
authenticates as the PikPak **Android** client. The CLI writes each replacement
back into the `.env` it loaded, so give it a file to write to — a token supplied
through the environment can only be printed, not persisted.

## Detached downloads

A transfer that outlives your tool call: `--detach` starts a worker and returns
immediately. The worker keeps running after that process exits.

```console
$ pikpak --json download --detach --path "/My Pack/Movies" --output /data
{"ok":true,"command":"download","result":{"path":"/My Pack/Movies","output":"/data","started_at":1770000000,"log":".pikpak/logs/1770000000-123456.log","job":{"id":"1770000000-123456",…}}}
```

`result.job.id` is what every later call takes. The job's records live under
`PIKPAK_STATE_DIR` (default `.pikpak`), beside the `.env` you started from:

| path | what |
| --- | --- |
| `jobs/<id>.json` | the record: state, path, output, pid, timestamps, progress, error |
| `jobs/<id>.cancel` | created by `jobs cancel`; the worker watches for it |
| `logs/<id>.log` | everything the worker printed |

```bash
pikpak --json jobs list                          # {"jobs":[…]}, oldest first
pikpak --json jobs status <id>                   # {"job":{…}}
pikpak --json jobs cancel <id>                   # {"cancelled":true,"job":{…}}
pikpak --json jobs wait <id> --timeout-seconds 600
```

Each reports `command` as `jobs.list`, `jobs.status`, `jobs.cancel` or
`jobs.wait` — the same name on success and on failure.

- **States**: `running`, `succeeded`, `failed`, `cancelled`, `lost`. A `running`
  job whose worker has not beat for ~30s is reported `lost` — never forever
  running. The worker refreshes `updated_at` every 5s while it is alive.
- **`jobs wait` exits with the outcome**: `0` succeeded, or the same code a
  synchronous run would have produced (`3` auth, `5` network, …), `9` if it was
  cancelled, `8` if your deadline passed first. The record carries the worker's
  own `exit_code` for exactly this.
- **`jobs cancel` is cooperative**: it drops a marker and the worker stops at its
  next check (within a few seconds; a chunk in flight finishes). The `.part` file
  is left in place, so re-running the same download resumes. Cancelling a job
  that already finished changes nothing and reports `"cancelled":false` with its
  final state.
- **`jobs list`/`status`/`cancel` need no credentials** — they only read the
  state directory. That is deliberate: asking what is running must not fail
  because the token has since rotated away.
- **`jobs wait` with `--timeout-seconds 0`** (the default) waits indefinitely;
  give it a value if your runtime has its own deadline.

## Recipes

```bash
# Is the account reachable? Branch on the exit code, not the text.
pikpak --json quota || case $? in 3) reauth;; 5) sleep 30;; esac

# What is in a folder?
pikpak --json ls --path "/My Pack" | jq -r '.result.entries[] | select(.kind=="file") | .id'

# Download a folder, four files at a time.
pikpak --json download --path "/My Pack/Movies" --output /data -j 4

# Start it in the background, then follow it from anywhere.
id=$(pikpak --json download --detach --path "/My Pack/Movies" --output /data | jq -r .result.job.id)
pikpak --json jobs wait "$id" --timeout-seconds 1800
```
