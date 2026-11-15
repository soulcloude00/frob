use super::*;

pub(super) async fn handle_tab_list(state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let tabs = mgr.tab_list();
    Ok(json!({ "tabs": tabs }))
}

pub(super) async fn handle_tab_new(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let url = cmd.get("url").and_then(|v| v.as_str());
    let label = cmd.get("label").and_then(|v| v.as_str());
    let domain_filter = state.domain_filter.read().await.clone();
    let has_proxy_creds = state.proxy_credentials.read().await.is_some();
    let defer_url_until_controls =
        should_defer_url_until_network_controls(domain_filter.as_ref(), has_proxy_creds, url)?;
    // UA, init scripts, headers, and routes are per-session in CDP, so when
    // any are configured the tab is created blank and navigated only after
    // they are replayed onto it; otherwise the first document would miss them.
    let defer_url =
        defer_url_until_controls || (url.is_some() && session_setup_pending(state).await);

    let (mut result, new_session_id) = {
        let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
        let result = mgr
            .tab_new(if defer_url { None } else { url }, label)
            .await?;
        (result, mgr.active_session_id()?.to_string())
    };

    state.ref_map.begin_snapshot();
    state.active_iframe_sessions.clear();
    state.active_frame_id = None;
    state.webmcp.clear_invocations();

    apply_session_setup(state, &new_session_id).await?;
    install_network_controls_or_close(state, has_proxy_creds).await?;
    state.drain_cdp_events_background().await?;

    if defer_url {
        if let Some(url) = url {
            let nav = {
                let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
                mgr.navigate(url, WaitUntil::Load).await?
            };
            if let Some(obj) = result.as_object_mut() {
                if let Some(value) = nav.get("url") {
                    obj.insert("url".to_string(), value.clone());
                }
                if let Some(value) = nav.get("title") {
                    obj.insert("title".to_string(), value.clone());
                }
            }
        }
    }

    state.refresh_active_iframe_sessions().await;

    Ok(result)
}

pub(super) async fn handle_tab_switch(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let tab_ref_str = cmd
        .get("tabId")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'tabId' parameter (expected `t<N>` or a label)")?;
    let tab_ref = crate::native::browser::TabRef::parse(tab_ref_str)?;
    let tab_id = {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        mgr.resolve_tab_ref(&tab_ref)?
    };
    if let Some(target_id) = state
        .browser
        .as_ref()
        .and_then(|mgr| mgr.target_id_for_tab(tab_id))
    {
        if let Some(owner) = crate::native::tab_binding::pinned_owner(target_id, &state.session_id)
        {
            return Err(format!(
                "tab_locked: tab {} is pinned by session '{}' (--pin-tab); close that session or use another tab",
                target_id, owner
            ));
        }
    }
    let dialog_session = state
        .pending_dialog
        .as_ref()
        .and_then(|d| d.session_id.clone());
    let result = {
        let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
        mgr.tab_switch_by_id(tab_id, dialog_session.as_deref())
            .await?
    };
    // Clear only after the switch commits, so a failed switch does not strand
    // the user on the old tab with dead refs and frame scope.
    state.ref_map.begin_snapshot();
    state.active_iframe_sessions.clear();
    state.active_frame_id = None;
    state.webmcp.clear_invocations();

    let has_proxy_creds = state.proxy_credentials.read().await.is_some();
    install_network_controls_or_close(state, has_proxy_creds).await?;
    state.refresh_active_iframe_sessions().await;

    // A dialog-blocked tab's renderer is paused and cannot answer an eval, so
    // skip the viewport sync; it would otherwise stall on the CDP timeout. The
    // sync resumes on the next command once the dialog is resolved.
    let dialog_blocked = result.get("dialogBlocked").and_then(|v| v.as_bool()) == Some(true);
    if let Some(ref server) = state.stream_server {
        if let Some(mgr) = state.browser.as_ref().filter(|_| !dialog_blocked) {
            if let Ok(dims) = mgr
                .evaluate(
                    "JSON.stringify([window.innerWidth,window.innerHeight])",
                    None,
                )
                .await
            {
                if let Some(s) = dims.get("result").and_then(|v| v.as_str()) {
                    if let Ok(arr) = serde_json::from_str::<Vec<u32>>(s) {
                        if arr.len() == 2 && arr[0] > 0 && arr[1] > 0 {
                            server.set_viewport(arr[0], arr[1]).await;
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

pub(super) async fn handle_tab_close(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let tab_id = {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        match cmd.get("tabId").and_then(|v| v.as_str()) {
            Some(s) => {
                let tab_ref = crate::native::browser::TabRef::parse(s)?;
                Some(mgr.resolve_tab_ref(&tab_ref)?)
            }
            None => None,
        }
    };
    let dialog_session = state
        .pending_dialog
        .as_ref()
        .and_then(|d| d.session_id.clone());
    let result = {
        let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
        mgr.tab_close_by_id(tab_id, dialog_session.as_deref())
            .await?
    };
    // Clear only after the close commits; a rejected close (last tab, bad
    // index) must not wipe the caller's refs and frame scope.
    state.ref_map.begin_snapshot();
    state.active_iframe_sessions.clear();
    state.webmcp.clear_invocations();
    state.active_frame_id = None;
    state.refresh_active_iframe_sessions().await;
    Ok(result)
}

pub(super) async fn handle_frame(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let selector = cmd.get("selector").and_then(|v| v.as_str());
    let name = cmd.get("name").and_then(|v| v.as_str());
    let url = cmd.get("url").and_then(|v| v.as_str());

    if selector.is_none() && name.is_none() && url.is_none() {
        return Err("At least one of 'selector', 'name', or 'url' is required".to_string());
    }

    let tree_result = mgr
        .client
        .send_command_no_params("Page.getFrameTree", Some(&session_id))
        .await?;

    fn find_frame(tree: &Value, name: Option<&str>, url: Option<&str>) -> Option<String> {
        let frame = tree.get("frame")?;
        let frame_name = frame.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let frame_url = frame.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let frame_id = frame.get("id").and_then(|v| v.as_str())?;

        if let Some(n) = name {
            if frame_name == n {
                return Some(frame_id.to_string());
            }
        }
        if let Some(u) = url {
            if frame_url.contains(u) {
                return Some(frame_id.to_string());
            }
        }

        if let Some(children) = tree.get("childFrames").and_then(|v| v.as_array()) {
            for child in children {
                if let Some(id) = find_frame(child, name, url) {
                    return Some(id);
                }
            }
        }
        None
    }

    let frame_tree = &tree_result["frameTree"];

    // If selector is a ref (@e1), resolve the iframe element from the ref map
    if let Some(sel) = selector {
        if let Some(ref_id) = crate::native::element::parse_ref(sel) {
            let entry = state
                .ref_map
                .get(&ref_id)
                .ok_or_else(|| format!("Unknown ref: {}", ref_id))?;
            let backend_node_id = entry
                .backend_node_id
                .ok_or_else(|| format!("Ref {} has no backend node id", ref_id))?;

            // Use DOM.describeNode to resolve the child frame ID directly.
            // This works reliably for all iframes, including those without
            // name, id, or src attributes.
            let describe: Value = mgr
                .client
                .send_command(
                    "DOM.describeNode",
                    Some(json!({ "backendNodeId": backend_node_id, "depth": 1 })),
                    Some(&session_id),
                )
                .await?;

            // Verify this is an iframe/frame element
            let node_name = describe
                .get("node")
                .and_then(|n| n.get("nodeName"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if node_name != "IFRAME" && node_name != "FRAME" {
                return Err("Ref does not point to an iframe element".to_string());
            }

            // Try contentDocument.frameId first (standard for iframes)
            let frame_id = describe
                .get("node")
                .and_then(|n| n.get("contentDocument"))
                .and_then(|cd| cd.get("frameId"))
                .and_then(|v| v.as_str())
                // Fallback: the node itself may carry a frameId
                .or_else(|| {
                    describe
                        .get("node")
                        .and_then(|n| n.get("frameId"))
                        .and_then(|v| v.as_str())
                })
                .ok_or("Could not resolve frame ID for iframe element")?;

            let label = describe
                .get("node")
                .and_then(|n| n.get("attributes"))
                .and_then(|a| a.as_array())
                .and_then(|attrs| {
                    attrs
                        .iter()
                        .enumerate()
                        .find(|(_, v)| v.as_str() == Some("name"))
                        .and_then(|(i, _)| attrs.get(i + 1))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or(&ref_id);

            state.active_frame_id = Some(frame_id.to_string());
            return Ok(json!({ "frame": label }));
        }

        // CSS selector path
        let js = format!(
            r#"(() => {{
                const el = document.querySelector({});
                if (!el) return null;
                if (el.tagName === 'IFRAME' || el.tagName === 'FRAME') {{
                    return el.name || el.id || el.src || null;
                }}
                return null;
            }})()"#,
            serde_json::to_string(sel).unwrap_or_default()
        );
        let result = mgr.evaluate(&js, None).await?;
        let frame_name = result.as_str().ok_or("Could not find frame for selector")?;
        if let Some(frame_id) = find_frame(frame_tree, Some(frame_name), None) {
            state.active_frame_id = Some(frame_id);
            return Ok(json!({ "frame": frame_name }));
        }
    }

    if let Some(frame_id) = find_frame(frame_tree, name, url) {
        let label = name.or(url).unwrap_or("frame");
        state.active_frame_id = Some(frame_id);
        return Ok(json!({ "frame": label }));
    }

    Err("Frame not found".to_string())
}

pub(super) async fn handle_mainframe(state: &mut DaemonState) -> Result<Value, String> {
    state.active_frame_id = None;
    Ok(json!({ "frame": "main" }))
}

// ---------------------------------------------------------------------------
// Semantic locator handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_window_new(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let (tab_id, session_id) = {
        let mgr = state.browser.as_mut().ok_or("Browser not launched")?;

        // Create a new browser context
        let context_result = mgr
            .client
            .send_command_no_params("Target.createBrowserContext", None)
            .await?;
        let context_id = context_result
            .get("browserContextId")
            .and_then(|v| v.as_str())
            .ok_or("Failed to create browser context")?
            .to_string();

        let create_result: crate::native::cdp::types::CreateTargetResult = mgr
            .client
            .send_command_typed(
                "Target.createTarget",
                &json!({ "url": "about:blank", "browserContextId": context_id }),
                None,
            )
            .await?;

        let attach: crate::native::cdp::types::AttachToTargetResult = mgr
            .client
            .send_command_typed(
                "Target.attachToTarget",
                &crate::native::cdp::types::AttachToTargetParams {
                    target_id: create_result.target_id.clone(),
                    flatten: true,
                },
                None,
            )
            .await?;

        mgr.prepare_domains_pub(&attach.session_id).await?;

        let tab_id = mgr.assign_tab_id();
        mgr.add_page(crate::native::browser::PageInfo {
            tab_id,
            label: None,
            target_id: create_result.target_id,
            session_id: attach.session_id.clone(),
            url: "about:blank".to_string(),
            title: String::new(),
            target_type: "page".to_string(),
        });
        (tab_id, attach.session_id)
    };

    let has_proxy_creds = state.proxy_credentials.read().await.is_some();
    install_network_controls_or_resume_prepared_session(state, has_proxy_creds, &session_id)
        .await?;
    state.drain_cdp_events_background().await?;

    if let Some(viewport) = cmd.get("viewport") {
        let width = viewport
            .get("width")
            .and_then(|v| v.as_i64())
            .unwrap_or(1280) as i32;
        let height = viewport
            .get("height")
            .and_then(|v| v.as_i64())
            .unwrap_or(720) as i32;
        let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
        mgr.set_viewport(width, height, 1.0, false).await?;

        // Update stream server viewport
        if let Some(ref server) = state.stream_server {
            server.set_viewport(width as u32, height as u32).await;
        }
    }

    let total = state
        .browser
        .as_ref()
        .ok_or("Browser not launched")?
        .page_count();
    state.ref_map.begin_snapshot();

    Ok(json!({
        "tabId": crate::native::browser::format_tab_id(tab_id),
        "total": total,
    }))
}
