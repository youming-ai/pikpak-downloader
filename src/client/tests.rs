//! Tests for the API client, against a scripted drive API mock.

use super::*;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// Build a listing body; `next` adds a pagination token.
fn page_json(entries: &[(&str, &str, &str)], next: Option<&str>) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|(id, name, kind)| format!(r#"{{"id":"{id}","name":"{name}","kind":"{kind}"}}"#))
        .collect();
    match next {
        Some(token) => format!(
            r#"{{"files":[{}],"next_page_token":"{token}"}}"#,
            items.join(",")
        ),
        None => format!(r#"{{"files":[{}]}}"#, items.join(",")),
    }
}

#[derive(Default)]
struct MockState {
    /// Scripted drive responses, consumed in order; the last one repeats.
    drive: VecDeque<(u16, String)>,
    /// Every request target (path + query), in order.
    targets: Vec<String>,
    auth_calls: usize,
    captcha_calls: usize,
    captcha_replies: VecDeque<(u16, String)>,
}

/// Scripted stand-in for the PikPak stack: auth, captcha init and drive.
struct Mock {
    addr: SocketAddr,
    state: Arc<Mutex<MockState>>,
}

impl Mock {
    async fn start(drive: Vec<(u16, String)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(MockState {
            drive: drive.into_iter().collect(),
            ..MockState::default()
        }));
        tokio::spawn({
            let state = state.clone();
            async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        return;
                    };
                    let state = state.clone();
                    tokio::spawn(async move {
                        let request = crate::test_http::read_request(&mut sock).await;

                        // The std mutex is released before any await.
                        let (status, payload) = {
                            let mut st = state.lock().unwrap();
                            st.targets.push(request.target.clone());
                            if request.target.starts_with("/v1/auth/token") {
                                st.auth_calls += 1;
                                (
                                    200,
                                    crate::test_http::TOKEN_JSON.replace(
                                        "\"access\"",
                                        &format!("\"access-{}\"", st.auth_calls),
                                    ),
                                )
                            } else if request.target.starts_with("/v1/shield/captcha/init") {
                                st.captcha_calls += 1;
                                let token = format!("captcha-{}", st.captcha_calls);
                                st.captcha_replies.pop_front().unwrap_or_else(|| {
                                    (200, format!(r#"{{"captcha_token":"{token}"}}"#))
                                })
                            } else {
                                match st.drive.len() {
                                    0 => (200, "{}".to_string()),
                                    1 => st.drive[0].clone(),
                                    _ => st.drive.pop_front().unwrap(),
                                }
                            }
                        };

                        let reason = if status == 200 { "OK" } else { "Error" };
                        crate::test_http::respond_json(&mut sock, status, reason, &payload).await;
                    });
                }
            }
        });
        Self { addr, state }
    }

    fn client(&self) -> Client {
        Client::builder()
            .refresh_token("initial-token")
            .auth_base_url(format!("http://{}", self.addr))
            .api_base_url(format!("http://{}", self.addr))
            .build()
            .unwrap()
    }

    fn targets(&self) -> Vec<String> {
        self.state.lock().unwrap().targets.clone()
    }

    fn drive_targets(&self) -> Vec<String> {
        self.targets()
            .into_iter()
            .filter(|t| t.starts_with("/drive/"))
            .collect()
    }

    fn auth_calls(&self) -> usize {
        self.state.lock().unwrap().auth_calls
    }

    fn captcha_calls(&self) -> usize {
        self.state.lock().unwrap().captcha_calls
    }
}

#[tokio::test]
async fn captcha_401_refreshes_a_previously_cached_access_token() {
    let mock = Mock::start(vec![
        (200, r#"{"quota":{"limit":"100","usage":"0"}}"#.into()),
        (200, page_json(&[], None)),
    ])
    .await;
    let client = mock.client();
    client.quota().await.unwrap();
    mock.state
        .lock()
        .unwrap()
        .captcha_replies
        .push_back((401, "expired".into()));

    assert!(client.list_folder("").await.unwrap().is_empty());
    assert_eq!(mock.auth_calls(), 2);
    assert_eq!(mock.captcha_calls(), 3);
    assert_eq!(mock.drive_targets().len(), 2);
}

#[tokio::test]
async fn captcha_401_retries_authentication_only_once() {
    let mock = Mock::start(vec![]).await;
    mock.state.lock().unwrap().captcha_replies =
        vec![(401, "expired".into()), (401, "still expired".into())].into();
    let result = mock.client().list_folder("").await;
    assert!(matches!(result, Err(Error::Api { status: 401, .. })));
    assert_eq!(mock.auth_calls(), 2);
    assert_eq!(mock.captcha_calls(), 2);
    assert!(mock.drive_targets().is_empty());
}

#[tokio::test]
async fn a_rejected_access_token_is_refreshed_and_retried_once() {
    let mock = Mock::start(vec![
        (401, r#"{"error":"token expired"}"#.to_string()),
        (200, page_json(&[], None)),
    ])
    .await;

    let files = mock.client().list_folder("").await.unwrap();

    assert!(files.is_empty());
    assert_eq!(
        mock.auth_calls(),
        2,
        "the 401 must invalidate the cached token so the retry authenticates again"
    );
    assert_eq!(
        mock.drive_targets().len(),
        2,
        "the request must be retried once"
    );
}

#[tokio::test]
async fn a_second_consecutive_401_surfaces_as_an_error() {
    // The auth path gets exactly one retry. If the fresh token is also
    // rejected the request must fail instead of looping. The trailing 500
    // keeps a regression that retried forever from hanging the suite: it
    // would fail out through the 5xx budget with a different status.
    let mock = Mock::start(vec![
        (401, r#"{"error":"token expired"}"#.to_string()),
        (401, r#"{"error":"still expired"}"#.to_string()),
        (500, "unavailable".to_string()),
    ])
    .await;

    match mock.client().list_folder("").await {
        Err(Error::Api { status, .. }) => assert_eq!(status, 401),
        other => panic!("expected an Api error, got {other:?}"),
    }
    assert_eq!(
        mock.drive_targets().len(),
        2,
        "exactly one auth retry, then an error"
    );
    assert_eq!(
        mock.auth_calls(),
        2,
        "the second 401 must not trigger a third authentication"
    );
}

#[tokio::test]
async fn captcha_error_code_9_refreshes_the_captcha_and_retries() {
    let mock = Mock::start(vec![
        (
            400,
            r#"{"error_code":9,"error":"captcha token expired"}"#.to_string(),
        ),
        (200, page_json(&[], None)),
    ])
    .await;

    mock.client().list_folder("").await.unwrap();

    assert_eq!(
        mock.captcha_calls(),
        2,
        "error_code 9 must force a new captcha token"
    );
    assert_eq!(
        mock.auth_calls(),
        1,
        "the access token was still valid and must not be refreshed"
    );
    assert_eq!(mock.drive_targets().len(), 2);
}

#[tokio::test]
async fn a_server_error_is_retried_with_backoff() {
    let mock = Mock::start(vec![
        (503, "temporarily unavailable".to_string()),
        (200, page_json(&[], None)),
    ])
    .await;

    mock.client().list_folder("").await.unwrap();

    assert_eq!(mock.drive_targets().len(), 2, "5xx must be retried");
}

#[tokio::test]
async fn a_non_retryable_status_surfaces_as_an_api_error() {
    let mock = Mock::start(vec![(404, r#"{"error":"not found"}"#.to_string())]).await;

    match mock.client().get_download_url("missing").await {
        Err(Error::Api { status, .. }) => assert_eq!(status, 404),
        other => panic!("expected an Api error, got {other:?}"),
    }
    assert_eq!(mock.drive_targets().len(), 1, "404 must not be retried");
}

#[tokio::test]
async fn the_builder_wires_the_rotation_hook() {
    let mock = Mock::start(vec![]).await;
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = calls.clone();
    let client = Client::builder()
        .refresh_token("initial-token")
        .auth_base_url(format!("http://{}", mock.addr))
        .on_refresh_token(move |token| recorder.lock().unwrap().push(token.to_string()))
        .build()
        .unwrap();

    client.tokens.access_token().await.unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["rotated"]);
}

#[tokio::test]
async fn listing_follows_next_page_tokens() {
    let mock = Mock::start(vec![
        (200, page_json(&[("1", "a.bin", "drive#file")], Some("t1"))),
        (200, page_json(&[("2", "b.bin", "drive#file")], None)),
    ])
    .await;

    let files = mock.client().list_folder("").await.unwrap();

    assert_eq!(
        files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        vec!["a.bin", "b.bin"]
    );
    let drive = mock.drive_targets();
    assert_eq!(drive.len(), 2);
    assert!(
        drive[1].contains("page_token=t1"),
        "the second page must carry the token: {}",
        drive[1]
    );
}

#[tokio::test]
async fn a_null_file_list_is_an_empty_folder() {
    let mock = Mock::start(vec![(200, r#"{"files":null}"#.to_string())]).await;
    assert!(mock.client().list_folder("").await.unwrap().is_empty());
}

#[test]
fn retry_after_is_read_in_seconds_and_capped() {
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    let with = |value: &str| {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        super::retry_after(&headers)
    };
    assert_eq!(with("3"), Duration::from_secs(3));
    assert_eq!(with("86400"), super::MAX_RETRY_AFTER);
    // The date form is not parsed: fall back to our own backoff.
    assert_eq!(with("Wed, 21 Oct 2026 07:28:00 GMT"), Duration::ZERO);
    assert_eq!(super::retry_after(&HeaderMap::new()), Duration::ZERO);
}

#[tokio::test]
async fn listing_terminates_when_the_server_repeats_a_page_token() {
    // A server that keeps returning the same token must not spin forever.
    // The final scripted 500 keeps a regression from hanging the suite: the
    // loop would then fail out through the normal retry budget.
    let repeated = page_json(&[], Some("same"));
    let mock = Mock::start(vec![
        (200, repeated.clone()),
        (200, repeated.clone()),
        (200, repeated),
        (500, "still paginating".to_string()),
    ])
    .await;

    // Ended, and as a failure: a quiet stop would pass a partial listing
    // off as the whole folder.
    let err = mock.client().list_folder("").await.unwrap_err();
    assert!(matches!(err, Error::Protocol(_)), "{err:?}");

    assert_eq!(
        mock.drive_targets().len(),
        2,
        "a repeated page token must end the loop after the second request"
    );
}

#[tokio::test]
async fn listing_terminates_on_a_two_token_cycle() {
    // A -> B -> A: comparing only with the previous token would ping-pong
    // forever. The trailing 500 keeps a regression failing rather than
    // hanging the suite.
    let mock = Mock::start(vec![
        (200, page_json(&[], Some("A"))),
        (200, page_json(&[], Some("B"))),
        (200, page_json(&[], Some("A"))),
        (500, "still paginating".to_string()),
    ])
    .await;

    let err = mock.client().list_folder("").await.unwrap_err();
    assert!(matches!(err, Error::Protocol(_)), "{err:?}");

    let drive = mock.drive_targets();
    assert_eq!(
        drive.len(),
        3,
        "the cycle must end once A has been followed already: {drive:?}"
    );
    assert!(drive[1].contains("page_token=A"), "{}", drive[1]);
    assert!(drive[2].contains("page_token=B"), "{}", drive[2]);
}

#[tokio::test]
async fn resolve_path_walks_segments_and_names_the_missing_one() {
    let mock = Mock::start(vec![
        (200, page_json(&[("A", "A", "drive#folder")], None)),
        (200, page_json(&[("B", "B", "drive#folder")], None)),
        (200, page_json(&[("A", "A", "drive#folder")], None)),
        (200, page_json(&[], None)),
    ])
    .await;
    let client = mock.client();

    assert_eq!(client.resolve_path_info("/A/B").await.unwrap().id, "B");

    match client.resolve_path_info("/A/Nope").await {
        Err(Error::NotFound { segment, .. }) => assert_eq!(segment, "Nope"),
        other => panic!("expected NotFound, got {other:?}"),
    }

    let drive = mock.drive_targets();
    assert!(
        drive[1].contains("parent_id=A"),
        "the second hop must list inside A: {}",
        drive[1]
    );
    assert!(
        drive[3].contains("parent_id=A"),
        "the failing lookup must be inside A: {}",
        drive[3]
    );
}

#[tokio::test]
async fn resolve_path_info_prefers_a_folder_for_middle_segments() {
    // The parent holds both a file and a folder named "X": a middle segment
    // must resolve to the folder, not the same-named file.
    let mock = Mock::start(vec![
        (
            200,
            page_json(
                &[
                    ("file-X", "X", "drive#file"),
                    ("folder-X", "X", "drive#folder"),
                ],
                None,
            ),
        ),
        (200, page_json(&[("f1", "movie.mp4", "drive#file")], None)),
    ])
    .await;

    let info = mock
        .client()
        .resolve_path_info("/X/movie.mp4")
        .await
        .unwrap();

    assert_eq!(info.id, "f1");
    assert_eq!(info.name, "movie.mp4");
    assert!(
        mock.drive_targets()[1].contains("parent_id=folder-X"),
        "middle segment must resolve to the folder"
    );
}

#[tokio::test]
async fn resolve_path_info_prefers_a_folder_for_the_last_segment_too() {
    // The same collision at the final segment: the folder wins there as well,
    // so the order a listing happens to arrive in cannot change what a path
    // resolves to (which is what `ls` and `download` both key off).
    let mock = Mock::start(vec![(
        200,
        page_json(
            &[
                ("file-X", "X", "drive#file"),
                ("folder-X", "X", "drive#folder"),
            ],
            None,
        ),
    )])
    .await;

    let info = mock.client().resolve_path_info("X").await.unwrap();

    assert_eq!(info.id, "folder-X");
    assert!(info.kind.is_folder());
}

#[tokio::test]
async fn resolve_path_info_still_resolves_a_lone_file() {
    let mock = Mock::start(vec![(
        200,
        page_json(&[("f1", "movie.mp4", "drive#file")], None),
    )])
    .await;

    let info = mock.client().resolve_path_info("movie.mp4").await.unwrap();

    assert_eq!(info.id, "f1");
    assert!(info.kind.is_file());
}

#[tokio::test]
async fn a_rejected_token_from_a_drive_call_explains_itself() {
    // The token exchange is not the only place a dead token surfaces: a drive
    // call sees the same rejection, and must give the same actionable message
    // rather than relaying the server's raw body.
    let body = r#"{"error":"invalid_grant","error_code":4126}"#;
    let mock = Mock::start(vec![(400, body.to_string())]).await;

    let err = mock.client().list_folder("").await.unwrap_err();

    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("single-use"), "{text}");
    assert!(text.contains(".env"), "{text}");
}

#[tokio::test]
async fn a_rejected_token_from_captcha_init_explains_itself() {
    let body = r#"{"error":"invalid_grant","error_code":4126}"#;
    let mock = Mock::start(vec![]).await;
    mock.state.lock().unwrap().captcha_replies = vec![(400, body.to_string())].into();

    let err = mock.client().list_folder("").await.unwrap_err();

    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(err.to_string().contains("single-use"), "{err}");
    assert!(mock.drive_targets().is_empty());
}

#[test]
fn an_absent_size_stays_absent_while_an_explicit_zero_is_a_size() {
    // The download path substitutes the directory listing's size only when the
    // detail response left the field out. A file the API reports as zero bytes
    // must not be mistaken for one it said nothing about, or a genuinely empty
    // file is judged against a stale listing size and never completes.
    let absent: super::FileDetailResponse =
        serde_json::from_str(r#"{"web_content_link":"http://x"}"#).unwrap();
    assert_eq!(absent.size, None);

    let blank: super::FileDetailResponse =
        serde_json::from_str(r#"{"size":"","web_content_link":"http://x"}"#).unwrap();
    assert_eq!(blank.size, None);

    let empty: super::FileDetailResponse =
        serde_json::from_str(r#"{"size":"0","web_content_link":"http://x"}"#).unwrap();
    assert_eq!(empty.size, Some(0));

    let numbered: super::FileDetailResponse =
        serde_json::from_str(r#"{"size":64,"web_content_link":"http://x"}"#).unwrap();
    assert_eq!(numbered.size, Some(64));
}

#[tokio::test]
async fn resolve_path_info_rejects_an_empty_path() {
    let mock = Mock::start(vec![]).await;

    match mock.client().resolve_path_info("/").await {
        Err(Error::InvalidPath(_)) => {}
        other => panic!("expected InvalidPath, got {other:?}"),
    }
}

#[tokio::test]
async fn quota_reads_the_about_endpoint() {
    let mock = Mock::start(vec![(
        200,
        r#"{"quota":{"limit":"1000","usage":"250"}}"#.to_string(),
    )])
    .await;

    let quota = mock.client().quota().await.unwrap();

    assert_eq!(quota.total, 1000);
    assert_eq!(quota.used, 250);
    assert_eq!(quota.free(), 750);
}

#[test]
fn retryable_statuses_are_server_errors_and_429() {
    assert!(is_retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
    assert!(is_retryable_status(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    ));
    assert!(is_retryable_status(reqwest::StatusCode::BAD_GATEWAY));
    assert!(!is_retryable_status(reqwest::StatusCode::NOT_FOUND));
    assert!(!is_retryable_status(reqwest::StatusCode::UNAUTHORIZED));
    assert!(!is_retryable_status(reqwest::StatusCode::BAD_REQUEST));
}

#[test]
fn drive_root_paths() {
    assert!(is_drive_root(""));
    assert!(is_drive_root("/"));
    assert!(is_drive_root("///"));
    assert!(!is_drive_root("/a"));
    assert!(!is_drive_root("a"));
}

#[test]
fn exponential_backoff_grows_from_its_base_and_caps() {
    let base = Duration::from_millis(500);
    let cap = Duration::from_secs(15);
    assert_eq!(exponential_backoff(base, cap, 0), base);
    assert_eq!(exponential_backoff(base, cap, 1), Duration::from_secs(1));
    assert_eq!(
        exponential_backoff(base, cap, 30),
        cap,
        "the cap applies far past the doubling range"
    );
}

#[test]
fn jittered_backoff_stays_in_bounds_and_varies() {
    for ms in [1u64, 2, 10, 500, 1000, 15_000] {
        let base = Duration::from_millis(ms);
        for _ in 0..50 {
            let jittered = jittered_backoff(base);
            assert!(
                jittered >= base / 2 && jittered <= base,
                "{jittered:?} outside [{:?}, {base:?}]",
                base / 2
            );
        }
    }
    // A zero base has no span to spread; it must stay zero.
    assert_eq!(jittered_backoff(Duration::ZERO), Duration::ZERO);

    // It must actually spread: a constant delay would leave workers that
    // failed together retrying together, which is the whole point.
    let base = Duration::from_millis(1000);
    let distinct: HashSet<u128> = (0..100)
        .map(|_| jittered_backoff(base).as_nanos())
        .collect();
    assert!(
        distinct.len() > 1,
        "jitter produced no variation: {distinct:?}"
    );
}
