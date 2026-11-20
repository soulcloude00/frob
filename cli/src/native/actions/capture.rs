use super::*;

/// Snapshots longer than this fold their biggest sections (see `--collapse`), so a long document
/// costs an agent about 8k tokens instead of 60k. `FROB_SNAPSHOT_LINES` changes it; 0 turns it off.
const DEFAULT_SNAPSHOT_LINES: usize = 800;

/// The line budget for a snapshot: `collapse` on the command (0 for none), else the default.
/// Delta snapshots are compared line by line, so they are never folded by default.
fn snapshot_line_budget(cmd: &Value) -> Option<usize> {
    if let Some(n) = cmd.get("collapse").and_then(Value::as_u64) {
        return (n > 0).then_some(n as usize);
    }
    if cmd.get("delta").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let budget = std::env::var("FROB_SNAPSHOT_LINES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_SNAPSHOT_LINES);
    (budget > 0).then_some(budget)
}

pub(super) async fn handle_snapshot(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let options = SnapshotOptions {
        selector: cmd
            .get("selector")
            .and_then(|v| v.as_str())
            .map(String::from),
        interactive: cmd
            .get("interactive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        compact: cmd
            .get("compact")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        depth: cmd
            .get("maxDepth")
            .and_then(|v| v.as_u64())
            .map(|d| d as usize),
        urls: cmd.get("urls").and_then(|v| v.as_bool()).unwrap_or(false),
        collapse: snapshot_line_budget(cmd),
        scope_object_id: None,
    };
    // A ref selector (`snapshot -s @e5`) is resolved before the refs map is reset below.
    let mut options = options;
    if let Some(selector) = options.selector.as_deref() {
        if crate::native::element::parse_ref(selector).is_some() {
            let (object_id, _) = crate::native::element::resolve_element_object_id(
                &mgr.client,
                &session_id,
                &state.ref_map,
                selector,
                &state.iframe_sessions,
            )
            .await?;
            options.scope_object_id = Some(object_id);
        }
    }

    let previous_refs = state.ref_map.ref_ids();
    state.ref_map.begin_snapshot();
    let tree = snapshot::take_snapshot(
        &mgr.client,
        &session_id,
        &options,
        &mut state.ref_map,
        state.active_frame_id.as_deref(),
        &state.iframe_sessions,
    )
    .await?;

    let url = mgr.get_url().await.unwrap_or_default();

    let refs: serde_json::Map<String, Value> = state
        .ref_map
        .entries_sorted()
        .into_iter()
        .map(|(ref_id, entry)| {
            let mut obj = serde_json::Map::new();
            obj.insert("role".into(), Value::String(entry.role));
            obj.insert("name".into(), Value::String(entry.name));
            (ref_id, Value::Object(obj))
        })
        .collect();

    let current_refs = state.ref_map.ref_ids();
    let mut removed_refs = previous_refs
        .difference(&current_refs)
        .map(|ref_id| format!("@{}", ref_id))
        .collect::<Vec<_>>();
    removed_refs.sort_by_key(|ref_id| {
        ref_id
            .trim_start_matches("@e")
            .parse::<usize>()
            .unwrap_or(usize::MAX)
    });

    if !cmd.get("delta").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(
            json!({ "snapshot": tree, "origin": url, "refs": refs, "removedRefs": removed_refs }),
        );
    }

    let options_key = serde_json::to_string(&json!({
        "selector": options.selector,
        "interactive": options.interactive,
        "compact": options.compact,
        "depth": options.depth,
        "urls": options.urls,
    }))
    .unwrap_or_default();
    let previous = state.snapshot_revisions.get(&session_id).cloned();
    let revision = previous.as_ref().map_or(1, |entry| entry.revision + 1);
    let force_full = cmd.get("full").and_then(Value::as_bool).unwrap_or(false)
        || previous
            .as_ref()
            .is_none_or(|entry| entry.url != url || entry.options != options_key);
    let response = if force_full {
        json!({
            "snapshot": { "kind": "full", "revision": revision, "tree": tree, "refs": refs },
            "origin": url,
        })
    } else {
        snapshot_delta_response(previous.as_ref().unwrap(), revision, &tree, &refs, &url)
    };

    if state.snapshot_revisions.len() >= 32 && !state.snapshot_revisions.contains_key(&session_id) {
        if let Some(oldest_key) = state
            .snapshot_revisions
            .iter()
            .min_by_key(|(_, entry)| entry.revision)
            .map(|(key, _)| key.clone())
        {
            state.snapshot_revisions.remove(&oldest_key);
        }
    }
    state.snapshot_revisions.insert(
        session_id,
        SnapshotRevision {
            revision,
            url,
            options: options_key,
            tree,
            refs,
        },
    );
    Ok(response)
}

pub(super) fn snapshot_delta_response(
    previous: &SnapshotRevision,
    revision: u64,
    tree: &str,
    refs: &serde_json::Map<String, Value>,
    origin: &str,
) -> Value {
    if previous.tree == tree {
        return json!({
            "snapshot": { "kind": "unchanged", "baseRevision": previous.revision, "revision": revision },
            "origin": origin,
        });
    }

    let mut changes = Vec::new();
    for (ref_id, old_node) in &previous.refs {
        match refs.get(ref_id) {
            None => changes.push(json!({ "op": "remove", "ref": format!("@{}", ref_id) })),
            Some(new_node) => {
                for field in ["role", "name"] {
                    if old_node.get(field) != new_node.get(field) {
                        changes.push(json!({
                            "op": "replace",
                            "ref": format!("@{}", ref_id),
                            "field": field,
                            "value": new_node.get(field).cloned().unwrap_or(Value::Null),
                        }));
                    }
                }
            }
        }
    }
    for (ref_id, node) in refs {
        if !previous.refs.contains_key(ref_id) {
            changes.push(json!({ "op": "add", "ref": format!("@{}", ref_id), "node": node }));
        }
    }

    // Ref metadata contains only role/name, so it cannot describe checkbox
    // state, text, values, hierarchy, or ordering. Include an exact tree splice
    // alongside ref operations so every accepted revision can be reconstructed.
    let before_lines: Vec<&str> = previous.tree.split('\n').collect();
    let after_lines: Vec<&str> = tree.split('\n').collect();
    let prefix = before_lines
        .iter()
        .zip(&after_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = before_lines[prefix..]
        .iter()
        .rev()
        .zip(after_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let tree_change = json!({
        "startLine": prefix,
        "deleteCount": before_lines.len() - prefix - suffix,
        "lines": after_lines[prefix..after_lines.len() - suffix],
    });
    let delta = json!({
        "kind": "delta",
        "baseRevision": previous.revision,
        "revision": revision,
        "changes": changes,
        "treeChange": tree_change,
    });
    let delta_size = serde_json::to_vec(&delta).map_or(usize::MAX, |bytes| bytes.len());
    if delta_size >= tree.len().saturating_mul(7) / 10 {
        json!({
            "snapshot": { "kind": "full", "revision": revision, "tree": tree, "refs": refs },
            "origin": origin,
        })
    } else {
        json!({ "snapshot": delta, "origin": origin })
    }
}

pub(super) fn decode_screenshot_pixels(
    base64_data: &str,
) -> Result<(u32, u32, Vec<u8>, u64), String> {
    let encoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64_data)
        .map_err(|e| format!("Base64 decode error: {}", e))?;
    let image = image::load_from_memory(&encoded)
        .map_err(|e| format!("Failed to decode screenshot pixels: {}", e))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    rgba.hash(&mut hasher);
    Ok((width, height, rgba, hasher.finish()))
}

pub(super) fn changed_pixel_ratio(
    previous: &ScreenshotObservation,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> f64 {
    if previous.width != width || previous.height != height || previous.rgba.len() != rgba.len() {
        return 1.0;
    }
    if rgba.is_empty() {
        return 0.0;
    }
    let changed = previous
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rgba.as_chunks::<4>().0)
        .filter(|(before, after)| before != after)
        .count();
    changed as f64 / (rgba.len() / 4) as f64
}

/// Compares decoded pixels with the last returned conditional screenshot.
/// Suppressed captures retain that baseline so small changes can accumulate.
/// Encoded image metadata therefore cannot create a false positive.
pub(super) fn observe_screenshot(
    state: &mut DaemonState,
    key: String,
    signature: String,
    base64_data: &str,
    threshold: f64,
    options: &ScreenshotOptions,
) -> Result<Value, String> {
    let (width, height, rgba, decoded_hash) = decode_screenshot_pixels(base64_data)?;
    // Retain an independent baseline and revision when alternating capture
    // scopes in one tab. The shared cap also bounds retained pixel buffers.
    let key = (key, signature.clone());
    // Evict stale tab history before admitting another capture scope so decoded
    // image buffers remain bounded even during long multi-tab sessions.
    if !state.screenshot_observations.contains_key(&key)
        && state.screenshot_observations.len() >= 32
    {
        if let Some(eviction_key) = state.screenshot_observations.keys().next().cloned() {
            state.screenshot_observations.remove(&eviction_key);
        }
    }
    let previous = state.screenshot_observations.get(&key);
    let revision = previous.map_or(1, |item| item.revision.saturating_add(1));
    let pixel_change_ratio = match previous {
        Some(item) if item.signature == signature && item.decoded_hash == decoded_hash => 0.0,
        Some(item) if item.signature == signature => {
            changed_pixel_ratio(item, width, height, &rgba)
        }
        _ => 1.0,
    };
    let changed = previous.is_none()
        || previous.is_some_and(|item| item.signature != signature)
        || pixel_change_ratio > threshold;

    let path = if changed {
        Some(screenshot::save_screenshot(base64_data, options)?)
    } else {
        None
    };
    if changed {
        state.screenshot_observations.insert(
            key,
            ScreenshotObservation {
                revision,
                signature,
                decoded_hash,
                width,
                height,
                rgba,
            },
        );
    } else if let Some(previous) = state.screenshot_observations.get_mut(&key) {
        previous.revision = revision;
    }
    let mut response = json!({
        "changed": changed,
        "revision": revision,
        "pixelChangeRatio": pixel_change_ratio,
        "threshold": threshold
    });
    if let Some(path) = path {
        response["path"] = json!(path);
    }
    Ok(response)
}

pub(super) async fn handle_screenshot(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let annotate = cmd
        .get("annotate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let if_changed = cmd
        .get("ifChanged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let threshold = cmd.get("threshold").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let signature = format!(
        "selector={:?};fullPage={};annotate={}",
        cmd.get("selector").and_then(|v| v.as_str()),
        cmd.get("fullPage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        annotate
    );

    let format = cmd
        .get("format")
        .or_else(|| cmd.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("png")
        .to_string();

    let mut options = ScreenshotOptions {
        selector: cmd
            .get("selector")
            .and_then(|v| v.as_str())
            .map(String::from),
        path: cmd.get("path").and_then(|v| v.as_str()).map(String::from),
        full_page: cmd
            .get("fullPage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        format,
        quality: cmd
            .get("quality")
            .and_then(|v| v.as_i64())
            .map(|q| q as i32),
        annotate,
        output_dir: cmd
            .get("screenshotDir")
            .and_then(|v| v.as_str())
            .map(String::from),
    };

    let (session_id, result) = if let Some(wb) = state
        .webdriver_backend
        .as_ref()
        .filter(|_| state.browser.is_none())
    {
        if annotate {
            return Err(
                "Annotated screenshots are not yet implemented on the WebDriver backend"
                    .to_string(),
            );
        }
        options.path.get_or_insert_with(|| {
            format!(
                "/tmp/screenshot-{}.png",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            )
        });
        (
            "webdriver-active".to_string(),
            screenshot::ScreenshotResult {
                base64: wb.screenshot().await?,
                annotations: Vec::new(),
            },
        )
    } else {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        let session_id = mgr.active_session_id()?.to_string();
        if annotate {
            state.ref_map.begin_snapshot();
            let _ = snapshot::take_snapshot(
                &mgr.client,
                &session_id,
                &SnapshotOptions {
                    interactive: true,
                    ..SnapshotOptions::default()
                },
                &mut state.ref_map,
                state.active_frame_id.as_deref(),
                &state.iframe_sessions,
            )
            .await?;
        }

        let result = screenshot::take_screenshot(
            &mgr.client,
            &session_id,
            &state.ref_map,
            &options,
            &state.iframe_sessions,
        )
        .await?;

        (session_id, result)
    };

    let mut response = if if_changed {
        observe_screenshot(
            state,
            session_id,
            signature,
            &result.base64,
            threshold,
            &options,
        )?
    } else {
        json!({ "path": screenshot::save_screenshot(&result.base64, &options)? })
    };
    if !result.annotations.is_empty() {
        response["annotations"] = serde_json::to_value(&result.annotations)
            .map_err(|e| format!("Failed to serialize annotations: {}", e))?;
    }

    Ok(response)
}

pub(super) async fn handle_diff_snapshot(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let compact = cmd
        .get("compact")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_depth = cmd
        .get("maxDepth")
        .and_then(|v| v.as_u64())
        .map(|d| d as usize);
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .map(String::from);

    let options = SnapshotOptions {
        compact,
        depth: max_depth,
        selector,
        ..SnapshotOptions::default()
    };
    // Reuse the current document identities without committing a failed capture.
    // Build the replacement separately so a failed diff leaves the existing refs usable.
    let mut current_ref_map = state.ref_map.clone();
    current_ref_map.begin_snapshot();
    let current = snapshot::take_snapshot(
        &mgr.client,
        &session_id,
        &options,
        &mut current_ref_map,
        state.active_frame_id.as_deref(),
        &state.iframe_sessions,
    )
    .await?;

    let baseline = cmd.get("baseline").and_then(|v| v.as_str());

    let baseline_text = match baseline {
        Some(b) if std::path::Path::new(b).exists() => {
            let mut contents = std::fs::read_to_string(b)
                .map_err(|e| format!("Failed to read baseline: {}", e))?;
            // Plain CLI output ends with one newline when redirected to a baseline file.
            if contents.ends_with('\n') {
                contents.pop();
                if contents.ends_with('\r') {
                    contents.pop();
                }
            }
            contents
        }
        Some(b) => b.to_string(),
        None => String::new(),
    };

    let result = diff::diff_snapshots(
        &diff::snapshot_comparison_text(&baseline_text),
        &diff::snapshot_comparison_text(&current),
    );
    state.ref_map = current_ref_map;
    Ok(json!({
        "diff": result.diff,
        "additions": result.additions,
        "removals": result.removals,
        "unchanged": result.unchanged,
        "changed": result.changed,
    }))
}

pub(super) async fn handle_diff_url(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_mut().ok_or("Browser not launched")?;

    let url1 = cmd
        .get("url1")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url1' parameter")?;
    let url2 = cmd
        .get("url2")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url2' parameter")?;

    let wait_until = cmd
        .get("waitUntil")
        .and_then(|v| v.as_str())
        .map(WaitUntil::from_str)
        .unwrap_or(WaitUntil::Load);

    // Each navigation can replace the document, so invalidate refs before it starts.
    let first_session_id = mgr.active_session_id()?.to_string();
    state.ref_map.invalidate_page(&first_session_id);

    // Navigate to URL1 and snapshot
    mgr.navigate(url1, wait_until).await?;
    let first_session_id = mgr.active_session_id()?.to_string();
    let options = SnapshotOptions::default();
    let mut snap1_ref_map = state.ref_map.clone();
    let snap1 = snapshot::take_snapshot(
        &mgr.client,
        &first_session_id,
        &options,
        &mut snap1_ref_map,
        None,
        &state.iframe_sessions,
    )
    .await?;

    // Navigate to URL2 and snapshot
    snap1_ref_map.invalidate_page(&first_session_id);
    mgr.navigate(url2, wait_until).await?;
    let second_session_id = mgr.active_session_id()?.to_string();
    let mut snap2_ref_map = snap1_ref_map;
    let snap2 = snapshot::take_snapshot(
        &mgr.client,
        &second_session_id,
        &options,
        &mut snap2_ref_map,
        None,
        &state.iframe_sessions,
    )
    .await?;

    let result = diff::diff_text(
        &diff::snapshot_comparison_text(&snap1),
        &diff::snapshot_comparison_text(&snap2),
    );
    state.ref_map = snap2_ref_map;
    Ok(json!({
        "diff": result,
        "url1": url1,
        "url2": url2,
        "snapshot1": snap1,
        "snapshot2": snap2,
    }))
}

pub(super) async fn handle_pdf(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let params = json!({
        "printBackground": cmd.get("printBackground").and_then(|v| v.as_bool()).unwrap_or(true),
        "landscape": cmd.get("landscape").and_then(|v| v.as_bool()).unwrap_or(false),
        "preferCSSPageSize": cmd.get("preferCSSPageSize").and_then(|v| v.as_bool()).unwrap_or(false),
    });

    let result = mgr
        .client
        .send_command("Page.printToPDF", Some(params), Some(&session_id))
        .await?;

    let data = result
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or("No PDF data returned")?;

    let path = cmd.get("path").and_then(|v| v.as_str());
    let save_path = match path {
        Some(p) => p.to_string(),
        None => {
            let dir = std::env::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".frob")
                .join("tmp")
                .join("pdfs");
            let _ = std::fs::create_dir_all(&dir);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            dir.join(format!("page-{}.pdf", timestamp))
                .to_string_lossy()
                .to_string()
        }
    };

    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
        .map_err(|e| format!("Failed to decode PDF: {}", e))?;
    std::fs::write(&save_path, &bytes).map_err(|e| format!("Failed to save PDF: {}", e))?;

    Ok(json!({ "path": save_path }))
}

// ---------------------------------------------------------------------------
// Phase 8 handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_diff_screenshot(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let baseline_path = cmd
        .get("baseline")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'baseline' parameter")?;

    let threshold = cmd.get("threshold").and_then(|v| v.as_f64()).unwrap_or(0.1);

    let options = ScreenshotOptions {
        selector: cmd
            .get("selector")
            .and_then(|v| v.as_str())
            .map(String::from),
        path: None,
        full_page: cmd
            .get("fullPage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        format: "png".to_string(),
        quality: None,
        annotate: false,
        output_dir: None,
    };

    let result = screenshot::take_screenshot(
        &mgr.client,
        &session_id,
        &state.ref_map,
        &options,
        &state.iframe_sessions,
    )
    .await?;

    let current_bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &result.base64)
            .map_err(|e| format!("Failed to decode screenshot: {}", e))?;

    let baseline_bytes =
        std::fs::read(baseline_path).map_err(|e| format!("Failed to read baseline: {}", e))?;

    let result = diff::diff_screenshot(&baseline_bytes, &current_bytes, threshold)?;

    let output_path = cmd.get("output").and_then(|v| v.as_str());
    if let (Some(out_path), Some(ref diff_data)) = (output_path, &result.diff_image) {
        std::fs::write(out_path, diff_data)
            .map_err(|e| format!("Failed to write diff image: {}", e))?;
    }

    Ok(json!({
        "match": result.matched,
        "mismatchPercentage": result.mismatch_percentage,
        "totalPixels": result.total_pixels,
        "differentPixels": result.different_pixels,
        "diffPath": output_path,
        "dimensionMismatch": result.dimension_mismatch,
    }))
}

// ---------------------------------------------------------------------------
// Video and HAR handlers
// ---------------------------------------------------------------------------
