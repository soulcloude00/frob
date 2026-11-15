use super::*;

pub(super) async fn handle_gettext(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let text = crate::native::element::get_element_text(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    let url = mgr.get_url().await.unwrap_or_default();
    Ok(json!({ "text": text, "origin": url }))
}

pub(super) async fn handle_getattribute(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let attribute = cmd
        .get("attribute")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'attribute' parameter")?;

    let value = crate::native::element::get_element_attribute(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        attribute,
        &state.iframe_sessions,
    )
    .await?;
    let url = mgr.get_url().await.unwrap_or_default();
    Ok(json!({ "value": value, "origin": url }))
}

pub(super) async fn handle_isvisible(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let visible = crate::native::element::is_element_visible(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    let url = mgr.get_url().await.unwrap_or_default();
    Ok(json!({ "visible": visible, "origin": url }))
}

pub(super) async fn handle_isenabled(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let enabled = crate::native::element::is_element_enabled(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    let url = mgr.get_url().await.unwrap_or_default();
    Ok(json!({ "enabled": enabled, "origin": url }))
}

pub(super) async fn handle_ischecked(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let checked = crate::native::element::is_element_checked(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    let url = mgr.get_url().await.unwrap_or_default();
    Ok(json!({ "checked": checked, "origin": url }))
}

pub(super) async fn handle_boundingbox(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let bbox = crate::native::element::get_element_bounding_box(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(bbox)
}

pub(super) async fn handle_innertext(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let text = crate::native::element::get_element_inner_text(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "text": text }))
}

pub(super) async fn handle_innerhtml(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let html = crate::native::element::get_element_inner_html(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "html": html }))
}

pub(super) async fn handle_inputvalue(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let value = crate::native::element::get_element_input_value(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "value": value }))
}

pub(super) async fn handle_count(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let count =
        crate::native::element::get_element_count(&mgr.client, &session_id, selector).await?;
    Ok(json!({ "count": count, "selector": selector }))
}

pub(super) async fn handle_styles(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let properties = cmd.get("properties").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    });

    let styles = crate::native::element::get_element_styles(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        properties,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "styles": styles }))
}
