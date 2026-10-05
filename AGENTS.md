# Driving `pikpak` from an agent

This file is the machine contract. It is what a program — an agent, a script, a
CI job — may rely on. Anything not stated here is an implementation detail.

## Two rules

1. **stdout carries data, stderr carries narration.** Transfer progress, retry
   notices, `Downloading:`/`Saved:` lines and warnings all go to stderr. stdout
   is what you parse.
2. **With `--json`, stdout carries exactly one JSON document per run** — the
   result on success, the failure on error. One line, one document.

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
- **`download`** — `{output, files, downloaded, failed, bytes}`. A run that lost
  files still reports what landed before failing, so `failed > 0` with a non-zero
  exit code is a partial result, not a missing one.

## Credentials

Read from the environment, or from the first `.env` in the working directory or
its parents: `PIKPAK_REFRESH_TOKEN` (required), and optionally `PIKPAK_PROXY`,
`PIKPAK_CLIENT_ID`, `PIKPAK_CLIENT_SECRET`.

Refresh tokens are **single-use**: every login rotates them, and this tool
authenticates as the PikPak **Android** client. The CLI writes each replacement
back into the `.env` it loaded, so give it a file to write to — a token supplied
through the environment can only be printed, not persisted.

## Recipes

```bash
# Is the account reachable? Branch on the exit code, not the text.
pikpak --json quota || case $? in 3) reauth;; 5) sleep 30;; esac

# What is in a folder?
pikpak --json ls --path "/My Pack" | jq -r '.result.entries[] | select(.kind=="file") | .id'

# Download a folder, four files at a time.
pikpak --json download --path "/My Pack/Movies" --output /data -j 4
```
