use super::*;

pub(super) async fn handle_dialog(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let response = cmd.get("response").and_then(|v| v.as_str());

    // dialog status — return pending dialog info
    if response == Some("status") {
        return Ok(match &state.pending_dialog {
            Some(dialog) => {
                let mut obj = json!({
                    "hasDialog": true,
                    "type": dialog.dialog_type,
                    "message": dialog.message,
                });
                if let Some(ref prompt) = dialog.default_prompt {
                    obj["defaultPrompt"] = json!(prompt);
                }
                obj
            }
            None => json!({ "hasDialog": false }),
        });
    }

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let accept = response
        .map(|r| r == "accept")
        .or_else(|| cmd.get("accept").and_then(|v| v.as_bool()))
        .unwrap_or(true);
    let prompt_text = cmd.get("promptText").and_then(|v| v.as_str());

    // Clear tracked state even if Chrome reports no dialog (e.g. it was
    // already resolved and the closed event was missed); otherwise a stale
    // pending_dialog would make every page command fail fast forever.
    let result = mgr.handle_dialog(accept, prompt_text).await;
    state.pending_dialog = None;
    result?;

    // If a click's mousedown opened this dialog, the button is still logically
    // down. Release it now that the page is unblocked so the next click does
    // not register as a drag or double-click.
    if let Some(release) = state.pending_pointer_release.take() {
        if let Some(ref mgr) = state.browser {
            let _ = interaction::dispatch_pending_release(&mgr.client, &release).await;
        }
        state.mouse_state.buttons = 0;
        if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
            cursor.record(state.mouse_state.x, state.mouse_state.y, 0);
        }
    }
    Ok(json!({ "handled": true, "accepted": accept }))
}

pub(super) async fn handle_upload(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let files: Vec<String> = cmd
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .or_else(|| {
            cmd.get("file")
                .and_then(|v| v.as_str())
                .map(|s| vec![s.to_string()])
        })
        .unwrap_or_default();

    mgr.upload_files(selector, &files, &state.ref_map, &state.iframe_sessions)
        .await?;
    Ok(json!({ "uploaded": files.len(), "selector": selector }))
}

pub(super) async fn handle_addscript(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let content = cmd
        .get("content")
        .or_else(|| cmd.get("source"))
        .or_else(|| cmd.get("script"))
        .and_then(|v| v.as_str());
    let url = cmd.get("url").and_then(|v| v.as_str());

    if content.is_none() && url.is_none() {
        return Err("At least one of 'content' or 'url' is required".to_string());
    }

    if let Some(src_url) = url {
        let js = format!(
            r#"new Promise((resolve, reject) => {{
                const s = document.createElement('script');
                s.src = {};
                s.onload = () => resolve(true);
                s.onerror = () => reject(new Error('Failed to load script'));
                document.head.appendChild(s);
            }})"#,
            serde_json::to_string(src_url).unwrap_or_default()
        );
        mgr.evaluate(&js, None).await?;
    } else if let Some(source) = content {
        let js = format!(
            r#"(() => {{
                const s = document.createElement('script');
                s.textContent = {};
                document.head.appendChild(s);
            }})()"#,
            serde_json::to_string(source).unwrap_or_default()
        );
        mgr.evaluate(&js, None).await?;
    }

    Ok(json!({ "added": true }))
}

pub(super) async fn handle_addinitscript(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let source = cmd
        .get("script")
        .or_else(|| cmd.get("source"))
        .or_else(|| cmd.get("content"))
        .and_then(|v| v.as_str())
        .ok_or("Missing 'script' parameter")?;

    let session_identifier = mgr.add_script_to_evaluate(source).await?;
    let identifier = state.session_setup.register_init_script(
        source.to_string(),
        session_id,
        session_identifier,
    );
    Ok(json!({ "added": true, "identifier": identifier }))
}

pub(super) async fn handle_removeinitscript(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let identifier = cmd
        .get("identifier")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'identifier' parameter")?;
    let (client, active_session_id, live_session_ids) = {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        (
            mgr.client.clone(),
            mgr.active_session_id()?.to_string(),
            mgr.pages_list()
                .into_iter()
                .map(|page| page.session_id)
                .collect::<HashSet<_>>(),
        )
    };
    let tracked_index = state
        .session_setup
        .init_scripts
        .iter()
        .position(|script| script.identifier == identifier);

    // Daemon-owned handles are session-wide. Remove each target-local CDP
    // registration before dropping the source that would reach future tabs.
    if let Some(index) = tracked_index {
        let session_identifiers = state.session_setup.init_scripts[index]
            .session_identifiers
            .clone();
        for (session_id, session_identifier) in session_identifiers {
            if !live_session_ids.contains(&session_id) {
                continue;
            }
            client
                .send_command(
                    "Page.removeScriptToEvaluateOnNewDocument",
                    Some(json!({ "identifier": session_identifier })),
                    Some(&session_id),
                )
                .await?;
            state.session_setup.init_scripts[index]
                .session_identifiers
                .remove(&session_id);
        }
        state.session_setup.init_scripts.remove(index);
    } else {
        client
            .send_command(
                "Page.removeScriptToEvaluateOnNewDocument",
                Some(json!({ "identifier": identifier })),
                Some(&active_session_id),
            )
            .await?;
    }
    Ok(json!({ "removed": true, "identifier": identifier }))
}

// === React / Web primitives ===

/// Parse a `Runtime.evaluate` result whose expression returned a JSON string.
/// Returns a helpful error if parsing fails.
pub(super) fn parse_json_string(value: Value, what: &str) -> Result<Value, String> {
    let s = value
        .as_str()
        .ok_or_else(|| format!("{} returned non-string value", what))?;
    serde_json::from_str(s).map_err(|e| format!("{} returned invalid JSON: {}", what, e))
}

pub(super) async fn handle_pushstate(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let url = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' parameter")?;
    let script = react::scripts::PUSHSTATE.replace(
        "{{URL}}",
        &serde_json::to_string(url).unwrap_or_else(|_| "\"\"".to_string()),
    );
    let result = mgr.evaluate(&script, None).await?;
    let after = result.as_str().map(String::from).unwrap_or_default();
    Ok(json!({ "url": after }))
}

pub(super) async fn handle_addstyle(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let content = cmd
        .get("content")
        .or_else(|| cmd.get("css"))
        .and_then(|v| v.as_str());
    let url = cmd.get("url").and_then(|v| v.as_str());

    if content.is_none() && url.is_none() {
        return Err("At least one of 'content' or 'url' is required".to_string());
    }

    if let Some(href) = url {
        let js = format!(
            r#"new Promise((resolve, reject) => {{
                const link = document.createElement('link');
                link.rel = 'stylesheet';
                link.href = {};
                link.onload = () => resolve(true);
                link.onerror = () => reject(new Error('Failed to load stylesheet'));
                document.head.appendChild(link);
            }})"#,
            serde_json::to_string(href).unwrap_or_default()
        );
        mgr.evaluate(&js, None).await?;
    } else if let Some(css) = content {
        let js = format!(
            r#"(() => {{
                const style = document.createElement('style');
                style.textContent = {};
                document.head.appendChild(style);
            }})()"#,
            serde_json::to_string(css).unwrap_or_default()
        );
        mgr.evaluate(&js, None).await?;
    }

    Ok(json!({ "added": true }))
}

pub(super) async fn handle_clipboard(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let action = cmd
        .get("subAction")
        .or_else(|| cmd.get("operation"))
        .and_then(|v| v.as_str())
        .unwrap_or("read");

    let session_id = mgr.active_session_id()?.to_string();

    // cfg! is compile-time; assumes the browser runs on the same OS as the CLI binary.
    let modifier: i32 = if cfg!(target_os = "macos") { 4 } else { 2 };

    match action {
        "write" => {
            let text = cmd
                .get("text")
                .or_else(|| cmd.get("value"))
                .and_then(|v| v.as_str())
                .ok_or("Missing 'text' parameter")?;
            let js = format!(
                "navigator.clipboard.writeText({})",
                serde_json::to_string(text).unwrap_or_default()
            );
            mgr.evaluate(&js, None).await?;
            Ok(json!({ "written": text }))
        }
        "copy" => {
            interaction::press_key_with_modifiers(&mgr.client, &session_id, "c", Some(modifier))
                .await?;
            Ok(json!({ "copied": true }))
        }
        "paste" => {
            interaction::press_key_with_modifiers(&mgr.client, &session_id, "v", Some(modifier))
                .await?;
            Ok(json!({ "pasted": true }))
        }
        _ => {
            let result = mgr.evaluate("navigator.clipboard.readText()", None).await?;
            Ok(json!({ "text": result }))
        }
    }
}

pub(super) async fn handle_evalhandle(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let script = cmd
        .get("script")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'script' parameter")?;

    let result: crate::native::cdp::types::EvaluateResult = mgr
        .client
        .send_command_typed(
            "Runtime.evaluate",
            &crate::native::cdp::types::EvaluateParams {
                expression: script.to_string(),
                return_by_value: Some(false),
                await_promise: Some(true),
            },
            Some(&session_id),
        )
        .await?;

    let handle = result.result.object_id.unwrap_or_default();
    Ok(json!({ "handle": handle }))
}

// ---------------------------------------------------------------------------
// Advanced interaction handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_expose(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;

    mgr.client
        .send_command(
            "Runtime.addBinding",
            Some(json!({ "name": name })),
            Some(&session_id),
        )
        .await?;

    Ok(json!({ "exposed": name }))
}

pub(super) async fn handle_pause(_state: &DaemonState) -> Result<Value, String> {
    Ok(json!({ "paused": true, "note": "Use DevTools to inspect. The daemon remains running." }))
}
