use super::*;

pub(super) async fn handle_trace_start(state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    native_tracing::trace_start(&mgr.client, &session_id, &mut state.tracing_state).await
}

pub(super) async fn handle_trace_stop(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let path = cmd.get("path").and_then(|v| v.as_str());
    native_tracing::trace_stop(&mgr.client, &session_id, &mut state.tracing_state, path).await
}

pub(super) async fn handle_profiler_start(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let categories = cmd.get("categories").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    });
    native_tracing::profiler_start(
        &mgr.client,
        &session_id,
        &mut state.tracing_state,
        categories,
    )
    .await
}

pub(super) async fn handle_profiler_stop(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let path = cmd.get("path").and_then(|v| v.as_str());
    native_tracing::profiler_stop(&mgr.client, &session_id, &mut state.tracing_state, path).await
}

/// Read an optional `fps` field from a recording command, rejecting rates the
/// recorder cannot honor before any browser work happens.
pub(super) fn recording_fps_from_command(cmd: &Value) -> Result<Option<u32>, String> {
    match cmd.get("fps") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value
                .as_u64()
                .filter(|v| *v <= u32::MAX as u64)
                .ok_or_else(|| format!("Invalid fps: {} is not a positive integer", value))?;
            recording::validate_fps(raw as u32).map(Some)
        }
    }
}

pub(super) fn recording_options_from_command(
    cmd: &Value,
) -> Result<recording::RecordingOptions, String> {
    let contact_sheet_threshold = match cmd.get("contactSheetThreshold") {
        Some(value) => value
            .as_f64()
            .ok_or_else(|| format!("Invalid contact sheet threshold: {} is not a number", value))?,
        None => recording::DEFAULT_CONTACT_SHEET_THRESHOLD,
    };
    recording::validate_contact_sheet_threshold(contact_sheet_threshold)?;
    Ok(recording::RecordingOptions {
        fps: recording_fps_from_command(cmd)?,
        cursor: cmd.get("cursor").and_then(Value::as_bool).unwrap_or(false),
        contact_sheet: cmd
            .get("contactSheet")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || cmd.get("contactSheetThreshold").is_some(),
        contact_sheet_threshold,
    })
}

/// Start recording the current active page. The recorder attaches to the
/// active session as-is: no new browser context, no new tab, and no
/// navigation unless a URL is given (in which case the active tab navigates
/// there first). Capture therefore starts on an already-hydrated page instead
/// of at `load` on a cold navigation. Use `tab new` first to record in a
/// separate tab.
pub(super) async fn handle_recording_start(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    // Reject before creating a context or changing the active page.
    if state.recording_state.active {
        return Err("Recording already active".to_string());
    }

    let path = cmd
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' parameter")?;

    let recording_url = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    // Validate the rate and output path before any browser work so a bad value
    // costs nothing.
    let options = recording_options_from_command(cmd)?;
    recording::validate_output_path(path)?;

    {
        let domain_filter = state.domain_filter.read().await;
        if let Some(url) = recording_url {
            check_url_allowed_by_filter(domain_filter.as_ref(), url)?;
        }
    }

    if state.recording_state.active {
        return Err("Recording already active".to_string());
    }
    if state.browser.is_none() {
        return Err("Browser not launched".to_string());
    }

    // Check the external dependency before an optional navigation mutates the
    // active page. State is still inactive, so a failure needs no rollback.
    recording::check_ffmpeg_available().await?;

    if let Some(url) = recording_url {
        navigate_active_page(state, url, WaitUntil::Load).await?;
    }
    let (client, session_id) = {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        (mgr.client.clone(), mgr.active_session_id()?.to_string())
    };

    let result = recording::recording_start(&mut state.recording_state, path, options)?;
    state.start_recording_task(client, session_id).await?;

    if let Some(ref server) = state.stream_server {
        server.set_recording(true, &state.engine).await;
    }

    Ok(result)
}

pub(super) async fn handle_recording_stop(state: &mut DaemonState) -> Result<Value, String> {
    // Clear the recording state even when the capture task failed, so a
    // broken take does not block the next `record start`.
    let task_result = state.stop_recording_task().await;
    let result = recording::recording_stop(&mut state.recording_state);
    task_result?;

    if let Some(ref server) = state.stream_server {
        server.set_recording(false, &state.engine).await;
    }

    result
}

pub(super) async fn handle_recording_restart(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let path = cmd
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' parameter")?;
    let recording_url = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    // Validate the path and rate before stopping the in-flight take.
    recording::validate_output_path(path)?;
    let options = recording_options_from_command(cmd)?;

    {
        let domain_filter = state.domain_filter.read().await;
        if let Some(ref url) = recording_url {
            check_url_allowed_by_filter(domain_filter.as_ref(), url)?;
        }
    }

    // Preserve the in-flight take when the replacement cannot start. A
    // restart without a browser keeps its existing state-only behavior.
    if state.browser.is_some() {
        recording::check_ffmpeg_available().await?;
    }

    let _ = state.stop_recording_task().await;
    let previous_path = if state.recording_state.active {
        recording::recording_stop(&mut state.recording_state)
            .ok()
            .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(String::from))
    } else {
        None
    };

    let recording_target = if state.browser.is_some() {
        if let Some(ref url) = recording_url {
            navigate_active_page(state, url, WaitUntil::Load).await?;
        }
        let browser = state.browser.as_ref().ok_or("Browser not launched")?;
        Some((
            browser.client.clone(),
            browser.active_session_id()?.to_string(),
        ))
    } else {
        None
    };

    recording::recording_start(&mut state.recording_state, path, options)?;

    if let Some((client, session_id)) = recording_target {
        state.start_recording_task(client, session_id).await?;
    }

    Ok(json!({
        "restarted": true,
        "previousPath": previous_path,
        "path": path,
        "fps": state.recording_state.fps,
        "cursor": state.recording_state.cursor,
        "contactSheet": state.recording_state.contact_sheet,
        "contactSheetPath": state.recording_state.contact_sheet_path,
    }))
}

pub(super) async fn handle_screencast_start(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    if state.screencasting {
        return Err("Screencast already active".to_string());
    }

    // Use stored viewport as default for screencast dimensions
    // Quality only. The frame-size env vars are deliberately not applied here:
    // this command reports its dimensions as the viewport in the status
    // broadcast, and clients scale pointer coordinates by that, so a smaller
    // encode would move every click.
    let env_cfg = stream::ScreencastConfig::from_env();
    let (default_w, default_h) = if let Some(ref server) = state.stream_server {
        server.viewport().await
    } else {
        (1280, 720)
    };
    let format = cmd.get("format").and_then(|v| v.as_str()).unwrap_or("jpeg");
    let quality = cmd
        .get("quality")
        .and_then(|v| v.as_i64())
        .map(|q| q as i32)
        .unwrap_or(env_cfg.quality);
    let max_width = cmd
        .get("maxWidth")
        .and_then(|v| v.as_i64())
        .unwrap_or(default_w as i64) as i32;
    let max_height = cmd
        .get("maxHeight")
        .and_then(|v| v.as_i64())
        .unwrap_or(default_h as i64) as i32;

    stream::start_screencast(
        &mgr.client,
        &session_id,
        format,
        quality,
        max_width,
        max_height,
    )
    .await?;
    state.screencasting = true;

    if let Some(ref server) = state.stream_server {
        server.set_screencasting(true).await;
        server
            .broadcast_status(
                true,
                true,
                max_width as u32,
                max_height as u32,
                &state.engine,
            )
            .await;
    }

    Ok(json!({ "started": true }))
}

pub(super) async fn handle_screencast_stop(state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?;

    if !state.screencasting {
        return Err("No screencast active".to_string());
    }

    stream::stop_screencast(&mgr.client, session_id).await?;
    state.screencasting = false;

    if let Some(ref server) = state.stream_server {
        server.set_screencasting(false).await;
        let (vw, vh) = server.viewport().await;
        server
            .broadcast_status(true, false, vw, vh, &state.engine)
            .await;
    }

    Ok(json!({ "stopped": true }))
}

// ---------------------------------------------------------------------------
// Wait variant handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_video_start(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let path = cmd
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' parameter")?;

    if state.recording_state.active {
        return Err("A recording is already in progress".to_string());
    }

    let fps = recording_fps_from_command(cmd)?;
    recording::validate_output_path(path)?;

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    recording::check_ffmpeg_available().await?;
    recording::recording_start(
        &mut state.recording_state,
        path,
        recording::RecordingOptions {
            fps,
            ..recording::RecordingOptions::default()
        },
    )?;
    state
        .start_recording_task(mgr.client.clone(), session_id)
        .await?;

    Ok(json!({
        "started": true,
        "fps": state.recording_state.fps,
        "note": "Video recording started. Use video_stop to save the recording."
    }))
}

pub(super) async fn handle_video_stop(state: &mut DaemonState) -> Result<Value, String> {
    if !state.recording_state.active {
        return Ok(json!({
            "stopped": false,
            "note": "No video recording was started. Use recording_stop if you used recording_start."
        }));
    }

    state.stop_recording_task().await?;
    recording::recording_stop(&mut state.recording_state)
}
