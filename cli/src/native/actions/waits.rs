use super::*;

pub(super) async fn handle_wait(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let timeout_ms = state.timeout_ms(cmd);

    if let Some(text) = cmd.get("text").and_then(|v| v.as_str()) {
        wait_for_text(&mgr.client, &session_id, text, timeout_ms).await?;
        return Ok(json!({ "waited": "text", "text": text }));
    }

    if let Some(selector) = cmd.get("selector").and_then(|v| v.as_str()) {
        let state_str = cmd
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("visible");
        // Honor an active `frame <sel>` selection, like element resolution does.
        match state.active_frame_id.as_deref() {
            Some(frame_id) => match state.iframe_sessions.get(frame_id) {
                Some(frame_session) => {
                    wait_for_selector(&mgr.client, frame_session, selector, state_str, timeout_ms)
                        .await?
                }
                None => {
                    wait_for_selector_in_frame(
                        &mgr.client,
                        &session_id,
                        frame_id,
                        selector,
                        state_str,
                        timeout_ms,
                    )
                    .await?
                }
            },
            None => {
                wait_for_selector(&mgr.client, &session_id, selector, state_str, timeout_ms).await?
            }
        }
        return Ok(json!({ "waited": "selector", "selector": selector }));
    }

    if let Some(url_pattern) = cmd.get("url").and_then(|v| v.as_str()) {
        wait_for_url(mgr, url_pattern, timeout_ms).await?;
        return Ok(json!({ "waited": "url", "url": url_pattern }));
    }

    if let Some(fn_str) = cmd.get("function").and_then(|v| v.as_str()) {
        wait_for_function(&mgr.client, &session_id, fn_str, timeout_ms).await?;
        return Ok(json!({ "waited": "function" }));
    }

    if let Some(load_state) = cmd.get("loadState").and_then(|v| v.as_str()) {
        let wait_until = WaitUntil::from_str(load_state);
        mgr.wait_for_lifecycle_external(wait_until, &session_id, timeout_ms)
            .await?;
        return Ok(json!({ "waited": "load", "state": load_state }));
    }

    // Just a timeout wait
    tokio::time::sleep(tokio::time::Duration::from_millis(timeout_ms)).await;
    Ok(json!({ "waited": "timeout", "ms": timeout_ms }))
}

pub(super) async fn wait_for_selector(
    client: &crate::native::cdp::client::CdpClient,
    session_id: &str,
    selector: &str,
    state: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let check_fn = match state {
        "attached" => format!(
            "!!document.querySelector({})",
            serde_json::to_string(selector).unwrap_or_default()
        ),
        "detached" => format!(
            "!document.querySelector({})",
            serde_json::to_string(selector).unwrap_or_default()
        ),
        "hidden" => format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return true;
                const s = window.getComputedStyle(el);
                return s.display === 'none' || s.visibility === 'hidden' || parseFloat(s.opacity) === 0;
            }})()"#,
            sel = serde_json::to_string(selector).unwrap_or_default()
        ),
        _ => format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return false;
                const r = el.getBoundingClientRect();
                const s = window.getComputedStyle(el);
                return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
            }})()"#,
            sel = serde_json::to_string(selector).unwrap_or_default()
        ),
    };

    poll_until_true(client, session_id, &check_fn, timeout_ms).await
}

pub(super) async fn wait_for_url(
    mgr: &BrowserManager,
    pattern: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);

    loop {
        let url = mgr.get_url().await?;
        if route_url_matches(pattern, &url) {
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(format!("Wait timed out after {}ms", timeout_ms));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

pub(super) async fn wait_for_text(
    client: &crate::native::cdp::client::CdpClient,
    session_id: &str,
    text: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let check_fn = format!(
        "(document.body.innerText || '').includes({})",
        serde_json::to_string(text).unwrap_or_default()
    );
    poll_until_true(client, session_id, &check_fn, timeout_ms).await
}

pub(super) async fn wait_for_function(
    client: &crate::native::cdp::client::CdpClient,
    session_id: &str,
    fn_str: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let check_fn = format!("!!({})", fn_str);
    poll_until_true(client, session_id, &check_fn, timeout_ms).await
}

/// wait_for_selector inside a same-process iframe selected via `frame <sel>`:
/// polls through the owner element's contentDocument, which stays correct
/// even if the frame navigates (the getter re-resolves every poll).
pub(super) async fn wait_for_selector_in_frame(
    client: &crate::native::cdp::client::CdpClient,
    session_id: &str,
    frame_id: &str,
    selector: &str,
    state: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let owner_object_id =
        crate::native::element::frame_owner_object_id(client, session_id, frame_id).await?;
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let check = match state {
        "attached" => format!("!!doc.querySelector({sel})"),
        "detached" => format!("!doc.querySelector({sel})"),
        "hidden" => format!(
            r#"(() => {{
                const el = doc.querySelector({sel});
                if (!el) return true;
                const s = doc.defaultView.getComputedStyle(el);
                return s.display === 'none' || s.visibility === 'hidden' || parseFloat(s.opacity) === 0;
            }})()"#,
        ),
        _ => format!(
            r#"(() => {{
                const el = doc.querySelector({sel});
                if (!el) return false;
                const r = el.getBoundingClientRect();
                const s = doc.defaultView.getComputedStyle(el);
                return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
            }})()"#,
        ),
    };
    let function = format!(
        "function() {{ const doc = this.contentDocument; if (!doc) return false; return {check}; }}",
    );
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
    loop {
        let result = client
            .send_command(
                "Runtime.callFunctionOn",
                Some(json!({
                    "objectId": owner_object_id,
                    "functionDeclaration": function,
                    "returnByValue": true,
                })),
                Some(session_id),
            )
            .await?;
        let satisfied = result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if satisfied {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("Wait timed out after {}ms", timeout_ms));
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

pub(super) async fn poll_until_true(
    client: &crate::native::cdp::client::CdpClient,
    session_id: &str,
    expression: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);

    loop {
        let result: crate::native::cdp::types::EvaluateResult = client
            .send_command_typed(
                "Runtime.evaluate",
                &crate::native::cdp::types::EvaluateParams {
                    expression: expression.to_string(),
                    return_by_value: Some(true),
                    await_promise: Some(true),
                },
                Some(session_id),
            )
            .await?;

        if result
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(format!("Wait timed out after {}ms", timeout_ms));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

// ---------------------------------------------------------------------------
// Phase 3 handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_waitforurl(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let url_pattern = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' parameter")?;
    let timeout_ms = state.timeout_ms(cmd);

    wait_for_url(mgr, url_pattern, timeout_ms).await?;
    let url = mgr.get_url().await.unwrap_or_default();
    Ok(json!({ "url": url }))
}

/// Wait for a page lifecycle state and return an error when its timeout expires.
pub(super) async fn handle_waitforloadstate(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let load_state = cmd.get("state").and_then(|v| v.as_str()).unwrap_or("load");
    let timeout_ms = state.timeout_ms(cmd);

    let wait_until = WaitUntil::from_str(load_state);
    mgr.wait_for_lifecycle_external(wait_until, &session_id, timeout_ms)
        .await?;

    Ok(json!({ "state": load_state }))
}

pub(super) async fn handle_waitforfunction(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let expression = cmd
        .get("expression")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'expression' parameter")?;
    let timeout_ms = state.timeout_ms(cmd);

    wait_for_function(&mgr.client, &session_id, expression, timeout_ms).await?;

    let result: crate::native::cdp::types::EvaluateResult = mgr
        .client
        .send_command_typed(
            "Runtime.evaluate",
            &crate::native::cdp::types::EvaluateParams {
                expression: format!("({})", expression),
                return_by_value: Some(true),
                await_promise: Some(true),
            },
            Some(&session_id),
        )
        .await?;

    Ok(json!({ "result": result.result.value.unwrap_or(Value::Null) }))
}

// ---------------------------------------------------------------------------
// Frame handlers
// ---------------------------------------------------------------------------
