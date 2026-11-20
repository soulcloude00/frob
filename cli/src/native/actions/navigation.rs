use super::*;

pub(super) async fn handle_navigate(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let url = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' parameter")?;

    {
        let df = state.domain_filter.read().await;
        if let Some(ref filter) = *df {
            filter.check_url(url)?;
        }
    }

    // WebDriver backend path
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            state.webmcp.clear_invocations();
            state.ref_map.invalidate_all_documents();
            wb.navigate(url).await?;
            let new_url = wb.get_url().await.unwrap_or_else(|_| url.to_string());
            let title = wb.get_title().await.unwrap_or_default();
            return Ok(json!({ "url": new_url, "title": title }));
        }
    }

    let mgr = state.browser.as_mut().ok_or("Browser not launched")?;

    let wait_until = cmd
        .get("waitUntil")
        .and_then(|v| v.as_str())
        .map(WaitUntil::from_str)
        .unwrap_or(WaitUntil::Load);

    // If --headers was passed, store them keyed by origin and enable Fetch
    // interception. The background fetch_handler_task (started on launch)
    // injects them into matching requests in real-time.
    let scoped_headers = cmd
        .get("headers")
        .and_then(|v| v.as_object())
        .filter(|m| !m.is_empty());

    if let Some(headers_map) = scoped_headers {
        if let Some(origin) = url::Url::parse(url)
            .ok()
            .map(|u| u.origin().ascii_serialization())
        {
            let headers: HashMap<String, String> = headers_map
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();

            let first_origin_header = {
                let mut map = state.origin_headers.write().await;
                let first = map.is_empty();
                map.insert(origin, headers);
                first
            };

            // Enable Fetch interception the first time --headers is used.
            // Fetch.enable is idempotent — safe even if domain filter or
            // routes already enabled it. Wildcard ensures we see all requests.
            if first_origin_header {
                let session_id = mgr.active_session_id()?.to_string();
                let has_proxy_creds = state.proxy_credentials.read().await.is_some();
                let mut params = json!({ "patterns": [{ "urlPattern": "*" }] });
                if has_proxy_creds {
                    params["handleAuthRequests"] = json!(true);
                }
                mgr.client
                    .send_command("Fetch.enable", Some(params), Some(&session_id))
                    .await?;
            }
        }
    }

    navigate_active_page(state, url, wait_until).await
}

/// Navigate the active page and drop element refs and frame scope. Every
/// command that replaces the active document must go through here, or a
/// stale `@e3` from the previous page keeps resolving. WebMCP page state is
/// invalidated by `Page.frameNavigated` only when the document is replaced.
pub(super) async fn navigate_active_page(
    state: &mut DaemonState,
    url: &str,
    wait_until: WaitUntil,
) -> Result<Value, String> {
    // Same-document navigation retains registered tools and pending calls.
    // Subscribe before navigating, then let the ordered frameNavigated and
    // toolsAdded events invalidate and repopulate state for a new document.
    let page_session = state
        .browser
        .as_ref()
        .and_then(|browser| browser.active_session_id().ok())
        .map(ToString::to_string);
    crate::timing::mark("nav webmcp enable");
    let _ = enable_webmcp_events(state).await;
    crate::timing::mark("nav webmcp enabled");

    // With one tab, every tracked iframe belongs to the page being replaced.
    // With multiple tabs, retain the other tabs' sessions so switching back to
    // an already-attached OOPIF does not lose its execution context.
    let has_background_tabs = state
        .browser
        .as_ref()
        .is_some_and(|browser| browser.page_count() > 1);
    if !has_background_tabs {
        state.iframe_sessions.clear();
    }

    if let Some(session_id) = page_session.as_deref() {
        state.ref_map.invalidate_page(session_id);
    } else {
        state.ref_map.begin_snapshot();
    }
    state.active_iframe_sessions.clear();
    state.active_frame_id = None;
    state
        .blocked_documents
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
    crate::timing::mark("nav navigate");
    let result = mgr.navigate(url, wait_until).await?;
    crate::timing::mark("nav navigated");
    state.refresh_active_iframe_sessions().await;
    crate::timing::mark("nav iframes refreshed");

    // The domain filter answers a blocked document with a 403 page, so a
    // redirect to a disallowed host still completes as a navigation.
    // The main frame shares its ID with the target.
    let blocked = result
        .get("targetId")
        .and_then(|v| v.as_str())
        .and_then(|target_id| {
            state
                .blocked_documents
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(target_id)
        });
    if let Some(blocked_url) = blocked {
        return Err(format!(
            "Navigation to {} was blocked by the domain filter",
            blocked_url
        ));
    }
    Ok(result)
}

pub(super) async fn handle_url(state: &DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            let url = wb.get_url().await?;
            return Ok(json!({ "url": url }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let url = mgr.get_url().await?;
    Ok(json!({ "url": url }))
}

pub(super) async fn handle_read(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mut options = crate::read::options_from_command(cmd)?;
    if let Some(allowed_domains) = {
        let df = state.domain_filter.read().await;
        df.as_ref().map(|filter| filter.allowed_domains.clone())
    } {
        if !allowed_domains.is_empty() {
            options.enforced_allowed_domains.push(allowed_domains);
        }
    }

    if let Some(url) = cmd.get("url").and_then(|v| v.as_str()) {
        return crate::read::run_read(url, options).await;
    }

    let url_data = handle_url(state).await?;
    let active_url = url_data
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "Active tab has no URL".to_string())?;
    crate::read::check_allowed_active_url_for_options(active_url, &options)?;

    if options.llms.is_some() || options.require_md {
        return crate::read::run_read(active_url, options).await;
    }

    let content_data = handle_content(state).await?;
    let html = content_data
        .get("html")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Active tab content is unavailable".to_string())?;
    let origin = content_data
        .get("origin")
        .and_then(|v| v.as_str())
        .filter(|origin| !origin.is_empty())
        .unwrap_or(active_url);
    Ok(crate::read::read_json_from_active_html(
        origin,
        html.to_string(),
        &options,
    ))
}

pub(super) async fn handle_cdp_url(state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    if !mgr.is_connection_alive().await {
        return Err("Browser not launched".to_string());
    }
    Ok(json!({ "cdpUrl": mgr.get_cdp_url() }))
}

pub(super) async fn handle_inspect(state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;

    // Shut down any existing inspect server so we always target the current page
    if let Some(server) = state.inspect_server.take() {
        server.shutdown();
    }

    let target_id = mgr.active_target_id()?.to_string();
    let chrome_hp = mgr.chrome_host_port().to_string();
    let proxy_handle = mgr.client.inspect_handle();

    let server = InspectServer::start(proxy_handle, target_id, chrome_hp).await?;
    let url = format!("http://127.0.0.1:{}", server.port());
    open_url_in_browser(&url);

    state.inspect_server = Some(server);
    Ok(json!({ "opened": true, "url": url }))
}

pub(super) fn open_url_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let result: Result<std::process::Child, std::io::Error> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "unsupported platform",
    ));
    if let Err(e) = result {
        let _ = writeln!(std::io::stderr(), "[inspect] Failed to open browser: {}", e);
    }
}

pub(super) async fn handle_title(state: &DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            let title = wb.get_title().await?;
            return Ok(json!({ "title": title }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let title = mgr.get_title().await?;
    Ok(json!({ "title": title }))
}

pub(super) async fn handle_content(state: &DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            let html = wb.get_content().await?;
            let url = wb.get_url().await.unwrap_or_default();
            return Ok(json!({ "html": html, "origin": url }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let html = mgr.get_content().await?;
    let url = mgr.get_url().await.unwrap_or_default();
    Ok(json!({ "html": html, "origin": url }))
}

pub(super) async fn handle_evaluate(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            let script = cmd
                .get("script")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'script' parameter")?;
            let result = wb.evaluate(script).await?;
            let url = wb.get_url().await.unwrap_or_default();
            return Ok(json!({ "result": result, "origin": url }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let script = cmd
        .get("script")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'script' parameter")?;

    let result = mgr.evaluate_remote_object(script).await?;
    let unserializable = result
        .unserializable_value
        .clone()
        .or_else(|| (result.object_type == "undefined").then(|| "undefined".to_string()));
    let url = mgr.get_url().await.unwrap_or_default();
    let mut data = json!({ "result": result.value.unwrap_or(Value::Null), "origin": url });
    if let Some(token) = unserializable {
        data["unserializable"] = json!(token);
    }
    Ok(data)
}

/// A user `close`: the session is over, so its stored security settings go.
pub(super) async fn handle_close(state: &mut DaemonState) -> Result<Value, String> {
    close_session(state, true).await
}

/// Closes the browser and what hangs off it. The stored security settings are
/// forgotten only when `forget_settings` is set and the close succeeded: the
/// daemon restarts the CLI asks for (upgrade, changed options) keep them.
pub(super) async fn close_session(
    state: &mut DaemonState,
    forget_settings: bool,
) -> Result<Value, String> {
    let save_result = auto_save_restore_state(state).await;
    close_all_browser_backends(state).await?;
    state.attached_target = None;

    // Stop background Fetch handler
    if let Some(task) = state.fetch_handler_task.take() {
        task.abort();
    }
    {
        let mut map = state.origin_headers.write().await;
        map.clear();
    }
    state.session_setup = SessionSetup::default();

    if let Some(server) = state.inspect_server.take() {
        server.shutdown();
    }

    state.ref_map.invalidate_all_documents();
    // An explicit close releases this session's pin before replying, so other
    // sessions need not wait for the daemon process to exit.
    let pin_release_error = match tab_binding::release_pin(&state.session_id) {
        Ok(Some(binding)) => {
            state.last_persisted_binding = Some(binding);
            None
        }
        Ok(None) => None,
        Err(e) => Some(e),
    };
    let settings_clear_error = if forget_settings {
        state.store.clear().err()
    } else {
        None
    };
    let unsaved = if state.flow.active() {
        state.har_recording = false;
        state.har_entries.clear();
        state.har_progress.clear();
        state.har_body_total_bytes = 0;
        state.flow.end_capture()
    } else {
        0
    };
    let mut reply = match save_result {
        Ok(Some(path)) => json!({
            "closed": true,
            "restoreStatus": state.restore_status,
            "saveStatus": state.restore_save_status,
            "statePath": path
        }),
        Ok(None) => json!({
            "closed": true,
            "restoreStatus": state.restore_status,
            "saveStatus": state.restore_save_status
        }),
        Err(err) => json!({
            "closed": true,
            "restoreStatus": state.restore_status,
            "saveStatus": state.restore_save_status,
            "saveError": err
        }),
    };
    if unsaved > 0 {
        reply["unsaved"] = json!({ "steps": unsaved });
    }
    if let Some(err) = pin_release_error {
        reply["pinReleaseError"] = json!(err);
    }
    if let Some(err) = settings_clear_error {
        reply["settingsClearError"] = json!(err.to_string());
    }
    Ok(reply)
}

// ---------------------------------------------------------------------------
// Phase 2 handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_back(state: &mut DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            wb.back().await?;
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let url = wb.get_url().await.unwrap_or_default();
            state.ref_map.invalidate_all_documents();
            return Ok(json!({ "url": url }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    mgr.evaluate("history.back()", None).await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let url = mgr.get_url().await.unwrap_or_default();
    state.ref_map.invalidate_page(&session_id);
    Ok(json!({ "url": url }))
}

pub(super) async fn handle_forward(state: &mut DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            wb.forward().await?;
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let url = wb.get_url().await.unwrap_or_default();
            state.ref_map.invalidate_all_documents();
            return Ok(json!({ "url": url }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    mgr.evaluate("history.forward()", None).await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let url = mgr.get_url().await.unwrap_or_default();
    state.ref_map.invalidate_page(&session_id);
    Ok(json!({ "url": url }))
}

pub(super) async fn handle_reload(state: &mut DaemonState) -> Result<Value, String> {
    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            wb.reload().await?;
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            let url = wb.get_url().await.unwrap_or_default();
            state.ref_map.invalidate_all_documents();
            return Ok(json!({ "url": url }));
        }
    }
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    mgr.client
        .send_command_no_params("Page.reload", Some(&session_id))
        .await?;

    let mut rx = mgr.client.subscribe();
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if event.method == "Page.loadEventFired"
                        && event.session_id.as_deref() == Some(&session_id)
                    {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    })
    .await;

    let url = mgr.get_url().await.unwrap_or_default();
    state.ref_map.invalidate_page(&session_id);
    Ok(json!({ "url": url }))
}

// ---------------------------------------------------------------------------
// Wait helpers
// ---------------------------------------------------------------------------
