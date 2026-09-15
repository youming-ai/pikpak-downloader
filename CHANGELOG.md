# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking

- `FileKind` gained a catch-all `Unknown` variant, so a server-side entry kind
  this crate does not model can no longer fail an entire listing. Downstream
  `match` expressions over `FileKind` must handle it or keep a wildcard arm.
- The crate now declares `rust-version = "1.86"`, driven by the committed
  dependency versions (not by this crate's own syntax).

### Fixed

- **Downloads are no longer cut off by a total request deadline.** File content
  is fetched with a client whose timeout is a per-read stall timeout, so a
  transfer running longer than the configured timeout is not aborted mid-body
  and then failed after the retry budget. Previously any file taking more than
  ~30s to transfer could never finish.
- **A rotated refresh token survives access-token invalidation.** On HTTP 401
  the cached access token is cleared but the rotated refresh token is kept, so
  the next refresh uses the newest value instead of a server-invalidated older
  one.
- **Rotated refresh tokens are persisted the moment the server issues them**,
  not when the command finishes, and are written back to the `.env` that was
  actually loaded (dotenvy also searches parent directories). A token supplied
  through the real environment is never written into an unrelated `.env`.
- **`pikpak ls -h` no longer panics in debug builds.** `-h` is `--help`;
  human-readable sizes moved to `--human`.
- **A partial download is only resumed when it belongs to the same remote file**
  (id, size and modification time, recorded in a `<name>.part.meta` sidecar).
  Foreign or unlabelled partials are discarded instead of being spliced onto
  another version's tail.
- **A 416 response no longer finalizes an oversized partial**, and a transfer is
  never finalized while it is shorter than the reported size. A resume whose
  `Content-Range` starts at the wrong offset restarts cleanly.
- **One unknown entry kind no longer fails a whole listing**, and sizes are
  accepted as either decimal strings or JSON numbers.
- **Pagination terminates on a two-token cycle** (`A -> B -> A -> ...`) as well
  as on an immediately repeated page token.
- **Retry backoff is jittered**, and the download retry budget is consumed only
  by attempts that made no progress — with an absolute ceiling so a server that
  dribbles out a byte per attempt cannot loop forever.
- **The captcha cache lock is no longer held across the network call**, so a
  refresh no longer blocks unrelated actions or cache reads. Concurrent misses
  for one action still collapse into a single init request.
- A `PIKPAK_REFRESH_TOKEN` that is set but empty is reported as a configuration
  error instead of an opaque auth failure.

### Added

- `--path /` downloads the drive root by expanding it into its children.
- Windows file-name handling: reserved device names and trailing dots/spaces are
  adjusted when running on Windows, and re-downloading a name replaces the
  previous file.
- `.env` rewrites are atomic and preserve an `export ` prefix and indentation.
- `Client::download_client()` for content transfers, and
  `ClientBuilder::on_refresh_token` for persisting rotated tokens.
- Small public helpers shared by the CLI and the library: `jittered_backoff`,
  `exponential_backoff`, `is_retryable_status` and `is_drive_root`.
- CI (`fmt --check`, `clippy -D warnings`, `test`) plus an MSRV job pinned to
  1.86.

### Known limitations

- Resuming cannot detect a remote file that changed while keeping both its id
  and its length *and* reporting no modification time.
- A folder download still enumerates the whole tree before the first transfer
  starts (directory creation stays ahead of concurrent downloads; the first
  byte is delayed on very large trees).
