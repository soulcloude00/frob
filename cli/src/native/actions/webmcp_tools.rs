use super::*;

pub(super) async fn webmcp_page_context(
    state: &DaemonState,
) -> Result<(Arc<CdpClient>, String, String), String> {
    if matches!(state.backend_type, BackendType::WebDriver) || state.engine != "chrome" {
        return Err(webmcp::unsupported_error(&format!(
            "active backend is {}",
            state.engine
        )));
    }
    let browser = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = browser.active_session_id()?.to_string();
    let url = browser.get_url().await?;
    {
        let filter = state.domain_filter.read().await;
        if let Some(filter) = filter.as_ref() {
            filter.check_url(&url)?;
        }
    }
    let origin = url::Url::parse(&url)
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or(url);
    Ok((browser.client.clone(), session_id, origin))
}

pub(super) async fn collect_webmcp_tools(
    state: &mut DaemonState,
) -> Result<Vec<webmcp::ToolRecord>, String> {
    state.drain_cdp_events_background().await?;
    let (client, session_id, origin) = webmcp_page_context(state).await?;
    let mut rx = client.subscribe();
    fn collect_origins(tree: &Value, origins: &mut HashMap<String, String>) {
        if let Some(frame) = tree.get("frame") {
            if let (Some(frame_id), Some(origin)) = (
                frame.get("id").and_then(Value::as_str),
                webmcp::frame_origin(frame),
            ) {
                origins.insert(frame_id.to_string(), origin);
            }
        }
        if let Some(children) = tree.get("childFrames").and_then(Value::as_array) {
            for child in children {
                collect_origins(child, origins);
            }
        }
    }
    let mut frame_origins = HashMap::new();
    let frame_tree = client
        .send_command_no_params("Page.getFrameTree", Some(&session_id))
        .await
        .unwrap_or_else(|_| json!({}));
    if let Some(tree) = frame_tree.get("frameTree") {
        collect_origins(tree, &mut frame_origins);
    }
    for (frame_id, frame_origin) in &frame_origins {
        state
            .webmcp
            .update_frame_origin(&session_id, frame_id, frame_origin.as_str());
    }
    client
        .send_command_no_params("WebMCP.enable", Some(&session_id))
        .await
        .map_err(|error| webmcp::unsupported_error(&error))?;

    state.webmcp.observations.insert(session_id.clone(), Ok(()));

    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(webmcp::DISCOVERY_WINDOW_MS);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(event))
                if event.method == "WebMCP.toolsAdded"
                    && event.session_id.as_deref() == Some(session_id.as_str()) =>
            {
                state
                    .webmcp
                    .apply_tools_added(&session_id, &event.params, &origin);
            }
            Ok(Ok(event))
                if event.method == "WebMCP.toolsRemoved"
                    && event.session_id.as_deref() == Some(session_id.as_str()) =>
            {
                state.webmcp.apply_tools_removed(&session_id, &event.params);
            }
            Ok(Ok(_)) => continue,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => break,
        }
    }
    let tools = state.webmcp.tools(&session_id)?;
    let domain_filter = state.domain_filter.read().await.clone();
    if let Some(filter) = domain_filter.as_ref() {
        for tool in &tools {
            filter.check_url(&tool.origin)?;
        }
    }
    Ok(tools)
}

/// Subscribe once per CDP page session. Ordinary actions then read the event
/// cache without renderer evaluation, repeated CDP requests, or discovery waits.
pub(super) async fn enable_webmcp_events(state: &mut DaemonState) -> Result<(), String> {
    if !state.webmcp_enabled
        || state.engine != "chrome"
        || matches!(state.backend_type, BackendType::WebDriver)
    {
        return Err("WebMCP observation disabled".to_string());
    }
    let browser = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = browser.active_session_id()?.to_string();
    if let Some(result) = state.webmcp.observations.get(&session_id) {
        return result.clone();
    }
    let client = browser.client.clone();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        let tree = client
            .send_command_no_params("Page.getFrameTree", Some(&session_id))
            .await?;
        client
            .send_command_no_params("WebMCP.enable", Some(&session_id))
            .await?;
        fn update_origins(state: &mut webmcp::RuntimeState, session: &str, tree: &Value) {
            if let (Some(id), Some(origin)) = (
                tree["frame"]["id"].as_str(),
                webmcp::frame_origin(&tree["frame"]),
            ) {
                state.update_frame_origin(session, id, &origin);
            }
            if let Some(children) = tree["childFrames"].as_array() {
                for child in children {
                    update_origins(state, session, child);
                }
            }
        }
        update_origins(&mut state.webmcp, &session_id, &tree["frameTree"]);
        Ok::<(), String>(())
    })
    .await
    .unwrap_or_else(|_| Err("WebMCP observation timed out".to_string()));
    // Unsupported sessions are not probed again on every action. An explicit
    // list request can retry setup; fresh browser sessions start with no cache.
    state.webmcp.observations.insert(session_id, result.clone());
    result
}

pub(super) async fn handle_webmcp_list(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mut tools = collect_webmcp_tools(state).await?;
    if let Some(name) = cmd.get("tool").and_then(Value::as_str) {
        tools =
            vec![
                webmcp::resolve_tool(&tools, name, cmd.get("frameId").and_then(Value::as_str))?
                    .clone(),
            ];
    } else if let Some(frame) = cmd.get("frameId").and_then(Value::as_str) {
        tools.retain(|tool| tool.frame_id == frame);
    }
    let mut resp = json!({"experimental": true, "untrusted": true, "tools": tools});
    // Saved flows are tools the agent observed on this site, next to the ones
    // the page declares.
    if cmd.get("tool").is_none() {
        let host = match state.browser.as_ref() {
            Some(mgr) => mgr.get_url().await.ok(),
            None => None,
        }
        .and_then(|u| url::Url::parse(&u).ok())
        .and_then(|u| u.host_str().map(String::from));
        if let Some(host) = host {
            resp["observed"] = json!(crate::native::flow::list_flows(Some(&host)));
        }
    }
    Ok(resp)
}

pub(super) async fn wait_for_webmcp_invocation(
    state: &mut DaemonState,
    invocation_id: &str,
    timeout_ms: u64,
) -> Result<Value, String> {
    let deadline = tokio::time::Instant::now() + webmcp::timeout_duration(timeout_ms);
    loop {
        state.drain_cdp_events_background().await?;
        if let Some(record) = state.webmcp.invocations.get(invocation_id) {
            if record.is_terminal() {
                return Ok(record.to_json());
            }
        } else {
            return Err(format!(
                "{}: Unknown WebMCP invocation '{}'",
                webmcp::ERR_INVOCATION_NOT_FOUND,
                invocation_id
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            if let Ok((client, session_id, _)) = webmcp_page_context(state).await {
                let _ = client
                    .send_command(
                        "WebMCP.cancelInvocation",
                        Some(json!({ "invocationId": invocation_id })),
                        Some(&session_id),
                    )
                    .await;
            }
            if let Some(record) = state.webmcp.invocations.get_mut(invocation_id) {
                record.mark_timed_out();
                return Ok(record.to_json());
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub(super) async fn handle_webmcp_invoke(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let tool_name = cmd
        .get("tool")
        .and_then(Value::as_str)
        .ok_or("Missing 'tool' parameter")?;
    let input = cmd.get("params").cloned().unwrap_or_else(|| json!({}));
    webmcp::validate_input(&input)?;
    state.webmcp.ensure_capacity()?;
    let frame_id = cmd.get("frameId").and_then(Value::as_str);
    let tools = collect_webmcp_tools(state).await?;
    let tool = webmcp::resolve_tool(&tools, tool_name, frame_id)?;
    let (client, session_id, _) = webmcp_page_context(state).await?;
    let result = client
        .send_command(
            "WebMCP.invokeTool",
            Some(json!({
                "frameId": tool.frame_id,
                "toolName": tool.name,
                "input": input,
            })),
            Some(&session_id),
        )
        .await
        .map_err(|error| webmcp::invoke_error(&error))?;
    let invocation_id = result
        .get("invocationId")
        .and_then(Value::as_str)
        .ok_or("WebMCP.invokeTool response is missing invocationId")?
        .to_string();
    state.webmcp.insert(webmcp::InvocationRecord::pending(
        invocation_id.clone(),
        tool.name,
        tool.frame_id,
        tool.origin,
    ))?;

    if cmd.get("detach").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(state.webmcp.invocations[&invocation_id].to_json());
    }
    wait_for_webmcp_invocation(state, &invocation_id, state.timeout_ms(cmd)).await
}

pub(super) async fn handle_webmcp_result(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let invocation_id = cmd
        .get("invocationId")
        .and_then(Value::as_str)
        .ok_or("Missing 'invocationId' parameter")?;
    wait_for_webmcp_invocation(state, invocation_id, state.timeout_ms(cmd)).await
}

pub(super) async fn handle_webmcp_cancel(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let invocation_id = cmd
        .get("invocationId")
        .and_then(Value::as_str)
        .ok_or("Missing 'invocationId' parameter")?;
    state.drain_cdp_events_background().await?;
    let record = state.webmcp.invocations.get(invocation_id).ok_or_else(|| {
        format!(
            "{}: Unknown WebMCP invocation '{}'",
            webmcp::ERR_INVOCATION_NOT_FOUND,
            invocation_id
        )
    })?;
    if record.is_terminal() {
        return Err(format!(
            "{}: WebMCP invocation '{}' is already {}",
            webmcp::ERR_INVOCATION_NOT_ACTIVE,
            invocation_id,
            record.status
        ));
    }
    let (client, session_id, _) = webmcp_page_context(state).await?;
    if let Err(error) = client
        .send_command(
            "WebMCP.cancelInvocation",
            Some(json!({ "invocationId": invocation_id })),
            Some(&session_id),
        )
        .await
    {
        state.drain_cdp_events_background().await?;
        if let Some(record) = state.webmcp.invocations.get(invocation_id) {
            if record.is_terminal() {
                return Ok(record.to_json());
            }
        }
        return Err(webmcp::cancel_error(&error));
    }
    wait_for_webmcp_invocation(state, invocation_id, state.timeout_ms(cmd)).await
}

// ---------------------------------------------------------------------------
// Screencast handlers
// ---------------------------------------------------------------------------
