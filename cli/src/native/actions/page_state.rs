use super::*;

pub(super) async fn handle_cookies_get(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            let cookies_list = wb.get_cookies().await?;
            return Ok(json!({ "cookies": cookies_list }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let urls = cmd.get("urls").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    });

    let cookies_list = cookies::get_cookies(&mgr.client, &session_id, urls).await?;
    Ok(json!({ "cookies": cookies_list }))
}

pub(super) async fn handle_cookies_set(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let url = mgr.get_url().await.ok();

    let cookie_values = if let Some(arr) = cmd.get("cookies").and_then(|v| v.as_array()) {
        arr.clone()
    } else {
        let mut cookie = serde_json::Map::new();
        for key in &[
            "name", "value", "domain", "path", "expires", "httpOnly", "secure", "sameSite", "url",
        ] {
            if let Some(v) = cmd.get(*key) {
                if !v.is_null() {
                    cookie.insert(key.to_string(), v.clone());
                }
            }
        }
        vec![Value::Object(cookie)]
    };

    cookies::set_cookies(&mgr.client, &session_id, cookie_values, url.as_deref()).await?;
    Ok(json!({ "set": true }))
}

pub(super) async fn handle_cookies_clear(state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    cookies::clear_cookies(&mgr.client, &session_id).await?;
    Ok(json!({ "cleared": true }))
}

pub(super) async fn handle_storage_get(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let storage_type = cmd.get("type").and_then(|v| v.as_str()).unwrap_or("local");
    let key = cmd.get("key").and_then(|v| v.as_str());
    storage::storage_get(&mgr.client, &session_id, storage_type, key).await
}

pub(super) async fn handle_storage_set(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let storage_type = cmd.get("type").and_then(|v| v.as_str()).unwrap_or("local");
    let key = cmd
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'key' parameter")?;
    let value = cmd
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'value' parameter")?;
    storage::storage_set(&mgr.client, &session_id, storage_type, key, value).await?;
    Ok(json!({ "set": true }))
}

pub(super) async fn handle_storage_clear(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let storage_type = cmd.get("type").and_then(|v| v.as_str()).unwrap_or("local");
    storage::storage_clear(&mgr.client, &session_id, storage_type).await?;
    Ok(json!({ "cleared": true }))
}

pub(super) async fn handle_setcontent(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let html = cmd
        .get("html")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'html' parameter")?;
    network::set_content(&mgr.client, &session_id, html).await?;
    Ok(json!({ "set": true }))
}

pub(super) async fn handle_headers(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let headers_value = cmd.get("headers").ok_or("Missing 'headers' parameter")?;

    let headers: HashMap<String, String> = headers_value
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();

    network::set_extra_headers(&mgr.client, &session_id, &headers).await?;
    // A fresh target already has no extra headers, so an empty map clears the
    // inherited setup instead of forcing later tabs through deferred loading.
    state.session_setup.extra_headers = (!headers.is_empty()).then_some(headers);
    Ok(json!({ "set": true }))
}

pub(super) async fn handle_offline(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let offline = cmd.get("offline").and_then(|v| v.as_bool()).unwrap_or(true);
    network::set_offline(&mgr.client, &session_id, offline).await?;
    // Online is the default for a fresh target and needs no replay.
    state.session_setup.offline = offline.then_some(true);
    Ok(json!({ "offline": offline }))
}

pub(super) async fn handle_console(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let clear = cmd.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
    if clear {
        state.event_tracker.clear_console();
        Ok(json!({ "cleared": true }))
    } else {
        let result = state.event_tracker.get_console_json();
        Ok(result)
    }
}

/// Reads captured page errors or clears them when requested.
pub(super) async fn handle_errors(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let clear = cmd.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
    if clear {
        state.event_tracker.clear_errors();
        Ok(json!({ "cleared": true }))
    } else {
        Ok(state.event_tracker.get_errors_json())
    }
}

pub(super) async fn handle_session_info(state: &DaemonState) -> Result<Value, String> {
    Ok(json!({
        "session": state.session_id,
        "namespace": env::var("FROB_NAMESPACE").ok(),
        "socketDir": get_socket_dir().to_string_lossy(),
        "backgroundPid": std::process::id(),
        "browserLaunched": state.browser.is_some(),
        "pageCount": state.browser.as_ref().map(|mgr| mgr.page_count()).unwrap_or(0),
        "engine": state.engine,
        "launchHash": state.launch_identity,
        "compatibilityStatus": "current",
        "effectiveLaunch": {
            "browserLaunched": state.browser.is_some(),
            "engine": state.engine,
            "launchHash": state.launch_identity,
        },
        "restoreKey": state.session_name,
        "restoreStatus": state.restore_status,
        "restoreStatusDetail": state.restore_status_detail,
        "restoreSkippedOrigins": state.restore_skipped_origins,
        "restoreLoadedPath": state.restore_loaded_path,
        "restoreValidationPending": state.restore_validation_pending,
        "restoreSave": state.restore_save,
        "saveStatus": state.restore_save_status,
        "restoreSavedPath": state.restore_saved_path,
        "restoreCheckUrl": state.restore_check_url,
        "restoreCheckText": state.restore_check_text,
        "restoreCheckFn": state.restore_check_fn,
    }))
}

pub(super) async fn handle_state_save(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let path = cmd.get("path").and_then(|v| v.as_str());

    let saved_path = state::save_state(
        &mgr.client,
        &session_id,
        path,
        state.session_name.as_deref(),
        &state.session_id,
        mgr.visited_origins(),
    )
    .await?;

    Ok(json!({ "saved": true, "path": saved_path }))
}

pub(super) async fn handle_state_load(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let path = cmd
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' parameter")?;

    ensure_state_replay_supported_by_active_domain_filter(state, "state load").await?;
    let skipped = state::load_state(&mgr.client, &session_id, path).await?;
    mark_explicit_storage_state_loaded(state, path);
    Ok(json!({ "loaded": true, "path": path, "skippedOrigins": skipped }))
}

// ---------------------------------------------------------------------------
// Phase 6 handlers
// ---------------------------------------------------------------------------
