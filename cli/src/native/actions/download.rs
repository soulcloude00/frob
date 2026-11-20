use super::*;

/// Chrome may still be flushing a download to disk after signalling
/// completion; wait briefly for the file to appear.
pub(super) async fn wait_for_downloaded_file(path: &std::path::Path) -> bool {
    for _ in 0..10 {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    path.exists()
}

/// `download`: clicks the selector and saves the resulting file at `path`.
/// Only a browser this daemon launched may name files on the local filesystem.
pub(super) async fn handle_download(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    if state.browser.as_ref().is_some_and(|mgr| mgr.is_shared()) {
        return Err("download is not available with --shared-browser".to_string());
    }
    if state
        .browser
        .as_ref()
        .is_some_and(|mgr| mgr.is_cdp_connection())
    {
        return Err(
            "download is not supported over --cdp, --auto-connect, or a provider: the remote endpoint names the saved file, so only a browser frob launched can save to a local path"
                .to_string(),
        );
    }
    let result = download_to_path(cmd, state).await;
    // `download` points the browser at the destination's directory; restore the
    // configured one so later downloads land where `wait --download` expects.
    if let Some(mgr) = state.browser.as_ref() {
        if !mgr.is_cdp_connection() {
            mgr.enable_download_events().await;
        }
    }
    result
}

pub(super) async fn download_to_path(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let path_str = cmd
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' parameter")?;

    // Resolve to absolute path and canonicalize to prevent path traversal
    let raw_dest = if std::path::Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {}", e))?
            .join(path_str)
    };

    // Extract directory and desired filename
    let download_dir = raw_dest
        .parent()
        .ok_or("Invalid download path: no parent directory")?
        .to_path_buf();

    // Create the directory if it doesn't exist
    std::fs::create_dir_all(&download_dir)
        .map_err(|e| format!("Failed to create download directory: {}", e))?;

    // Canonicalize after mkdir so the path actually exists for resolution
    let download_dir = download_dir
        .canonicalize()
        .map_err(|e| format!("Failed to resolve download directory: {}", e))?;
    let dest = download_dir.join(
        raw_dest
            .file_name()
            .ok_or("Invalid download path: no filename")?,
    );
    let download_dir_str = download_dir
        .to_str()
        .ok_or("Download directory path is not valid UTF-8")?;

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    // Set download behavior to save to the parent directory
    mgr.set_download_behavior(download_dir_str).await?;

    // Subscribe to CDP events before clicking so we don't miss the download event
    let mut rx = mgr.client.subscribe();

    // Click the element to trigger the download
    let result = interaction::click(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        "left",
        1,
        &state.iframe_sessions,
    )
    .await?;
    record_click_animation(
        &result,
        &mut state.mouse_state,
        &state.recording_state.shared_cursor,
    );

    // Wait for download to complete
    const DOWNLOAD_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_secs(30);
    let deadline = tokio::time::Instant::now() + DOWNLOAD_TIMEOUT;
    let mut downloaded_guid: Option<String> = None;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("Timeout waiting for download to complete".to_string());
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(event)) => {
                // Browser-domain download events may arrive without a sessionId
                // or with a different sessionId than the page session, so we
                // accept them regardless. Page-domain events are matched by
                // session to avoid cross-tab confusion.
                let is_page_session = event.session_id.as_deref() == Some(&session_id);
                let is_download_event = |method: &str, browser_method: &str, page_method: &str| {
                    method == browser_method || (method == page_method && is_page_session)
                };

                // Capture the GUID from downloadWillBegin
                if is_download_event(
                    &event.method,
                    "Browser.downloadWillBegin",
                    "Page.downloadWillBegin",
                ) {
                    if let Some(guid) = event.params.get("guid").and_then(|v| v.as_str()) {
                        downloaded_guid = Some(guid.to_string());
                    }
                }
                // Check for download completion or cancellation
                if is_download_event(
                    &event.method,
                    "Browser.downloadProgress",
                    "Page.downloadProgress",
                ) {
                    match event.params.get("state").and_then(|v| v.as_str()) {
                        Some("completed") => break,
                        Some("canceled") => {
                            return Err("Download was canceled".to_string());
                        }
                        _ => {}
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err("Event stream closed".to_string()),
            Err(_) => return Err("Timeout waiting for download to complete".to_string()),
        }
    }

    if let Some(ref guid) = downloaded_guid {
        state.downloads.mark_claimed(guid);
    }

    // With "allowAndName" behavior, Chrome saves the file using the GUID as filename.
    // Rename it to the user-requested filename.
    if let Some(guid) = downloaded_guid {
        let guid_path = guid_download_path(&download_dir, &guid)?;
        if wait_for_downloaded_file(&guid_path).await {
            if !may_move_download(&guid_path, true, Some(download_dir_str)) {
                return Err(format!(
                    "Refusing to move {}: it resolves outside {}",
                    guid_path.display(),
                    download_dir.display()
                ));
            }
            std::fs::rename(&guid_path, &dest)
                .map_err(|e| format!("Failed to rename downloaded file: {}", e))?;
        } else {
            // The file might have been saved under its original name instead
            // of the GUID (e.g. when Chrome falls back to "allow" behavior).
            if !dest.exists() {
                return Err(format!(
                    "Downloaded file not found at expected path (GUID: {})",
                    guid
                ));
            }
        }
    } else {
        // GUID capture failed -- the file may have been saved under its original name
        // by Chrome. Only return success if dest already exists (avoid touching
        // unrelated files in the directory).
        if !dest.exists() {
            return Err(
                "Download completed but could not determine the downloaded file name".to_string(),
            );
        }
    }

    let dest_str = dest.to_string_lossy().to_string();
    Ok(json!({ "path": dest_str }))
}

// ---------------------------------------------------------------------------
// Phase 4 handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_waitfordownload(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let timeout_ms = state.timeout_ms(cmd);
    let mut rx = state
        .browser
        .as_ref()
        .ok_or("Browser not launched")?
        .client
        .subscribe();
    // Events that arrived after the per-command drain, such as during the
    // liveness check, are not delivered to the new receiver. Drain them into
    // the ledger now so a download that already finished is claimed (#561).
    state.drain_cdp_events_background().await?;

    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
    let record = loop {
        if let Some(record) = state.downloads.claim_finished() {
            break record;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(event)) => {
                state.downloads.observe(&event.method, &event.params);
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err("Event stream closed".to_string()),
            Err(_) => return Err("Timeout waiting for download".to_string()),
        }
    };

    if record.state == DownloadState::Canceled {
        return Err("Download was canceled".to_string());
    }
    let path_arg = cmd.get("path").and_then(|v| v.as_str());
    let Some(saved) = record.file_path else {
        if path_arg.is_none() {
            return Ok(json!({ "completed": true }));
        }
        return Err(
            "Download completed but the browser did not report where it saved the file".to_string(),
        );
    };
    let saved = PathBuf::from(saved);
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let launched_locally = !mgr.is_cdp_connection();
    let download_dir = mgr.download_path.clone();
    // A CDP endpoint reports a path on its own filesystem, which may not be
    // this machine's.
    if launched_locally && !wait_for_downloaded_file(&saved).await {
        return Err(format!(
            "Download completed but no file is at {}",
            saved.display()
        ));
    }

    let Some(path_str) = path_arg else {
        return Ok(json!({ "path": saved.to_string_lossy() }));
    };
    if !may_move_download(&saved, launched_locally, download_dir.as_deref()) {
        state.downloads.release(&record.guid);
        return Err(format!(
            "Refusing to move {}: only downloads from a browser frob launched, inside its download directory, can be moved",
            saved.display()
        ));
    }
    // A bad destination must not consume the download, so a retry can claim it.
    move_download(&saved, path_str).inspect_err(|_| state.downloads.release(&record.guid))
}

/// Returns the file Chrome saved for `guid` in `download_dir`.
/// The GUID comes from a CDP event, so anything other than a single filename is rejected.
pub(super) fn guid_download_path(
    download_dir: &std::path::Path,
    guid: &str,
) -> Result<PathBuf, String> {
    let path = download_dir.join(guid);
    if path.file_name() != Some(std::ffi::OsStr::new(guid)) {
        return Err(format!(
            "Refusing to move downloaded file: invalid download GUID {:?}",
            guid
        ));
    }
    Ok(path)
}

/// The saved path comes from a CDP event, so only a browser this daemon
/// launched is trusted to name a local file. A remote endpoint could otherwise
/// point the move at any file the daemon can read.
pub(super) fn may_move_download(
    saved: &std::path::Path,
    launched_locally: bool,
    download_dir: Option<&str>,
) -> bool {
    if !launched_locally {
        return false;
    }
    let Some(dir) = download_dir else {
        return true;
    };
    match (fs::canonicalize(saved), fs::canonicalize(dir)) {
        (Ok(saved), Ok(dir)) => saved.starts_with(dir),
        _ => false,
    }
}

pub(super) fn move_download(saved: &std::path::Path, path_str: &str) -> Result<Value, String> {
    let dest = if std::path::Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {}", e))?
            .join(path_str)
    };
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create download directory: {}", e))?;
    }
    if fs::rename(saved, &dest).is_err() {
        // rename fails across filesystems; copy, then remove the original.
        fs::copy(saved, &dest).map_err(|e| format!("Failed to move downloaded file: {}", e))?;
        let _ = fs::remove_file(saved);
    }
    Ok(json!({ "path": dest.to_string_lossy() }))
}
