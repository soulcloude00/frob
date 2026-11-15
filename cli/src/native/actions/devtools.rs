use super::*;

pub(super) async fn handle_react_tree(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let script =
        react::scripts::TREE_SNAPSHOT.replace("{{PICK_RI}}", react::scripts::PICK_REACT_RENDERER);
    let result = mgr.evaluate(&script, None).await?;
    let nodes_json = parse_json_string(result, "react tree")?;
    let nodes: Vec<react::TreeNode> = serde_json::from_value(nodes_json)
        .map_err(|e| format!("Failed to parse tree nodes: {}", e))?;

    let return_json = cmd.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
    if return_json {
        let nodes_value: Vec<Value> = nodes
            .iter()
            .map(|n| {
                json!({
                    "id": n.id,
                    "type": n.node_type,
                    "name": n.name,
                    "key": n.key,
                    "parent": n.parent,
                })
            })
            .collect();
        Ok(json!({ "nodes": nodes_value }))
    } else {
        Ok(json!({ "tree": react::format_tree(&nodes) }))
    }
}

pub(super) async fn handle_react_inspect(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let fiber_id = cmd
        .get("fiberId")
        .and_then(|v| v.as_i64())
        .ok_or("Missing 'fiberId' parameter (numeric React fiber id)")?;

    let script = react::scripts::TREE_INSPECT
        .replace("{{ID}}", &fiber_id.to_string())
        .replace("{{PICK_RI}}", react::scripts::PICK_REACT_RENDERER);
    let result = mgr.evaluate(&script, None).await?;
    let parsed = parse_json_string(result, "react inspect")?;
    Ok(parsed)
}

pub(super) async fn handle_react_renders_start(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    // Install for future navigations, then evaluate immediately so the
    // current page starts recording without a reload.
    let identifier = mgr
        .add_script_to_evaluate(react::scripts::RENDERS_INIT)
        .await?;
    mgr.evaluate(react::scripts::RENDERS_INIT, None).await?;
    let _ = cmd;
    Ok(json!({
        "recording": true,
        "identifier": identifier,
        "message": "recording renders - interact with the page, then run `react renders stop`"
    }))
}

pub(super) async fn handle_react_renders_stop(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let result = mgr.evaluate(react::scripts::RENDERS_STOP, None).await?;
    let data_json = parse_json_string(result, "react renders stop")?;
    let data: react::RendersData = serde_json::from_value(data_json.clone())
        .map_err(|e| format!("Failed to parse renders data: {}", e))?;

    let return_json = cmd.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
    if return_json {
        Ok(data_json)
    } else {
        Ok(json!({ "report": react::format_renders_report(&data) }))
    }
}

pub(super) async fn handle_react_suspense(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let script =
        react::scripts::SUSPENSE_WALK.replace("{{PICK_RI}}", react::scripts::PICK_REACT_RENDERER);
    let result = mgr.evaluate(&script, None).await?;
    let boundaries_json = parse_json_string(result, "react suspense")?;
    let boundaries: Vec<react::Boundary> = serde_json::from_value(boundaries_json.clone())
        .map_err(|e| format!("Failed to parse suspense boundaries: {}", e))?;

    let return_json = cmd.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
    let only_dynamic = cmd
        .get("onlyDynamic")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if return_json {
        // When only-dynamic is set, filter the JSON payload too so callers
        // get consistent output regardless of format choice.
        if only_dynamic {
            let filtered: Vec<&react::Boundary> = boundaries
                .iter()
                .filter(|b| {
                    b.parent_id != 0
                        && (b.is_suspended
                            || !b.suspended_by.is_empty()
                            || b.unknown_suspenders.is_some())
                })
                .collect();
            Ok(json!({ "boundaries": filtered }))
        } else {
            Ok(json!({ "boundaries": boundaries_json }))
        }
    } else {
        Ok(json!({ "report": react::format_suspense_report(&boundaries, only_dynamic) }))
    }
}

pub(super) async fn handle_vitals(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    // Install observers BEFORE the navigation/reload that we want to measure.
    // The script is idempotent — a no-op if already installed on the current page.
    {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        let _ = mgr.evaluate(react::scripts::VITALS_INIT, None).await?;
    }

    // Register as an init script too, so navigations done via `vitals --url`
    // start observing from the first paint.
    {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        let _ = mgr
            .add_script_to_evaluate(react::scripts::VITALS_INIT)
            .await;
    }

    // Navigate to the target URL (or reload the current page) to trigger a
    // full page load the observers can capture.
    let target = cmd.get("url").and_then(|v| v.as_str()).map(String::from);
    if let Some(url) = target {
        let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
        let _ = mgr.navigate(&url, WaitUntil::Load).await?;
    } else {
        handle_reload(state).await?;
    }

    // Give layout shifts and React effects a chance to settle.
    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let url = mgr.get_url().await.unwrap_or_default();
    let result = mgr.evaluate(react::scripts::VITALS_READ, None).await?;
    let raw = parse_json_string(result, "vitals")?;

    // The raw payload has { cwv, timing, ttfb }. Merge with URL and process
    // timing into React hydration phases + per-component durations.
    let cwv = raw.get("cwv").cloned().unwrap_or(json!({}));
    let timing = raw
        .get("timing")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let ttfb = raw.get("ttfb").and_then(|v| v.as_f64());
    let lcp = cwv.get("lcp").cloned().unwrap_or(Value::Null);
    let cls_score = cwv.get("cls").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let cls_entries = cwv.get("clsEntries").cloned().unwrap_or(json!([]));
    let fcp = cwv.get("fcp").and_then(|v| v.as_f64());
    let inp = cwv.get("inp").and_then(|v| v.as_f64());

    let round = |n: f64| (n * 100.0).round() / 100.0;

    let mut hydration_phases: Vec<Value> = Vec::new();
    let mut hydration_start = f64::INFINITY;
    let mut hydration_end = 0.0f64;
    let mut hydrated_components: Vec<Value> = Vec::new();
    // React's profiling build emits `console.timeStamp(label, start, end,
    // track, trackGroup, color)` entries whose `track` / `trackGroup`
    // fields are literal strings containing the atom glyph (e.g.
    // "Scheduler ⚛", "Components ⚛"). The comparisons below match those
    // exact strings — don't "clean up" the glyphs.
    for e in &timing {
        let label = e.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let track = e.get("track").and_then(|v| v.as_str()).unwrap_or("");
        let track_group = e.get("trackGroup").and_then(|v| v.as_str()).unwrap_or("");
        let color = e.get("color").and_then(|v| v.as_str()).unwrap_or("");
        let start = e.get("startTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end = e.get("endTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if end <= start {
            continue;
        }
        if track_group == "Scheduler ⚛" {
            hydration_phases.push(json!({
                "label": label,
                "startTime": round(start),
                "endTime": round(end),
                "duration": round(end - start),
            }));
            if label == "Hydrated" {
                if start < hydration_start {
                    hydration_start = start;
                }
                if end > hydration_end {
                    hydration_end = end;
                }
            }
        } else if track == "Components ⚛" && color.starts_with("tertiary") {
            hydrated_components.push(json!({
                "name": label,
                "startTime": round(start),
                "endTime": round(end),
                "duration": round(end - start),
            }));
        }
    }
    hydrated_components.sort_by(|a, b| {
        let da = a.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let db = b.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
        db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
    });

    let hydration = if hydration_start.is_finite() && hydration_end > 0.0 {
        json!({
            "startTime": round(hydration_start),
            "endTime": round(hydration_end),
            "duration": round(hydration_end - hydration_start),
        })
    } else {
        Value::Null
    };

    let data_value = json!({
        "url": url,
        "ttfb": ttfb,
        "lcp": lcp,
        "cls": { "score": round(cls_score), "entries": cls_entries },
        "fcp": fcp,
        "inp": inp,
        "hydration": hydration,
        "phases": hydration_phases,
        "hydratedComponents": hydrated_components,
    });

    // Always return the structured payload. The CLI output layer renders a
    // compact text summary in normal mode, while `--json` exposes these exact
    // fields for automation.
    Ok(data_value)
}

pub(super) async fn handle_a11y(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    // Navigate first if a target URL was given, so `a11y <url>` audits a
    // fresh load rather than whatever page is currently active.
    if cmd.get("url").and_then(|v| v.as_str()).is_some() {
        // Reuse canonical navigation so element refs, selected frames, domain
        // filtering, and backend-specific behavior stay consistent.
        let _ = handle_navigate(cmd, state).await?;
        // Navigation can attach new out-of-process iframe sessions. Apply
        // those events before installing axe into the complete frame tree.
        state.drain_cdp_events_background().await?;
    }

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;

    let tags = cmd.get("tags").and_then(|v| v.as_str());
    let selector = cmd.get("selector").and_then(|v| v.as_str());
    // Run private partial audits throughout the frame tree, then merge their
    // serialized results through the vendored top-frame engine.
    let session_id = mgr.active_session_id()?;
    let raw = a11y::run_audit(
        &mgr.client,
        session_id,
        &state.iframe_sessions,
        tags,
        selector,
    )
    .await?;
    if let Some(err) = raw.get("error").and_then(|v| v.as_str()) {
        return Err(err.to_string());
    }
    Ok(raw)
}
