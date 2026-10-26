# Agent contract

Use `pikpak --json <command>` for automation. The CLI never prompts or reads stdin.
For installation and credential setup, see [README.md](README.md#setup).

## Output

- stdout contains exactly one JSON document on one line: success or failure.
- Progress, retries, warnings, and debug logs go to stderr.
- Exceptions: `--help` and `--version` print text and exit 0. Invalid arguments
  print to stderr, leave stdout empty, and exit 2.
- Parse JSON without relying on field order. Branch on exit codes, not messages.

```json
{"ok":true,"command":"quota","result":{"total":100,"used":40,"free":60,"usage_percent":40.0}}
```

```json
{"ok":false,"command":"ls","error":{"kind":"auth","exit_code":3,"message":"not configured: missing PIKPAK_REFRESH_TOKEN"}}
```

These are two separate example runs. The process exit code equals
`error.exit_code` on failure. Codes are append-only.

| Exit | `error.kind` | Meaning / action |
| --- | --- | --- |
| 0 | — | Success; parse `result` |
| 1 | `unexpected` | Bug, malformed or contradictory response; report it |
| 2 | — | Usage error; fix arguments |
| 3 | `auth` | Missing, blank, rejected, or expired token; supply a fresh token |
| 4 | `not_found` | Remote path, content, or job missing; fix path or ID |
| 5 | `network` | Transport failure or HTTP 429/5xx; retry later |
| 6 | `refused` | Service refusal, including a persistently rejected download link; investigate before retrying |
| 7 | `io` | Local filesystem failure; check permissions and space |
| 8 | `timeout` | Wait deadline reached; job continues, wait again or cancel |
| 9 | `cancelled` | Job cancelled |

## Commands and results

Global flags may precede or follow the subcommand: `--json`, `--no-progress`,
`--verbose`. Progress is drawn only on a terminal's stderr.

| Command | `result` fields |
| --- | --- |
| `quota` | `total`, `used`, `free`, `usage_percent` |
| `ls --path <path>` | `path`, `entries: [{id, name, kind, size, modified_time, mime_type}]` |
| `download --path <path> --output <dir> -j 4` | `output`, `files`, `downloaded`, `failed`, `bytes` |
| `download --detach --path <path> --output <dir>` | `job`, `log`, `output`, `path`, `started_at` |
| `jobs list` | `jobs` (oldest first) |
| `jobs status <id>` | `job` |
| `jobs cancel <id>` | `cancelled`, `job` |
| `jobs wait <id> --timeout-seconds <n>` | `job` on success; failure envelope otherwise |

Job commands report `command` as `jobs.list`, `jobs.status`, `jobs.cancel`, or
`jobs.wait`, including on failure.

- Sizes are bytes, including JSON output with `--raw` or `--human`.
  `usage_percent` is `null` when capacity is zero.
- `ls.path` echoes the requested path. An empty folder has `entries: []`.
  Entry `kind` is `file`, `folder`, or `unknown`; absent `modified_time` and
  `mime_type` are `null`.
- Downloads skip non-empty local files matching the listed size and count them
  in `downloaded`. Empty files are fetched again: zero can mean an unknown size.
- Failed downloads emit only the failure document, with counts in `error.message`.
  Unknown entry kinds count as failed files; other entries still download.

## Credentials

Environment values take precedence over the first `.env` found in the current
directory or its parents. `PIKPAK_REFRESH_TOKEN` is required for remote commands.
Optional settings are listed in README.md.

Tokens are single-use and this tool uses the Android client. Keep tokens in a
writable `.env` so replacements are saved; environment-only replacements are
printed to stderr. The CLI saves a device ID to `.env` when one is absent.

## Background jobs

```bash
id=$(pikpak --json download --detach --path "/My Pack/Movies" --output /data | jq -r '.result.job.id')
pikpak --json jobs wait "$id" --timeout-seconds 1800
```

The worker outlives the launching command. All `jobs` commands work without
credentials. Use `result.job.id` for subsequent calls.

- States: `running`, `succeeded`, `failed`, `cancelled`, `lost`.
- Records include `id`, `state`, `path`, `output`, `jobs`, `pid`, `created_at`,
  `updated_at`, `files`, `downloaded`, `failed`, `bytes`, `error`, and `exit_code`.
  Timestamps are Unix seconds; `pid`, `error`, and `exit_code` may be `null`.
- Heartbeats update every 5s with current counts; `files` is 0 until planning
  completes. Final counts remain available even on failure. A running job with
  no heartbeat for about 30s is reported `lost`.
- `wait` exits with the job's outcome: 0 on success, its failure code otherwise,
  9 for cancellation, 1 for a lost worker, or 8 if the wait deadline expires.
  Timeout 0 (default) waits indefinitely; set a deadline for bounded tool calls.
- Cancellation is cooperative and takes a few seconds. Partials remain under
  `<output>/.pikpak-parts/` for resumption. Cancelling a finished job returns
  `cancelled: false` and keeps its final state.

State root: `PIKPAK_STATE_DIR`, else `.pikpak` beside the loaded `.env`, else
`.pikpak` in the current directory. Records are `jobs/<id>.json`, cancellation
markers `jobs/<id>.cancel`, and worker logs `logs/<id>.log`. Older jobs stored
under a subdirectory's `.pikpak` remain there; point `PIKPAK_STATE_DIR` at it.
