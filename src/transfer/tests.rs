//! Tests for content transfers, against hand-rolled CDN and API mocks.

use super::download_backoff;
use crate::test_http::{read_request, respond, respond_json, TOKEN_JSON};
use std::sync::Arc;

#[test]
fn download_backoff_grows_and_caps() {
    assert!(download_backoff(0) < download_backoff(2));
    assert!(download_backoff(30) <= std::time::Duration::from_millis(15_000));
}

async fn spawn_range_server(full: Vec<u8>) -> std::net::SocketAddr {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let full = full.clone();
            tokio::spawn(async move {
                let req = read_request(&mut sock).await;
                let start = req
                    .head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(|v| v.trim().trim_end_matches('-').to_string())
                    })
                    .and_then(|s| s.parse::<usize>().ok());

                match start {
                    Some(s) if s >= full.len() => {
                        let body = b"range not satisfiable";
                        let hdr = format!(
                            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                    }
                    Some(s) => {
                        let body = &full[s..];
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            body.len(),
                            s,
                            full.len() - 1,
                            full.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                    }
                    None => {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            full.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(&full).await;
                    }
                }
                let _ = sock.flush().await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn download_attempt_resumes_via_range() {
    let full: Vec<u8> = (0u8..32).collect();
    let addr = spawn_range_server(full.clone()).await;
    let client = crate::Client::builder()
        .refresh_token("dummy")
        .build()
        .unwrap();

    let dir = std::env::temp_dir().join(format!("pikpak-resume-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("f.part");
    std::fs::write(&part, &full[..10]).unwrap();

    let url = format!("http://{addr}/f");
    let res = client
        .attempt(&url, full.len() as u64, &part, &|_, _| {})
        .await;
    assert!(res.is_ok(), "resume attempt should succeed");

    let got = std::fs::read(&part).unwrap();
    assert_eq!(got, full, "resumed file must equal the full content");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_unreachable_cdn_is_a_network_failure_without_the_signed_url() {
    // Bind then drop a listener, so the port is known to refuse connections.
    let addr = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let err = reqwest::Client::new()
        .get(format!("http://{addr}/f?sig=SECRET"))
        .send()
        .await
        .unwrap_err();

    let (super::Attempt::Retry(error) | super::Attempt::Fail(error)) = super::classify(err);
    assert!(
        matches!(error, crate::Error::Http(_)),
        "a refused connection is a network failure, not a local disk one"
    );
    assert!(!format!("{error:#}").contains("SECRET"), "{error:#}");
}

#[tokio::test]
async fn download_attempt_restarts_on_416() {
    // Remote file is 4 bytes; the local part holds 6 stale bytes, so the
    // server answers 416. total is claimed larger than the part so we hit
    // the restart branch rather than the "already complete" shortcut.
    let full: Vec<u8> = vec![1, 2, 3, 4];
    let addr = spawn_range_server(full.clone()).await;
    let client = crate::Client::builder()
        .refresh_token("dummy")
        .build()
        .unwrap();

    let dir = std::env::temp_dir().join(format!("pikpak-416-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("f.part");
    std::fs::write(&part, [9u8; 6]).unwrap();

    let url = format!("http://{addr}/f");
    let res = client.attempt(&url, 16, &part, &|_, _| {}).await;
    assert!(
        matches!(res, Err(super::Attempt::Retry(_))),
        "416 with a shorter remote file must return a retryable restart"
    );
    // The stale part must be truncated so the next attempt starts clean and
    // never streams the 416 error body into the output file.
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Server that writes `body` in `chunk`-sized pieces spaced by `gap`, and
/// either closes normally or — when `stall_at` is `Some(n)` — goes silent
/// once `n` bytes have been sent.
async fn spawn_paced_server(
    body: Vec<u8>,
    chunk: usize,
    gap: std::time::Duration,
    stall_at: Option<usize>,
) -> std::net::SocketAddr {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let _ = read_request(&mut sock).await;
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = sock.write_all(hdr.as_bytes()).await;
        let mut sent = 0;
        while sent < body.len() {
            if stall_at == Some(sent) {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                return;
            }
            let end = (sent + chunk).min(body.len());
            if sock.write_all(&body[sent..end]).await.is_err() {
                return;
            }
            let _ = sock.flush().await;
            sent = end;
            tokio::time::sleep(gap).await;
        }
    });
    addr
}

#[tokio::test]
async fn download_survives_transfer_longer_than_timeout() {
    // 64 bytes at 8 bytes / 60ms takes ~480ms — far longer than the 100ms
    // timeout configured below. With a total deadline the body is cut off
    // mid-transfer; a per-read stall timeout must let it finish.
    let body: Vec<u8> = (0u8..64).collect();
    let addr =
        spawn_paced_server(body.clone(), 8, std::time::Duration::from_millis(60), None).await;
    let client = crate::Client::builder()
        .refresh_token("dummy")
        .timeout(std::time::Duration::from_millis(100))
        .build()
        .unwrap();

    let dir = std::env::temp_dir().join(format!("pikpak-paced-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("f.part");
    let url = format!("http://{addr}/f");

    let res = client.attempt(&url, 64, &part, &|_, _| {}).await;
    assert!(
        res.is_ok(),
        "a slow but progressing download must not be killed by the timeout"
    );
    assert_eq!(std::fs::read(&part).unwrap(), body);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_aborts_when_stream_stalls() {
    // Headers plus the first 8 bytes arrive, then the server goes silent.
    // The stall timeout must still fire so a hung transfer is retried.
    let body: Vec<u8> = (0u8..64).collect();
    let addr = spawn_paced_server(body, 8, std::time::Duration::from_millis(10), Some(8)).await;
    let client = crate::Client::builder()
        .refresh_token("dummy")
        .timeout(std::time::Duration::from_millis(100))
        .build()
        .unwrap();

    let dir = std::env::temp_dir().join(format!("pikpak-stall-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("f.part");
    let url = format!("http://{addr}/f");

    let res = client.attempt(&url, 64, &part, &|_, _| {}).await;
    assert!(
        matches!(res, Err(super::Attempt::Retry(_))),
        "a stalled stream must abort with a retryable error"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_range_start_reads_offsets() {
    use super::parse_range_start;
    let hv = |s: &str| reqwest::header::HeaderValue::from_str(s).unwrap();
    assert_eq!(parse_range_start(&hv("bytes 10-19/100")), Some(10));
    assert_eq!(parse_range_start(&hv("Bytes 10-19/100")), Some(10));
    assert_eq!(parse_range_start(&hv("BYTES 10-19/100")), Some(10));
    assert_eq!(parse_range_start(&hv("bytes=0-19/100")), Some(0));
    assert_eq!(parse_range_start(&hv("  Bytes 7-7/8  ")), Some(7));
    // 416-style and malformed values yield no usable offset.
    assert_eq!(parse_range_start(&hv("bytes */100")), None);
    assert_eq!(parse_range_start(&hv("nonsense")), None);
}

#[test]
fn only_a_stale_link_is_treated_as_an_expired_link() {
    use reqwest::StatusCode;
    assert!(super::is_expired_link_status(StatusCode::UNAUTHORIZED));
    assert!(super::is_expired_link_status(StatusCode::FORBIDDEN));
    assert!(super::is_expired_link_status(StatusCode::GONE));
    assert!(!super::is_expired_link_status(StatusCode::NOT_FOUND));
    assert!(!super::is_expired_link_status(
        StatusCode::TOO_MANY_REQUESTS
    ));
    assert!(!super::is_expired_link_status(
        StatusCode::INTERNAL_SERVER_ERROR
    ));
    assert!(!super::is_expired_link_status(StatusCode::OK));
}

/// Server that answers every request with a fixed raw response.
async fn spawn_raw_server(response: Vec<u8>) -> std::net::SocketAddr {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let response = response.clone();
            tokio::spawn(async move {
                let _ = read_request(&mut sock).await;
                let _ = sock.write_all(&response).await;
                let _ = sock.flush().await;
            });
        }
    });
    addr
}

fn test_client() -> crate::Client {
    crate::Client::builder()
        .refresh_token("dummy")
        .build()
        .unwrap()
}

#[tokio::test]
async fn download_attempt_rejects_oversized_body() {
    let addr = spawn_raw_server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef".to_vec(),
    )
    .await;
    let dir = std::env::temp_dir().join(format!("pikpak-long-body-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let result = test_client()
        .attempt(
            &format!("http://{addr}/f"),
            4,
            &dir.join("f.part"),
            &|_, _| {},
        )
        .await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(matches!(
        result,
        Err(super::Attempt::Fail(crate::Error::Protocol(_)))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_partial_removal_preserves_the_old_identity() {
    let dir = std::env::temp_dir().join(format!("pikpak-part-removal-{}", std::process::id()));
    let part = dir.join("f.part");
    // A nonempty directory cannot be removed with remove_file, regardless of
    // whether the tests run as root. Its nonzero size enters the stale-part path.
    std::fs::create_dir_all(&part).unwrap();
    std::fs::write(part.join("old"), b"old content").unwrap();
    let part_len = super::part_size(&part).await;
    let old = br#"{"id":"old","size":6,"modified_time":null}"#;
    let meta = super::identity_path(&part);
    std::fs::write(&meta, old).unwrap();
    let expected = super::PartIdentity::of(&file_info("new", "f", 6), 6);

    let result = super::prepare_part(&part, &expected).await;
    let recorded = std::fs::read(&meta).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(part_len > 0, "the stale-part path needs a nonempty part");
    assert!(matches!(result, Err(crate::Error::Io { .. })));
    assert_eq!(recorded, old, "never label old bytes with a new identity");
}

#[tokio::test]
async fn download_attempt_restarts_when_part_is_oversized() {
    // Remote file is 4 bytes but the stale part holds 6. The 416 handler
    // must discard the part rather than declare the download complete:
    // returning Ok here would rename a 6-byte file over a 4-byte original.
    let full: Vec<u8> = vec![1, 2, 3, 4];
    let addr = spawn_range_server(full.clone()).await;
    let client = test_client();

    let dir = std::env::temp_dir().join(format!("pikpak-oversize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("f.part");
    std::fs::write(&part, [9u8; 6]).unwrap();

    let url = format!("http://{addr}/f");
    let res = client.attempt(&url, 4, &part, &|_, _| {}).await;
    assert!(
        matches!(res, Err(super::Attempt::Retry(_))),
        "an oversized stale part must restart, not be treated as complete"
    );
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_attempt_rejects_short_body() {
    // The server closes cleanly after 16 bytes with no framing that could
    // reveal the truncation (no Content-Length, no chunked encoding), while
    // the API reported 64 bytes. This must not be finalized.
    let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(&[1u8; 16]);
    let addr = spawn_raw_server(response).await;
    let client = test_client();

    let dir = std::env::temp_dir().join(format!("pikpak-short-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("f.part");

    let url = format!("http://{addr}/f");
    let res = client.attempt(&url, 64, &part, &|_, _| {}).await;
    assert!(
        matches!(res, Err(super::Attempt::Retry(_))),
        "a body shorter than the reported size must not count as complete"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_attempt_rejects_offset_mismatch() {
    // We ask to resume at 10, but the server answers 206 starting at 0.
    // Appending would corrupt the file, so the attempt must restart.
    let mut response = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-31/32\r\nContent-Length: 32\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(&[2u8; 32]);
    let addr = spawn_raw_server(response).await;
    let client = test_client();

    let dir = std::env::temp_dir().join(format!("pikpak-offset-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("f.part");
    std::fs::write(&part, [1u8; 10]).unwrap();

    let url = format!("http://{addr}/f");
    let res = client.attempt(&url, 32, &part, &|_, _| {}).await;
    assert!(
        matches!(res, Err(super::Attempt::Retry(_))),
        "a 206 from the wrong offset must not be appended"
    );
    assert_eq!(
        std::fs::metadata(&part).unwrap().len(),
        0,
        "the mismatched part must be discarded"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mock PikPak stack on one port: auth, captcha init, the drive file
/// detail endpoint, and the content URL it hands out.
///
/// Content is served in `chunk`-sized pieces, each response ending cleanly
/// without a `Content-Length` — an interrupted transfer whose framing hides
/// nothing, which is what a flaky link looks like to the client.
async fn spawn_mock_stack(
    content: Vec<u8>,
    chunk: usize,
    content_hits: Arc<std::sync::atomic::AtomicUsize>,
) -> std::net::SocketAddr {
    use std::sync::atomic::Ordering;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let content = content.clone();
            let hits = content_hits.clone();
            tokio::spawn(async move {
                let request = read_request(&mut sock).await;
                let path = request.target.clone();

                if path.starts_with("/v1/auth/token") {
                    respond_json(&mut sock, 200, "OK", TOKEN_JSON).await;
                } else if path.starts_with("/v1/shield/captcha/init") {
                    respond_json(&mut sock, 200, "OK", r#"{"captcha_token":"captcha"}"#).await;
                } else if path.starts_with("/drive/v1/files?") {
                    // A three-level tree: root -> dir -> sub -> big.bin. That
                    // exercises queueing a nested folder, not just the top
                    // level, without ever recursing forever.
                    let parent = path
                        .split("parent_id=")
                        .nth(1)
                        .map(|rest| rest.split('&').next().unwrap_or("").to_string())
                        .unwrap_or_default();
                    let body = if parent.is_empty() {
                        r#"{"files":[{"id":"A","name":"dir","kind":"drive#folder"}]}"#.to_string()
                    } else if parent == "A" {
                        r#"{"files":[{"id":"B","name":"sub","kind":"drive#folder"}]}"#.to_string()
                    } else if parent == "B" {
                        format!(
                            r#"{{"files":[{{"id":"f1","name":"big.bin","size":"{}","kind":"drive#file"}}]}}"#,
                            content.len()
                        )
                    } else if parent == "U" {
                        // An entry whose kind this crate does not model.
                        r#"{"files":[{"id":"u1","name":"weird.link","kind":"drive#shortcut"}]}"#
                            .to_string()
                    } else {
                        r#"{"files":[]}"#.to_string()
                    };
                    let headers = [("Content-Length", body.len().to_string())];
                    respond(&mut sock, "200 OK", &headers, body.as_bytes()).await;
                } else if path.starts_with("/drive/v1/files/") {
                    let body = format!(
                        r#"{{"name":"big.bin","size":"{}","web_content_link":"http://{addr}/content/big.bin"}}"#,
                        content.len()
                    );
                    let headers = [("Content-Length", body.len().to_string())];
                    respond(&mut sock, "200 OK", &headers, body.as_bytes()).await;
                } else {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let start = request
                        .head
                        .split("\r\n")
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("range: bytes=")
                                .map(|v| v.trim().trim_end_matches('-').to_string())
                        })
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0);
                    let start = start.min(content.len());
                    let end = (start + chunk).min(content.len());
                    // Self-consistent partial response: correct Content-Range
                    // start, no Content-Length, then EOF.
                    let headers = [(
                        "Content-Range",
                        format!(
                            "bytes {start}-{}/{len}",
                            end.max(start + 1) - 1,
                            len = content.len()
                        ),
                    )];
                    respond(
                        &mut sock,
                        "206 Partial Content",
                        &headers,
                        &content[start..end],
                    )
                    .await;
                }
            });
        }
    });
    addr
}

fn file_info(id: &str, name: &str, size: u64) -> crate::FileInfo {
    crate::FileInfo {
        id: id.to_string(),
        name: name.to_string(),
        size,
        kind: crate::FileKind::File,
        parent_id: None,
        mime_type: None,
        created_time: None,
        modified_time: None,
        file_extension: None,
    }
}

/// A client whose API and auth endpoints both point at the mock stack.
fn mock_client(addr: std::net::SocketAddr) -> crate::Client {
    crate::Client::builder()
        .refresh_token("initial")
        .auth_base_url(format!("http://{addr}"))
        .api_base_url(format!("http://{addr}"))
        .build()
        .unwrap()
}

fn budget(max_stalls: u32, max_attempts: u32) -> super::RetryBudget {
    super::RetryBudget {
        max_stalls,
        max_attempts,
        attempts: 0,
        stalls: 0,
    }
}

#[test]
fn retry_budget_stops_after_consecutive_stalls() {
    let mut b = budget(3, 100);
    assert!(b.record_failure(false).is_some());
    assert!(b.record_failure(false).is_some());
    assert!(
        b.record_failure(false).is_none(),
        "the third consecutive stall exhausts a budget of three"
    );
}

#[test]
fn retry_budget_progress_resets_the_stall_count() {
    let mut b = budget(3, 100);
    assert!(b.record_failure(false).is_some());
    assert!(b.record_failure(false).is_some());
    assert!(
        b.record_failure(true).is_some(),
        "transferring bytes must clear the stall count"
    );
    assert!(b.record_failure(false).is_some());
    assert!(b.record_failure(false).is_some());
    assert!(b.record_failure(false).is_none());
}

#[test]
fn retry_budget_has_an_absolute_ceiling() {
    // Progress alone must not license infinite retries.
    let mut b = budget(3, 50);
    for _ in 0..49 {
        assert!(b.record_failure(true).is_some());
    }
    assert!(
        b.record_failure(true).is_none(),
        "the absolute attempt ceiling still applies to progressing downloads"
    );
}

#[test]
fn retry_budget_backoff_grows_with_stalls() {
    let mut b = budget(5, 100);
    let first = b.record_failure(false).unwrap();
    let second = b.record_failure(false).unwrap();
    assert!(second > first, "backoff must grow while the stream stalls");
}

#[test]
fn identity_sidecar_sits_next_to_the_partial() {
    assert_eq!(
        super::identity_path(std::path::Path::new("/tmp/x/big.bin.part")),
        std::path::PathBuf::from("/tmp/x/big.bin.part.meta")
    );
}

#[test]
fn part_identity_tracks_id_size_and_mtime() {
    let mut f = file_info("id-1", "big.bin", 10);
    f.modified_time = Some("2024-01-01T00:00:00Z".to_string());
    let identity = super::PartIdentity::of(&f, 42);
    assert_eq!(identity.id, "id-1");
    assert_eq!(identity.size, 42);
    assert_eq!(
        identity.modified_time.as_deref(),
        Some("2024-01-01T00:00:00Z")
    );
}

#[tokio::test]
async fn download_resumes_a_partial_that_belongs_to_the_remote_file() {
    // 32 of 224 bytes already on disk, delivered 64 bytes at a time from the
    // offset: three requests instead of the four a fresh download needs.
    let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let addr = spawn_mock_stack(content.clone(), 64, hits.clone()).await;
    let client = mock_client(addr);
    let dir = std::env::temp_dir().join(format!("pikpak-resume-id-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let file = file_info("f1", "big.bin", 224);
    std::fs::write(dir.join("f1.part"), &content[..32]).unwrap();
    std::fs::write(
        dir.join("f1.part.meta"),
        serde_json::to_vec(&super::PartIdentity::of(&file, 224)).unwrap(),
    )
    .unwrap();

    client
        .download_file(
            &file,
            &dir.join("big.bin"),
            &dir.join("f1.part"),
            &|_, _| {},
        )
        .await
        .expect("a matching partial must resume and finish");

    assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "resuming from 32/224 with 64-byte chunks must take three requests"
    );
    assert!(
        !dir.join("f1.part.meta").exists(),
        "the identity sidecar must be cleaned up on success"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_discards_a_partial_from_a_different_file() {
    // The stale bytes are deliberately wrong, so splicing them onto the real
    // tail would be visible in the result.
    let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let addr = spawn_mock_stack(content.clone(), 64, hits.clone()).await;
    let client = mock_client(addr);
    let dir = std::env::temp_dir().join(format!("pikpak-foreign-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("f1.part"), [0xAAu8; 32]).unwrap();
    let other = super::PartIdentity {
        id: "some-other-entry".to_string(),
        size: 999,
        modified_time: None,
    };
    std::fs::write(
        dir.join("f1.part.meta"),
        serde_json::to_vec(&other).unwrap(),
    )
    .unwrap();

    let file = file_info("f1", "big.bin", 224);
    client
        .download_file(
            &file,
            &dir.join("big.bin"),
            &dir.join("f1.part"),
            &|_, _| {},
        )
        .await
        .expect("a foreign partial must be discarded and the file re-fetched");

    assert_eq!(
        std::fs::read(dir.join("big.bin")).unwrap(),
        content,
        "the foreign partial must not be spliced onto the real tail"
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "a discarded partial means a full re-download"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_discards_a_partial_without_an_identity() {
    // Partials left by older versions have no sidecar; they cannot be shown
    // to belong to this file, so they must not be resumed.
    let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let addr = spawn_mock_stack(content.clone(), 64, hits.clone()).await;
    let client = mock_client(addr);
    let dir = std::env::temp_dir().join(format!("pikpak-nometa-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("f1.part"), [0xAAu8; 32]).unwrap();

    let file = file_info("f1", "big.bin", 224);
    client
        .download_file(
            &file,
            &dir.join("big.bin"),
            &dir.join("f1.part"),
            &|_, _| {},
        )
        .await
        .expect("a partial without an identity must be refetched");

    assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_file_retries_while_it_keeps_making_progress() {
    // 224 bytes delivered 32 at a time needs 7 attempts. A budget that
    // counted every attempt would give up after 5 even though each one
    // advanced the file; only stalls should exhaust it.
    let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let addr = spawn_mock_stack(content.clone(), 32, hits.clone()).await;

    let client = mock_client(addr);

    let dir = std::env::temp_dir().join(format!("pikpak-progress-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let file = file_info("f1", "big.bin", 224);
    client
        .download_file(
            &file,
            &dir.join("big.bin"),
            &dir.join("f1.part"),
            &|_, _| {},
        )
        .await
        .expect("a file that keeps advancing must finish");

    assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
    assert!(
        hits.load(std::sync::atomic::Ordering::SeqCst) > 5,
        "expected more attempts than the old five-attempt budget"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_file_gives_up_when_nothing_is_transferred() {
    // The server accepts the request and returns no bytes at all, so the
    // budget must expire and the error must surface.
    let content: Vec<u8> = vec![0u8; 32];
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let addr = spawn_mock_stack(content, 0, hits.clone()).await;

    let client = mock_client(addr);

    let dir = std::env::temp_dir().join(format!("pikpak-stallbudget-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let file = file_info("f1", "big.bin", 32);
    let res = client
        .download_file(
            &file,
            &dir.join("big.bin"),
            &dir.join("f1.part"),
            &|_, _| {},
        )
        .await;
    assert!(res.is_err(), "a server that never advances must fail");
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        super::MAX_STALLS as usize,
        "exactly the stall budget should be spent"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stack whose first download link is already stale and whose detail
/// endpoint hands out a fresh one on the next call.
///
/// `expired_status` is what the stale link answers with. `refresh_fails` makes
/// the second detail call fail as a rejected account token — the case where
/// refreshing the link cannot possibly help.
async fn spawn_expiring_link_server(
    content: Vec<u8>,
    expired_status: &'static str,
    refresh_fails: bool,
) -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let detail_calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = detail_calls.clone();
    let body = content.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let content = body.clone();
            let detail_calls = calls.clone();
            tokio::spawn(async move {
                let request = read_request(&mut sock).await;
                let path = request.target.clone();

                if path.starts_with("/v1/auth/token") {
                    respond_json(&mut sock, 200, "OK", TOKEN_JSON).await;
                } else if path.starts_with("/v1/shield/captcha/init") {
                    respond_json(&mut sock, 200, "OK", r#"{"captcha_token":"captcha"}"#).await;
                } else if path.starts_with("/drive/v1/files/f1") {
                    let n = detail_calls.fetch_add(1, Ordering::SeqCst);
                    if refresh_fails && n > 0 {
                        let body = r#"{"error":"invalid_grant","error_code":4126}"#;
                        respond(
                            &mut sock,
                            "400 Bad Request",
                            &[("Content-Length", body.len().to_string())],
                            body.as_bytes(),
                        )
                        .await;
                    } else {
                        let link = if n == 0 { "expired" } else { "fresh" };
                        let body = format!(
                            r#"{{"name":"big.bin","size":"{}","web_content_link":"http://{addr}/content/{link}"}}"#,
                            content.len()
                        );
                        respond(
                            &mut sock,
                            "200 OK",
                            &[("Content-Length", body.len().to_string())],
                            body.as_bytes(),
                        )
                        .await;
                    }
                } else if path.starts_with("/content/expired") {
                    let body = "stale link";
                    respond(
                        &mut sock,
                        expired_status,
                        &[("Content-Length", body.len().to_string())],
                        body.as_bytes(),
                    )
                    .await;
                } else if path.starts_with("/content/fresh") {
                    respond(
                        &mut sock,
                        "200 OK",
                        &[("Content-Length", content.len().to_string())],
                        &content,
                    )
                    .await;
                } else {
                    respond(&mut sock, "404 Not Found", &[], b"").await;
                }
            });
        }
    });
    (addr, detail_calls)
}

#[tokio::test]
async fn download_refreshes_the_link_when_the_cdn_says_it_is_gone() {
    use std::sync::atomic::Ordering;

    // A stale signed link answers 401, 403 or 410 depending on the CDN; each
    // one means the same thing: fetch a fresh link and try again.
    for expired_status in ["401 Unauthorized", "403 Forbidden", "410 Gone"] {
        let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let (addr, detail_calls) =
            spawn_expiring_link_server(content.clone(), expired_status, false).await;
        let client = mock_client(addr);
        let dir = std::env::temp_dir().join(format!(
            "pikpak-stale-link-{}-{}",
            std::process::id(),
            expired_status.split(' ').next().unwrap()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let file = file_info("f1", "big.bin", content.len() as u64);
        client
            .download_file(
                &file,
                &dir.join("big.bin"),
                &dir.join("f1.part"),
                &|_, _| {},
            )
            .await
            .unwrap_or_else(|e| panic!("{expired_status} must be retried: {e:#}"));

        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
        assert!(
            detail_calls.load(Ordering::SeqCst) >= 2,
            "{expired_status}: the link must be refreshed before retrying"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
async fn download_surfaces_a_rejected_token_instead_of_spending_the_budget() {
    use std::sync::atomic::Ordering;

    // The link looks stale, but refreshing it fails because the account token
    // was rejected. No retry can fix that, so the download must report the auth
    // failure rather than exhaust the budget and blame the CDN's status.
    let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
    let (addr, detail_calls) =
        spawn_expiring_link_server(content.clone(), "401 Unauthorized", true).await;
    let client = mock_client(addr);
    let dir = std::env::temp_dir().join(format!("pikpak-dead-token-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let file = file_info("f1", "big.bin", content.len() as u64);
    let err = client
        .download_file(
            &file,
            &dir.join("big.bin"),
            &dir.join("f1.part"),
            &|_, _| {},
        )
        .await
        .expect_err("a rejected token must fail the download");

    let text = format!("{err:#}");
    assert!(text.contains("invalid_grant"), "{text}");
    assert!(matches!(err, crate::Error::Auth(_)), "{text}");
    assert_eq!(
        detail_calls.load(Ordering::SeqCst),
        2,
        "one refresh attempt, then give up — the retry budget must stay untouched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
