use super::*;

pub(super) fn record_click_animation(
    result: &interaction::ClickResult,
    mouse: &mut MouseState,
    history: &recording::SharedRecordingCursor,
) {
    mouse.x = result.x;
    mouse.y = result.y;
    mouse.buttons = i32::from(result.pending_release.is_some());
    if let Ok(mut cursor) = history.lock() {
        cursor.record(result.x, result.y, 0);
        if result.button_pressed {
            cursor.record(result.x, result.y, 1);
            if result.pending_release.is_none() {
                cursor.record(result.x, result.y, 0);
            }
        }
    }
}

pub(super) async fn handle_click(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            wb.click(selector).await?;
            return Ok(json!({ "clicked": selector }));
        }
    }

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let new_tab = cmd.get("newTab").and_then(|v| v.as_bool()).unwrap_or(false);

    if new_tab {
        use crate::native::element::resolve_element_object_id;
        let (object_id, effective_session_id) = resolve_element_object_id(
            &mgr.client,
            &session_id,
            &state.ref_map,
            selector,
            &state.iframe_sessions,
        )
        .await?;
        let call_params = json!({
            "objectId": object_id,
            "functionDeclaration": "function() { var h = this.getAttribute('href'); if (!h) return null; try { return new URL(h, document.baseURI).toString(); } catch(e) { return null; } }",
            "returnByValue": true
        });
        let call_result = mgr
            .client
            .send_command(
                "Runtime.callFunctionOn",
                Some(call_params),
                Some(&effective_session_id),
            )
            .await?;
        let href = call_result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                format!(
                    "Element '{}' does not have an href attribute. --new-tab only works on links.",
                    selector
                )
            })?
            .to_string();

        let domain_filter = state.domain_filter.read().await.clone();
        let has_proxy_creds = state.proxy_credentials.read().await.is_some();
        let defer_url_until_controls = should_defer_url_until_network_controls(
            domain_filter.as_ref(),
            has_proxy_creds,
            Some(&href),
        )?;
        // `click --new-tab` creates the same kind of daemon-owned tab as
        // `tab new`, so it must use the same pre-navigation setup path.
        let defer_url = defer_url_until_controls || session_setup_pending(state).await;

        state.ref_map.begin_snapshot();
        state.active_iframe_sessions.clear();
        state.active_frame_id = None;
        state.webmcp.clear_invocations();
        let new_session_id = {
            let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
            mgr.tab_new(if defer_url { None } else { Some(&href) }, None)
                .await?;
            mgr.active_session_id()?.to_string()
        };

        apply_session_setup(state, &new_session_id).await?;
        install_network_controls_or_close(state, has_proxy_creds).await?;
        state.drain_cdp_events_background().await?;

        if defer_url {
            let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
            mgr.navigate(&href, WaitUntil::Load).await?;
        }

        state.refresh_active_iframe_sessions().await;

        return Ok(json!({ "clicked": selector, "newTab": true, "url": href }));
    }

    let button = cmd.get("button").and_then(|v| v.as_str()).unwrap_or("left");
    let click_count = cmd.get("clickCount").and_then(|v| v.as_i64()).unwrap_or(1) as i32;

    let input_mode = cmd
        .get("inputMode")
        .and_then(Value::as_str)
        .unwrap_or(&state.input_mode);
    if input_mode != "instant" {
        let (x, y, target_session_id) = crate::native::element::resolve_element_center(
            &mgr.client,
            &session_id,
            &state.ref_map,
            selector,
            &state.iframe_sessions,
            true,
        )
        .await?;
        let offset = crate::native::element::session_viewport_offset(
            &mgr.client,
            &session_id,
            &target_session_id,
            &state.iframe_sessions,
        )
        .await?;
        move_mouse_interpolated(
            &mgr.client,
            &target_session_id,
            &mut state.mouse_state,
            x,
            y,
            if input_mode == "smooth" { 200 } else { 0 },
            None,
            input_mode == "human",
            cmd.get("seed").and_then(Value::as_u64).unwrap_or(0),
            0,
            offset,
            &state.recording_state.shared_cursor,
        )
        .await?;
    }

    let result = interaction::click(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        button,
        click_count,
        &state.iframe_sessions,
    )
    .await?;
    record_click_animation(
        &result,
        &mut state.mouse_state,
        &state.recording_state.shared_cursor,
    );

    if result.dialog_opened {
        state.pending_pointer_release = result.pending_release;
        return Ok(json!({ "clicked": selector, "dialogOpened": true }));
    }
    Ok(json!({ "clicked": selector }))
}

pub(super) async fn handle_dblclick(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let result = interaction::dblclick(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    record_click_animation(
        &result,
        &mut state.mouse_state,
        &state.recording_state.shared_cursor,
    );
    if result.dialog_opened {
        state.pending_pointer_release = result.pending_release;
        return Ok(json!({ "clicked": selector, "dialogOpened": true }));
    }
    Ok(json!({ "clicked": selector }))
}

pub(super) async fn handle_fill(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let value = cmd
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'value' parameter")?;

    if let Some(ref wb) = state.webdriver_backend {
        if state.browser.is_none() {
            wb.fill(selector, value).await?;
            return Ok(json!({ "filled": selector }));
        }
    }

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    interaction::fill(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        value,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "filled": selector }))
}

pub(super) async fn handle_type(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let text = cmd
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'text' parameter")?;
    let clear = cmd.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
    let delay = cmd.get("delay").and_then(|v| v.as_u64());

    interaction::type_text(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        text,
        clear,
        delay,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "typed": text }))
}

pub(super) async fn handle_press(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let key = cmd
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'key' parameter")?;

    // Parse modifier+key chords like "Control+a", "Shift+Enter", "Control+Shift+a"
    let (actual_key, modifiers) = parse_key_chord(key);

    interaction::press_key_with_modifiers(&mgr.client, &session_id, &actual_key, modifiers).await?;
    Ok(json!({ "pressed": key }))
}

/// Parse a key chord string like "Control+a" or "Control+Shift+Enter" into
/// the actual key name and an optional CDP modifier bitmask.
///
/// CDP modifier values: 1 = Alt, 2 = Control, 4 = Meta (Cmd), 8 = Shift.
pub(super) fn parse_key_chord(input: &str) -> (String, Option<i32>) {
    let parts: Vec<&str> = input.split('+').collect();
    if parts.len() < 2 {
        return (input.to_string(), None);
    }

    let mut modifiers = 0i32;
    let mut key_parts: Vec<&str> = Vec::new();

    for part in &parts {
        match part.to_lowercase().as_str() {
            "alt" => modifiers |= 1,
            "control" | "ctrl" => modifiers |= 2,
            "meta" | "cmd" | "command" => modifiers |= 4,
            "shift" => modifiers |= 8,
            _ => key_parts.push(part),
        }
    }

    // If no modifiers were found, the '+' was part of the key name (e.g. "+")
    // or the input was something unexpected — treat the whole string as the key.
    if modifiers == 0 {
        return (input.to_string(), None);
    }

    // The actual key is whatever remains after stripping modifiers.
    // If nothing remains (e.g. "Control+"), treat the whole string as-is.
    let actual_key = if key_parts.is_empty() {
        input.to_string()
    } else {
        key_parts.join("+")
    };

    (actual_key, Some(modifiers))
}

pub(super) async fn handle_hover(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let position = interaction::hover(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    (state.mouse_state.x, state.mouse_state.y) = position;
    if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
        cursor.record(position.0, position.1, state.mouse_state.buttons);
    }
    Ok(json!({ "hovered": selector }))
}

pub(super) async fn handle_scroll(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd.get("selector").and_then(|v| v.as_str());

    let (mut dx, mut dy) = (
        cmd.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0),
        cmd.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0),
    );

    if let Some(direction) = cmd.get("direction").and_then(|v| v.as_str()) {
        let amount = cmd.get("amount").and_then(|v| v.as_f64()).unwrap_or(300.0);
        match direction {
            "up" => dy = -amount,
            "down" => dy = amount,
            "left" => dx = -amount,
            "right" => dx = amount,
            _ => {}
        }
    }

    interaction::scroll(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        dx,
        dy,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "scrolled": true }))
}

pub(super) async fn handle_select(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let values: Vec<String> = match cmd.get("values") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => cmd
            .get("value")
            .and_then(|v| v.as_str())
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
    };

    interaction::select_option(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &values,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "selected": values }))
}

pub(super) async fn handle_check(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    if let Some(position) = interaction::check(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?
    {
        (state.mouse_state.x, state.mouse_state.y) = position;
        if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
            cursor.record(position.0, position.1, state.mouse_state.buttons);
        }
    }
    Ok(json!({ "checked": selector }))
}

pub(super) async fn handle_uncheck(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    if let Some(position) = interaction::uncheck(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?
    {
        (state.mouse_state.x, state.mouse_state.y) = position;
        if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
            cursor.record(position.0, position.1, state.mouse_state.buttons);
        }
    }
    Ok(json!({ "unchecked": selector }))
}

pub(super) async fn handle_mouse(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let event_type = cmd
        .get("eventType")
        .and_then(|v| v.as_str())
        .unwrap_or("mouseMoved");
    let x = cmd.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = cmd.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let button = cmd.get("button").and_then(|v| v.as_str()).unwrap_or("none");
    let click_count = cmd.get("clickCount").and_then(|v| v.as_i64()).unwrap_or(0);

    mgr.client
        .send_command(
            "Input.dispatchMouseEvent",
            Some(json!({
                "type": event_type,
                "x": x,
                "y": y,
                "button": button,
                "clickCount": click_count,
            })),
            Some(&session_id),
        )
        .await?;
    state.mouse_state.x = x;
    state.mouse_state.y = y;
    match event_type {
        "mousePressed" => state.mouse_state.buttons |= mouse_button_mask(button),
        "mouseReleased" => state.mouse_state.buttons &= !mouse_button_mask(button),
        _ => {}
    }
    if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
        cursor.record(x, y, i32::from(event_type == "mousePressed"));
    }

    Ok(json!({ "dispatched": event_type }))
}

pub(super) async fn handle_keyboard(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    match cmd.get("subaction").and_then(|v| v.as_str()) {
        Some("type") => {
            let text = cmd
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'text' parameter")?;
            interaction::type_text_into_active_context(&mgr.client, &session_id, text, None)
                .await?;
            return Ok(json!({ "typed": text }));
        }
        Some("insertText") => {
            let text = cmd
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'text' parameter")?;
            mgr.client
                .send_command(
                    "Input.insertText",
                    Some(json!({ "text": text })),
                    Some(&session_id),
                )
                .await?;
            return Ok(json!({ "inserted": true }));
        }
        _ => {}
    }

    let event_type = cmd
        .get("eventType")
        .and_then(|v| v.as_str())
        .unwrap_or("keyDown");
    let key = cmd.get("key").and_then(|v| v.as_str());
    let code = cmd.get("code").and_then(|v| v.as_str());
    let text = cmd.get("text").and_then(|v| v.as_str());

    let mut params = json!({ "type": event_type });
    if let Some(k) = key {
        params["key"] = Value::String(k.to_string());
    }
    if let Some(c) = code {
        params["code"] = Value::String(c.to_string());
    }
    if let Some(t) = text {
        params["text"] = Value::String(t.to_string());
    }

    mgr.client
        .send_command("Input.dispatchKeyEvent", Some(params), Some(&session_id))
        .await?;

    Ok(json!({ "dispatched": event_type }))
}

// ---------------------------------------------------------------------------
// Phase 5 handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_focus(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    interaction::focus(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "focused": selector }))
}

pub(super) async fn handle_clear(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    interaction::clear(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "cleared": selector }))
}

pub(super) async fn handle_selectall(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    interaction::select_all(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "selected": selector }))
}

pub(super) async fn handle_scrollintoview(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    interaction::scroll_into_view(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "scrolled": selector }))
}

pub(super) async fn handle_dispatch(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let event_type = cmd
        .get("event")
        .or_else(|| cmd.get("eventType"))
        .and_then(|v| v.as_str())
        .ok_or("Missing 'event' parameter")?;
    let event_init = cmd.get("eventInit");

    interaction::dispatch_event(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        event_type,
        event_init,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "dispatched": event_type, "selector": selector }))
}

pub(super) async fn handle_highlight(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    interaction::highlight(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "highlighted": selector }))
}

pub(super) async fn handle_tap(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let selector = cmd.get("selector").and_then(|v| v.as_str());

    // Route through Appium for iOS/WebDriver using coordinate-based tap
    if let Some(ref appium) = state.appium {
        if state.browser.is_none() {
            let x = cmd.get("x").and_then(|v| v.as_f64()).unwrap_or(200.0);
            let y = cmd.get("y").and_then(|v| v.as_f64()).unwrap_or(200.0);
            appium.tap(x, y).await?;
            return Ok(json!({ "tapped": true, "x": x, "y": y }));
        }
    }

    let sel = selector.ok_or("Missing 'selector' parameter")?;
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    interaction::tap_touch(
        &mgr.client,
        &session_id,
        &state.ref_map,
        sel,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "tapped": sel }))
}

pub(super) async fn handle_setvalue(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let value = cmd
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'value' parameter")?;

    crate::native::element::set_element_value(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        value,
        &state.iframe_sessions,
    )
    .await?;
    Ok(json!({ "set": selector, "value": value }))
}

/// Dispatches a wheel event and updates the tracked pointer only after CDP accepts it.
pub(super) async fn handle_wheel(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let delta_x = cmd.get("deltaX").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let delta_y = cmd.get("deltaY").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut mouse_state = state.mouse_state;
    let params = build_mouse_event_params(
        &mut mouse_state,
        "mouseWheel",
        cmd.get("x").and_then(|v| v.as_f64()),
        cmd.get("y").and_then(|v| v.as_f64()),
        None,
        None,
        None,
        Some(delta_x),
        Some(delta_y),
        None,
    );

    mgr.client
        .send_command_typed::<_, Value>("Input.dispatchMouseEvent", &params, Some(&session_id))
        .await?;
    state.mouse_state = mouse_state;

    Ok(json!({ "scrolled": true, "deltaX": delta_x, "deltaY": delta_y }))
}

pub(super) async fn handle_drag(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let source = cmd
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'source' parameter")?;
    let target = cmd
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'target' parameter")?;

    let (sx, sy, source_session_id) = crate::native::element::resolve_element_center(
        &mgr.client,
        &session_id,
        &state.ref_map,
        source,
        &state.iframe_sessions,
        false,
    )
    .await?;
    let (tx, ty, target_session_id) = crate::native::element::resolve_element_center(
        &mgr.client,
        &session_id,
        &state.ref_map,
        target,
        &state.iframe_sessions,
        false,
    )
    .await?;

    let mode = cmd
        .get("inputMode")
        .and_then(Value::as_str)
        .unwrap_or(&state.input_mode);
    let human = mode == "human";
    let seed = cmd.get("seed").and_then(Value::as_u64).unwrap_or(0);

    // Approach the source before pressing so hover and pointer path handlers fire.
    let source_offset = crate::native::element::session_viewport_offset(
        &mgr.client,
        &session_id,
        &source_session_id,
        &state.iframe_sessions,
    )
    .await?;
    let target_offset = crate::native::element::session_viewport_offset(
        &mgr.client,
        &session_id,
        &target_session_id,
        &state.iframe_sessions,
    )
    .await?;
    move_mouse_interpolated(
        &mgr.client,
        &source_session_id,
        &mut state.mouse_state,
        sx,
        sy,
        0,
        if human { None } else { Some(1) },
        human,
        seed,
        0,
        source_offset,
        &state.recording_state.shared_cursor,
    )
    .await?;
    mgr.client
        .send_command(
            "Input.dispatchMouseEvent",
            Some(json!({ "type": "mousePressed", "x": sx, "y": sy, "button": "left", "buttons": 1, "clickCount": 1 })),
            Some(&source_session_id),
        )
        .await?;
    if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
        cursor.record(sx + source_offset.0, sy + source_offset.1, 1);
    }

    // Move in steps to target, keeping the left button held (buttons: 1) so
    // that the browser sees a drag rather than a plain pointer move.
    state.mouse_state.x = sx + source_offset.0;
    state.mouse_state.y = sy + source_offset.1;
    state.mouse_state.buttons = 1;
    move_mouse_interpolated(
        &mgr.client,
        &target_session_id,
        &mut state.mouse_state,
        tx,
        ty,
        if human { 250 } else { 100 },
        if human { None } else { Some(10) },
        human,
        seed.wrapping_add(1),
        1,
        target_offset,
        &state.recording_state.shared_cursor,
    )
    .await?;

    // Mouse up at target
    mgr.client
        .send_command(
            "Input.dispatchMouseEvent",
            Some(json!({ "type": "mouseReleased", "x": tx, "y": ty, "button": "left", "buttons": 0, "clickCount": 1 })),
            Some(&target_session_id),
        )
        .await?;
    state.mouse_state.buttons = 0;
    if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
        cursor.record(tx + target_offset.0, ty + target_offset.1, 0);
    }

    Ok(json!({ "dragged": true, "source": source, "target": target }))
}

pub(super) async fn handle_multiselect(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let values: Vec<String> = cmd
        .get("values")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let values_json = serde_json::to_string(&values).unwrap_or("[]".to_string());
    // A locator miss returns a sentinel instead of throwing, so it is detected in
    // Rust and normalized to the anchored "No element found: ..." shape. A thrown
    // error surfaces as "Evaluation error: ...", which is_locator_miss skips, and
    // a sentinel avoids misclassifying an invalid-selector error too.
    let js = format!(
        r#"(() => {{
            const select = document.querySelector({sel});
            if (!select) return {{ __ab_miss: true }};
            const vals = {vals};
            for (const opt of select.options) {{
                opt.selected = vals.includes(opt.value);
            }}
            select.dispatchEvent(new Event('change', {{ bubbles: true }}));
            return {{ selected: Array.from(select.selectedOptions).map(o => o.value) }};
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default(),
        vals = values_json,
    );

    let result = mgr.evaluate(&js, None).await?;
    if result
        .get("__ab_miss")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(format!("No element found: {selector}"));
    }
    let selected = result.get("selected").cloned().unwrap_or_else(|| json!([]));
    Ok(json!({ "selected": selected }))
}

pub(super) async fn handle_swipe(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    // Route through Appium for iOS/WebDriver
    if let Some(ref appium) = state.appium {
        if state.browser.is_none() {
            let start_x = cmd.get("startX").and_then(|v| v.as_f64()).unwrap_or(200.0);
            let start_y = cmd.get("startY").and_then(|v| v.as_f64()).unwrap_or(400.0);
            let end_x = cmd.get("endX").and_then(|v| v.as_f64()).unwrap_or(200.0);
            let end_y = cmd.get("endY").and_then(|v| v.as_f64()).unwrap_or(100.0);

            if let Some(direction) = cmd.get("direction").and_then(|v| v.as_str()) {
                let distance = cmd
                    .get("distance")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(300.0);
                let (dx, dy) = match direction {
                    "up" => (0.0, -distance),
                    "down" => (0.0, distance),
                    "left" => (-distance, 0.0),
                    "right" => (distance, 0.0),
                    _ => (0.0, -distance),
                };
                let actual_end_x = start_x + dx;
                let actual_end_y = start_y + dy;
                let duration = cmd.get("duration").and_then(|v| v.as_u64()).unwrap_or(800);
                appium
                    .swipe(start_x, start_y, actual_end_x, actual_end_y, duration)
                    .await?;
                return Ok(json!({ "swiped": direction }));
            }

            let duration = cmd.get("duration").and_then(|v| v.as_u64()).unwrap_or(800);
            appium
                .swipe(start_x, start_y, end_x, end_y, duration)
                .await?;
            return Ok(json!({ "swiped": true, "from": [start_x, start_y], "to": [end_x, end_y] }));
        }
    }

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let start_x = cmd.get("startX").and_then(|v| v.as_f64()).unwrap_or(200.0);
    let start_y = cmd.get("startY").and_then(|v| v.as_f64()).unwrap_or(400.0);
    let end_x = cmd.get("endX").and_then(|v| v.as_f64()).unwrap_or(200.0);
    let end_y = cmd.get("endY").and_then(|v| v.as_f64()).unwrap_or(100.0);

    if let Some(direction) = cmd.get("direction").and_then(|v| v.as_str()) {
        let distance = cmd
            .get("distance")
            .and_then(|v| v.as_f64())
            .unwrap_or(300.0);
        let (dx, dy) = match direction {
            "up" => (0.0, -distance),
            "down" => (0.0, distance),
            "left" => (-distance, 0.0),
            "right" => (distance, 0.0),
            _ => (0.0, -distance),
        };
        let cx = start_x;
        let cy = start_y;

        mgr.client
            .send_command(
                "Input.dispatchTouchEvent",
                Some(json!({ "type": "touchStart", "touchPoints": [{ "x": cx, "y": cy }] })),
                Some(&session_id),
            )
            .await?;

        let steps = 10;
        for i in 1..=steps {
            let x = cx + dx * (i as f64) / (steps as f64);
            let y = cy + dy * (i as f64) / (steps as f64);
            mgr.client
                .send_command(
                    "Input.dispatchTouchEvent",
                    Some(json!({ "type": "touchMove", "touchPoints": [{ "x": x, "y": y }] })),
                    Some(&session_id),
                )
                .await?;
            tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
        }

        mgr.client
            .send_command(
                "Input.dispatchTouchEvent",
                Some(json!({ "type": "touchEnd", "touchPoints": [] })),
                Some(&session_id),
            )
            .await?;

        return Ok(json!({ "swiped": direction }));
    }

    // Manual coordinates
    mgr.client
        .send_command(
            "Input.dispatchTouchEvent",
            Some(json!({ "type": "touchStart", "touchPoints": [{ "x": start_x, "y": start_y }] })),
            Some(&session_id),
        )
        .await?;

    let steps = 10;
    for i in 1..=steps {
        let x = start_x + (end_x - start_x) * (i as f64) / (steps as f64);
        let y = start_y + (end_y - start_y) * (i as f64) / (steps as f64);
        mgr.client
            .send_command(
                "Input.dispatchTouchEvent",
                Some(json!({ "type": "touchMove", "touchPoints": [{ "x": x, "y": y }] })),
                Some(&session_id),
            )
            .await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
    }

    mgr.client
        .send_command(
            "Input.dispatchTouchEvent",
            Some(json!({ "type": "touchEnd", "touchPoints": [] })),
            Some(&session_id),
        )
        .await?;

    Ok(json!({ "swiped": true, "from": [start_x, start_y], "to": [end_x, end_y] }))
}

pub(super) fn mouse_button_mask(button: &str) -> i32 {
    match button {
        "left" => 1,
        "right" => 2,
        "middle" => 4,
        "back" => 8,
        "forward" => 16,
        _ => 0,
    }
}

pub(super) fn primary_button_from_mask(buttons: i32) -> &'static str {
    if buttons & 1 != 0 {
        "left"
    } else if buttons & 2 != 0 {
        "right"
    } else if buttons & 4 != 0 {
        "middle"
    } else if buttons & 8 != 0 {
        "back"
    } else if buttons & 16 != 0 {
        "forward"
    } else {
        "none"
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_mouse_event_params(
    mouse_state: &mut MouseState,
    event_type: &str,
    x: Option<f64>,
    y: Option<f64>,
    button: Option<&str>,
    buttons: Option<i32>,
    click_count: Option<i32>,
    delta_x: Option<f64>,
    delta_y: Option<f64>,
    modifiers: Option<i32>,
) -> DispatchMouseEventParams {
    let x = x.unwrap_or(mouse_state.x);
    let y = y.unwrap_or(mouse_state.y);
    mouse_state.x = x;
    mouse_state.y = y;

    let mut next_buttons = buttons.unwrap_or(mouse_state.buttons);
    if buttons.is_none() {
        match event_type {
            "mousePressed" => {
                next_buttons |= mouse_button_mask(button.unwrap_or("left"));
            }
            "mouseReleased" => {
                next_buttons &= !mouse_button_mask(button.unwrap_or("left"));
            }
            _ => {}
        }
    }
    mouse_state.buttons = next_buttons;

    DispatchMouseEventParams {
        event_type: event_type.to_string(),
        x,
        y,
        button: Some(
            button
                .unwrap_or(primary_button_from_mask(next_buttons))
                .to_string(),
        ),
        buttons: Some(next_buttons),
        click_count,
        delta_x,
        delta_y,
        modifiers,
    }
}

pub(super) async fn handle_input_mouse(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let event_type = cmd
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("mouseMoved");
    let mut mouse_state = state.mouse_state;
    let params = build_mouse_event_params(
        &mut mouse_state,
        event_type,
        cmd.get("x").and_then(|v| v.as_f64()),
        cmd.get("y").and_then(|v| v.as_f64()),
        cmd.get("button").and_then(|v| v.as_str()),
        cmd.get("buttons")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        cmd.get("clickCount")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        cmd.get("deltaX").and_then(|v| v.as_f64()),
        cmd.get("deltaY").and_then(|v| v.as_f64()),
        cmd.get("modifiers")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
    );

    mgr.client
        .send_command_typed::<_, Value>("Input.dispatchMouseEvent", &params, Some(&session_id))
        .await?;
    state.mouse_state = mouse_state;
    if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
        cursor.record(mouse_state.x, mouse_state.y, mouse_state.buttons);
    }
    Ok(json!({ "dispatched": event_type }))
}

pub(super) async fn handle_input_keyboard(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let event_type = cmd
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("keyDown");

    let mut params = json!({ "type": event_type });
    for key in &["key", "code", "text"] {
        if let Some(v) = cmd.get(*key) {
            params[*key] = v.clone();
        }
    }

    mgr.client
        .send_command("Input.dispatchKeyEvent", Some(params), Some(&session_id))
        .await?;
    Ok(json!({ "dispatched": event_type }))
}

pub(super) async fn handle_input_touch(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let event_type = cmd
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("touchStart");

    mgr.client
        .send_command(
            "Input.dispatchTouchEvent",
            Some(json!({
                "type": event_type,
                "touchPoints": cmd.get("touchPoints").unwrap_or(&json!([])),
            })),
            Some(&session_id),
        )
        .await?;
    Ok(json!({ "dispatched": event_type }))
}

pub(super) async fn handle_keydown(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let key = cmd
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'key' parameter")?;

    mgr.client
        .send_command(
            "Input.dispatchKeyEvent",
            Some(json!({ "type": "keyDown", "key": key })),
            Some(&session_id),
        )
        .await?;
    Ok(json!({ "keydown": key }))
}

pub(super) async fn handle_keyup(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let key = cmd
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'key' parameter")?;

    mgr.client
        .send_command(
            "Input.dispatchKeyEvent",
            Some(json!({ "type": "keyUp", "key": key })),
            Some(&session_id),
        )
        .await?;
    Ok(json!({ "keyup": key }))
}

pub(super) async fn handle_inserttext(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let text = cmd
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'text' parameter")?;

    mgr.client
        .send_command(
            "Input.insertText",
            Some(json!({ "text": text })),
            Some(&session_id),
        )
        .await?;
    Ok(json!({ "inserted": true }))
}

/// Move the session cursor along a deterministic eased curve. Human mode adds
/// a seeded perpendicular bend and samples the path frequently enough for
/// animation-heavy pages while preserving exact, reproducible endpoints.
/// Schedule steps against one clock so Chrome's response time counts toward
/// the requested duration instead of being added to every step's delay.
#[allow(clippy::too_many_arguments)]
pub(super) async fn move_mouse_interpolated(
    client: &CdpClient,
    session_id: &str,
    mouse_state: &mut MouseState,
    target_x: f64,
    target_y: f64,
    duration_ms: u64,
    requested_steps: Option<usize>,
    human: bool,
    seed: u64,
    buttons: i32,
    viewport_offset: (f64, f64),
    recording_cursor: &recording::SharedRecordingCursor,
) -> Result<(), String> {
    // Cursor state stays in page coordinates even when dispatching to an OOPIF.
    let start_x = mouse_state.x - viewport_offset.0;
    let start_y = mouse_state.y - viewport_offset.1;
    let dx = target_x - start_x;
    let dy = target_y - start_y;
    let distance = dx.hypot(dy);
    let duration_ms = if duration_ms == 0 && human {
        (80.0 + distance * 0.35).clamp(100.0, 700.0) as u64
    } else {
        duration_ms
    };
    let steps = interpolated_mouse_steps(distance, duration_ms, requested_steps, human);
    let bend = if human && distance > 0.0 {
        let mixed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let unit = ((mixed >> 11) as f64) / ((1_u64 << 53) as f64);
        (unit * 2.0 - 1.0) * (distance * 0.08).min(36.0)
    } else {
        0.0
    };
    let (perp_x, perp_y) = if distance > 0.0 {
        (-dy / distance, dx / distance)
    } else {
        (0.0, 0.0)
    };
    let schedule = if duration_ms == 0 {
        None
    } else {
        Some((
            tokio::time::Instant::now(),
            tokio::time::Duration::from_millis(duration_ms),
        ))
    };

    for i in 1..=steps {
        let (x, y) = interpolated_mouse_point(
            start_x, start_y, target_x, target_y, perp_x, perp_y, bend, i, steps,
        );
        let params = build_mouse_event_params(
            mouse_state,
            "mouseMoved",
            Some(x),
            Some(y),
            None,
            Some(buttons),
            None,
            None,
            None,
            None,
        );
        client
            .send_command_typed::<_, Value>("Input.dispatchMouseEvent", &params, Some(session_id))
            .await?;
        mouse_state.x = x + viewport_offset.0;
        mouse_state.y = y + viewport_offset.1;
        if let Ok(mut cursor) = recording_cursor.lock() {
            cursor.record(mouse_state.x, mouse_state.y, buttons);
        }
        if let Some((started, duration)) = schedule {
            tokio::time::sleep_until(started + duration.mul_f64(i as f64 / steps as f64)).await;
        }
    }
    Ok(())
}

pub(super) fn interpolated_mouse_steps(
    distance: f64,
    duration_ms: u64,
    requested_steps: Option<usize>,
    human: bool,
) -> usize {
    requested_steps
        .unwrap_or_else(|| {
            let spatial_steps = ((distance / 12.0).ceil() as usize).clamp(1, 60);
            if human && duration_ms > 0 {
                spatial_steps.max(duration_ms.div_ceil(16) as usize)
            } else {
                spatial_steps
            }
        })
        .clamp(1, 240)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn interpolated_mouse_point(
    start_x: f64,
    start_y: f64,
    target_x: f64,
    target_y: f64,
    perp_x: f64,
    perp_y: f64,
    bend: f64,
    step: usize,
    steps: usize,
) -> (f64, f64) {
    if step == steps {
        return (target_x, target_y);
    }
    let t = step as f64 / steps as f64;
    let eased = t * t * (3.0 - 2.0 * t);
    let curve = 4.0 * t * (1.0 - t) * bend;
    (
        start_x + (target_x - start_x) * eased + perp_x * curve,
        start_y + (target_y - start_y) * eased + perp_y * curve,
    )
}

pub(super) async fn handle_mousemove(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let x = cmd.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = cmd.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let duration = cmd.get("duration").and_then(Value::as_u64).unwrap_or(0);
    let mut steps = cmd.get("steps").and_then(Value::as_u64).map(|v| v as usize);
    let mode = cmd
        .get("inputMode")
        .and_then(Value::as_str)
        .unwrap_or(&state.input_mode);
    let human = mode == "human";
    if mode == "instant" && duration == 0 && steps.is_none() {
        steps = Some(1);
    }
    let seed = cmd.get("seed").and_then(Value::as_u64).unwrap_or(0);
    let buttons = state.mouse_state.buttons;
    move_mouse_interpolated(
        &mgr.client,
        &session_id,
        &mut state.mouse_state,
        x,
        y,
        duration,
        steps,
        human,
        seed,
        buttons,
        (0.0, 0.0),
        &state.recording_state.shared_cursor,
    )
    .await?;
    Ok(json!({ "moved": true, "x": x, "y": y }))
}

pub(super) async fn handle_mousedown(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let button = cmd.get("button").and_then(|v| v.as_str()).unwrap_or("left");
    let params = build_mouse_event_params(
        &mut state.mouse_state,
        "mousePressed",
        None,
        None,
        Some(button),
        None,
        Some(1),
        None,
        None,
        None,
    );

    mgr.client
        .send_command_typed::<_, Value>("Input.dispatchMouseEvent", &params, Some(&session_id))
        .await?;
    if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
        cursor.record(
            state.mouse_state.x,
            state.mouse_state.y,
            state.mouse_state.buttons,
        );
    }
    Ok(json!({ "pressed": true }))
}

pub(super) async fn handle_mouseup(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let button = cmd.get("button").and_then(|v| v.as_str()).unwrap_or("left");
    let params = build_mouse_event_params(
        &mut state.mouse_state,
        "mouseReleased",
        None,
        None,
        Some(button),
        None,
        Some(1),
        None,
        None,
        None,
    );

    mgr.client
        .send_command_typed::<_, Value>("Input.dispatchMouseEvent", &params, Some(&session_id))
        .await?;
    if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
        cursor.record(
            state.mouse_state.x,
            state.mouse_state.y,
            state.mouse_state.buttons,
        );
    }
    Ok(json!({ "released": true }))
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------
