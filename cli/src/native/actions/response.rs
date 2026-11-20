use super::*;

pub(super) fn success_response(id: &str, data: Value) -> Value {
    json!({
        "id": id,
        "success": true,
        "data": data,
    })
}

/// Enrich ordinary browser results so discovery does not depend on the agent
/// remembering to call `webmcp list`. Administrative commands stay page-free.
pub(super) fn includes_webmcp_context(action: &str) -> bool {
    action != "webmcp_list"
        && (!skip_launch_action(action)
            || matches!(action, "launch" | "webmcp_result" | "webmcp_cancel"))
}

pub(super) async fn attach_webmcp_availability(
    resp: &mut Value,
    action: &str,
    state: &mut DaemonState,
) {
    if !includes_webmcp_context(action)
        || matches!(state.backend_type, BackendType::WebDriver)
        || state.engine != "chrome"
        || !state.webmcp_enabled
        || state.browser.is_none()
    {
        return;
    }
    // The first subscription is bounded. Once installed, observation is passive
    // and the dispatcher already drains CDP events after every browser action.
    let initializing = state
        .browser
        .as_ref()
        .and_then(|b| b.active_session_id().ok())
        .is_some_and(|session| !state.webmcp.observations.contains_key(session));
    let observed = enable_webmcp_events(state).await;
    let drained = if initializing {
        state.drain_cdp_events_background().await
    } else {
        Ok(())
    };
    let session = state
        .browser
        .as_ref()
        .and_then(|b| b.active_session_id().ok())
        .map(str::to_string);
    if let Some(session) = session {
        if observed.is_ok() && drained.is_ok() {
            if let Ok(tools) = state.webmcp.tools(&session) {
                let filter = state.domain_filter.read().await;
                attach_webmcp_availability_from_tools(
                    resp,
                    action,
                    &tools,
                    filter.as_ref(),
                    &mut state.webmcp,
                    &session,
                );
                return;
            }
        }
    }
    if let Some(context) = state.webmcp.context_unavailable() {
        insert_webmcp_context(resp, context);
    }
}

pub(super) fn attach_webmcp_availability_from_tools(
    resp: &mut Value,
    action: &str,
    tools: &[webmcp::ToolRecord],
    domain_filter: Option<&DomainFilter>,
    runtime: &mut webmcp::RuntimeState,
    session: &str,
) {
    if !includes_webmcp_context(action) {
        return;
    }
    let allowed: Vec<_> = tools
        .iter()
        .filter(|tool| domain_filter.is_none_or(|filter| filter.check_url(&tool.origin).is_ok()))
        .cloned()
        .collect();
    if let Some(context) = runtime.context_update(session, &allowed) {
        insert_webmcp_context(resp, context);
    }
}

pub(super) fn insert_webmcp_context(resp: &mut Value, context: Value) {
    let Some(response) = resp.as_object_mut() else {
        return;
    };
    let data = response.entry("data").or_insert_with(|| json!({}));
    if let Some(data) = data.as_object_mut() {
        data.insert("webmcp".to_string(), context);
    }
}

pub(super) fn inject_lifecycle(
    resp: &mut Value,
    state: &DaemonState,
    reused: bool,
    launched: bool,
    relaunched_browser: bool,
) {
    if resp.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return;
    }

    let Some(data) = resp.get_mut("data") else {
        return;
    };
    let Some(obj) = data.as_object_mut() else {
        return;
    };

    let data_reused = obj.get("reused").and_then(|v| v.as_bool()).unwrap_or(false);
    let data_launched = obj
        .get("launched")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let data_relaunched = obj
        .get("relaunchedBrowser")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let effective_reused = reused || data_reused;
    let effective_launched = launched || data_launched;
    let effective_relaunched = relaunched_browser || data_relaunched;

    obj.insert(
        "lifecycle".to_string(),
        json!({
            "reused": effective_reused,
            "launched": effective_launched,
            "relaunchedBrowser": effective_relaunched,
            "restartedBackground": false,
            "restoreStatus": state.restore_status,
            "saveStatus": state.restore_save_status,
            "effectiveLaunch": {
                "browserLaunched": state.browser.is_some(),
                "engine": state.engine,
                "launchHash": state.launch_identity,
            }
        }),
    );
}

pub(super) fn error_response(id: &str, error: &str) -> Value {
    let mut resp = json!({
        "id": id,
        "success": false,
        "error": error,
    });
    // Machine-readable code for "the bound tab no longer exists" so scripts
    // using --json can match on it instead of parsing the message.
    if error.starts_with(crate::native::browser::TAB_GONE_PREFIX) {
        resp["code"] = json!("tab_gone");
    } else if error.starts_with(crate::native::browser::PAGE_WEDGED_PREFIX) {
        resp["code"] = json!("page_wedged");
    } else if let Some((code, _)) = error.split_once(": ") {
        if code.starts_with("webmcp_") {
            resp["code"] = json!(code);
        }
    }
    resp
}

pub(super) fn attach_tab_gone_data(resp: &mut Value, state: &DaemonState) {
    if resp.get("code").and_then(Value::as_str) != Some("tab_gone") {
        return;
    }
    let Some((target_id, last_url)) = state
        .browser
        .as_ref()
        .and_then(BrowserManager::bound_target_gone_details)
    else {
        return;
    };

    let mut data = json!({ "targetId": target_id });
    if !last_url.is_empty() {
        data["lastUrl"] = json!(last_url);
    }
    resp["data"] = data;
}
