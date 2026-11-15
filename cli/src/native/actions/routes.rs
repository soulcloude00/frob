use super::*;

pub(super) async fn handle_responsebody(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let url_pattern = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' parameter")?;
    let timeout_ms = state.timeout_ms(cmd);

    let mut rx = mgr.client.subscribe();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "Timeout waiting for response matching '{}'",
                url_pattern
            ));
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(event)) => {
                if event.method == "Network.responseReceived"
                    && event.session_id.as_deref() == Some(&session_id)
                {
                    if let Some(resp_url) = event
                        .params
                        .get("response")
                        .and_then(|r| r.get("url"))
                        .and_then(|u| u.as_str())
                    {
                        if resp_url.contains(url_pattern) {
                            let request_id = event
                                .params
                                .get("requestId")
                                .and_then(|v| v.as_str())
                                .ok_or("No requestId in response event")?;
                            let status = event
                                .params
                                .get("response")
                                .and_then(|r| r.get("status"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0);
                            let headers = event
                                .params
                                .get("response")
                                .and_then(|r| r.get("headers"))
                                .cloned()
                                .unwrap_or(json!({}));

                            let body_result = mgr
                                .client
                                .send_command(
                                    "Network.getResponseBody",
                                    Some(json!({ "requestId": request_id })),
                                    Some(&session_id),
                                )
                                .await?;
                            let body = body_result
                                .get("body")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            return Ok(
                                json!({ "body": body, "status": status, "headers": headers }),
                            );
                        }
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err("Event stream closed".to_string()),
            Err(_) => {
                return Err(format!(
                    "Timeout waiting for response matching '{}'",
                    url_pattern
                ));
            }
        }
    }
}

pub(super) fn collapse_wildcards(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut last_was_star = false;
    for ch in pattern.chars() {
        if ch == '*' {
            if !last_was_star {
                out.push(ch);
            }
            last_was_star = true;
        } else {
            out.push(ch);
            last_was_star = false;
        }
    }
    out
}

/// Status, headers and base64 body for a mocked route response. A mock gets
/// CORS headers only for the origin the route allows (`--allow-origin`), so a
/// page from any other origin cannot read it.
pub(super) fn mocked_response(
    resp: &RouteResponse,
    request_headers: Option<&serde_json::Map<String, Value>>,
) -> (u16, Vec<Value>, String) {
    let status = resp.status.unwrap_or(200);
    let body_str = resp.body.as_deref().unwrap_or("");
    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        body_str.as_bytes(),
    );
    let mut headers = vec![];
    let route_sets = |name: &str| {
        resp.headers
            .as_ref()
            .is_some_and(|h| h.keys().any(|k| k.eq_ignore_ascii_case(name)))
    };
    // A JSON body with no content type is labeled as JSON, so the
    // page and HAR capture read it as JSON.
    let content_type = resp.content_type.clone().or_else(|| {
        if route_sets("content-type") {
            return None;
        }
        serde_json::from_str::<Value>(body_str)
            .is_ok()
            .then(|| "application/json".to_string())
    });
    if let Some(ct) = &content_type {
        headers.push(json!({ "name": "Content-Type", "value": ct }));
    }
    if let Some(h) = &resp.headers {
        for (k, v) in h {
            headers.push(json!({ "name": k, "value": v }));
        }
    }
    let header = |name: &str| {
        request_headers.and_then(|h| {
            h.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, v)| v.as_str())
                .map(String::from)
        })
    };
    let has_acao = route_sets("access-control-allow-origin");
    let origin = header("origin")
        .or_else(|| {
            header("referer")
                .and_then(|r| url::Url::parse(&r).ok())
                .map(|u| u.origin().ascii_serialization())
        })
        .filter(|o| {
            resp.allow_origin
                .as_deref()
                .is_some_and(|allowed| allowed.trim_end_matches('/') == o)
        });
    let preflight = header("access-control-request-method");
    if let (false, Some(origin)) = (has_acao, &origin) {
        let mut add = |name: &str, value: &str| {
            if !route_sets(name) {
                headers.push(json!({ "name": name, "value": value }));
            }
        };
        add("Access-Control-Allow-Origin", origin);
        add("Access-Control-Allow-Credentials", "true");
        if let Some(method) = &preflight {
            add("Access-Control-Allow-Methods", method);
            if let Some(h) = header("access-control-request-headers") {
                add("Access-Control-Allow-Headers", &h);
            }
        }
    }
    let (status, encoded) = if preflight.is_some() && !has_acao && origin.is_some() {
        (204, String::new())
    } else {
        (status, encoded)
    };
    (status, headers, encoded)
}

pub(super) fn route_url_matches(pattern: &str, url: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return url.contains(pattern);
    }

    let pattern = collapse_wildcards(pattern);
    let parts: Vec<&str> = pattern.split('*').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return true;
    }

    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let mut pos = 0usize;
    let mut idx = 0usize;

    if anchored_start {
        let first = parts[0];
        if !url.starts_with(first) {
            return false;
        }
        pos = first.len();
        idx = 1;
    }

    while idx < parts.len() {
        let part = parts[idx];
        let Some(found) = url[pos..].find(part) else {
            return false;
        };
        pos += found + part.len();
        idx += 1;
    }

    if anchored_end {
        if let Some(last) = parts.last() {
            return url.ends_with(last);
        }
    }

    true
}

pub(super) async fn resolve_fetch_paused(
    client: &CdpClient,
    domain_filter: Option<&DomainFilter>,
    routes: &[RouteEntry],
    origin_headers: &HashMap<String, HashMap<String, String>>,
    blocked_documents: &std::sync::Mutex<HashMap<String, String>>,
    paused: &FetchPausedRequest,
) {
    let session_id = &paused.session_id;

    // Domain filter check (takes priority over routes and origin headers)
    if let Some(filter) = domain_filter {
        if let Ok(parsed) = url::Url::parse(&paused.url) {
            let scheme = parsed.scheme();
            let enforce_host = matches!(scheme, "http" | "https" | "ws" | "wss");
            if !enforce_host {
                if paused.resource_type.eq_ignore_ascii_case("document") {
                    let _ = client
                        .send_command(
                            "Fetch.failRequest",
                            Some(json!({
                                "requestId": paused.request_id,
                                "errorReason": "BlockedByClient"
                            })),
                            Some(session_id),
                        )
                        .await;
                } else {
                    let _ = client
                        .send_command(
                            "Fetch.continueRequest",
                            Some(json!({ "requestId": paused.request_id })),
                            Some(session_id),
                        )
                        .await;
                }
                return;
            }

            if let Some(hostname) = parsed.host_str() {
                if !filter.is_allowed(hostname) {
                    if paused.resource_type.eq_ignore_ascii_case("document") {
                        blocked_documents
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(paused.frame_id.clone(), paused.url.clone());
                        let error_body = format!(
                            "<html><body><h1>Blocked</h1><p>Navigation to {} is not allowed by domain filter.</p></body></html>",
                            hostname
                        );
                        let encoded = base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            error_body.as_bytes(),
                        );
                        let _ = client
                            .send_command(
                                "Fetch.fulfillRequest",
                                Some(json!({
                                    "requestId": paused.request_id,
                                    "responseCode": 403,
                                    "responseHeaders": [
                                        { "name": "Content-Type", "value": "text/html" },
                                    ],
                                    "body": encoded,
                                })),
                                Some(session_id),
                            )
                            .await;
                    } else {
                        let _ = client
                            .send_command(
                                "Fetch.failRequest",
                                Some(json!({
                                    "requestId": paused.request_id,
                                    "errorReason": "BlockedByClient"
                                })),
                                Some(session_id),
                            )
                            .await;
                    }
                    return;
                }
            }
        }
    }

    // Route matching
    if let Some(route) = select_route(routes, &paused.url, &paused.resource_type) {
        if route.abort {
            let _ = client
                .send_command(
                    "Fetch.failRequest",
                    Some(json!({
                        "requestId": paused.request_id,
                        "errorReason": "Failed"
                    })),
                    Some(session_id),
                )
                .await;
            return;
        }

        if let Some(ref resp) = route.response {
            let (status, headers, encoded) = mocked_response(resp, paused.request_headers.as_ref());
            let _ = client
                .send_command(
                    "Fetch.fulfillRequest",
                    Some(json!({
                        "requestId": paused.request_id,
                        "responseCode": status,
                        "responseHeaders": headers,
                        "body": encoded,
                    })),
                    Some(session_id),
                )
                .await;
            return;
        }
    }

    // No matching route — continue, injecting origin-scoped headers if applicable.
    let extra = url::Url::parse(&paused.url)
        .ok()
        .map(|u| u.origin().ascii_serialization())
        .and_then(|o| origin_headers.get(&o));

    if let Some(extra_headers) = extra {
        // Merge original request headers with extra headers.
        // Fetch.continueRequest replaces (not merges), so include originals.
        let mut combined: Vec<Value> = Vec::new();
        if let Some(ref orig) = paused.request_headers {
            for (k, v) in orig {
                if !extra_headers.keys().any(|ek| ek.eq_ignore_ascii_case(k)) {
                    if let Some(s) = v.as_str() {
                        combined.push(json!({ "name": k, "value": s }));
                    }
                }
            }
        }
        for (k, v) in extra_headers {
            combined.push(json!({ "name": k, "value": v }));
        }
        let _ = client
            .send_command(
                "Fetch.continueRequest",
                Some(json!({ "requestId": paused.request_id, "headers": combined })),
                Some(session_id),
            )
            .await;
    } else {
        let _ = client
            .send_command(
                "Fetch.continueRequest",
                Some(json!({ "requestId": paused.request_id })),
                Some(session_id),
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// Build the Fetch.enable patterns list from current routes, domain filter,
/// and origin headers state.  When domain filtering or origin-scoped headers
/// are active a wildcard pattern is included so all requests are intercepted.
pub(super) async fn build_fetch_patterns(state: &DaemonState) -> Vec<Value> {
    let routes = state.routes.read().await;
    let mut patterns: Vec<Value> = routes
        .iter()
        .map(|r| json!({ "urlPattern": collapse_wildcards(&r.url_pattern) }))
        .collect();
    let has_domain_filter = state.domain_filter.read().await.is_some();
    let has_origin_headers = !state.origin_headers.read().await.is_empty();
    let has_proxy_creds = state.proxy_credentials.read().await.is_some();
    if (has_domain_filter || has_origin_headers || has_proxy_creds)
        && !patterns.iter().any(|p| p["urlPattern"] == "*")
    {
        patterns.push(json!({ "urlPattern": "*" }));
    }
    patterns
}

/// Build the full Fetch.enable params object, including `handleAuthRequests`
/// when proxy credentials are configured.
pub(super) async fn build_fetch_enable_params(state: &DaemonState, patterns: Vec<Value>) -> Value {
    let has_proxy_creds = state.proxy_credentials.read().await.is_some();
    if has_proxy_creds {
        json!({ "patterns": patterns, "handleAuthRequests": true })
    } else {
        json!({ "patterns": patterns })
    }
}

pub(super) fn parse_route_response(cmd: &Value) -> Option<RouteResponse> {
    cmd.get("response")
        .and_then(|v| {
            if v.is_null() {
                return None;
            }
            Some(RouteResponse {
                status: v.get("status").and_then(|s| s.as_u64()).map(|s| s as u16),
                body: v.get("body").and_then(|s| s.as_str()).map(String::from),
                content_type: v
                    .get("contentType")
                    .and_then(|s| s.as_str())
                    .map(String::from),
                allow_origin: v
                    .get("allowOrigin")
                    .and_then(|s| s.as_str())
                    .map(String::from),
                headers: v.get("headers").and_then(|h| {
                    h.as_object().map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                }),
            })
        })
        .or_else(|| {
            cmd.get("body")
                .and_then(|v| v.as_str())
                .map(|body| RouteResponse {
                    status: None,
                    body: Some(body.to_string()),
                    content_type: None,
                    headers: None,
                    allow_origin: cmd
                        .get("allowOrigin")
                        .and_then(|s| s.as_str())
                        .map(String::from),
                })
        })
}

pub(super) async fn handle_route(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let url_pattern = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' parameter")?
        .to_string();
    let abort = cmd.get("abort").and_then(|v| v.as_bool()).unwrap_or(false);

    let resource_types: Vec<String> = cmd
        .get("resourceType")
        .or_else(|| cmd.get("resourceTypes"))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(
                    s.split(',')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect(),
                )
            } else {
                v.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .filter(|s| !s.is_empty())
                        .collect()
                })
            }
        })
        .unwrap_or_default();

    let response = parse_route_response(cmd);

    {
        let mut routes = state.routes.write().await;
        routes.push(RouteEntry {
            url_pattern: url_pattern.clone(),
            response,
            abort,
            resource_types,
        });
    }

    let patterns = build_fetch_patterns(state).await;
    let params = build_fetch_enable_params(state, patterns).await;
    mgr.client
        .send_command("Fetch.enable", Some(params), Some(&session_id))
        .await?;

    Ok(json!({ "routed": url_pattern }))
}

pub(super) async fn handle_unroute(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    let url = cmd.get("url").and_then(|v| v.as_str());

    {
        let mut routes = state.routes.write().await;
        match url {
            Some(pattern) => {
                routes.retain(|r| r.url_pattern != pattern);
            }
            None => {
                routes.clear();
            }
        }
    }

    let patterns = build_fetch_patterns(state).await;
    if patterns.is_empty() {
        mgr.client
            .send_command("Fetch.disable", None, Some(&session_id))
            .await?;
    } else {
        let params = build_fetch_enable_params(state, patterns).await;
        mgr.client
            .send_command("Fetch.enable", Some(params), Some(&session_id))
            .await?;
    }

    let label = url.unwrap_or("all");
    Ok(json!({ "unrouted": label }))
}

pub fn matches_status_filter(status: Option<i64>, filter: &str) -> bool {
    let Some(code) = status else { return false };
    let f = filter.to_lowercase();
    if let Ok(exact) = f.parse::<i64>() {
        return code == exact;
    }
    if f.len() == 3 && f.ends_with("xx") {
        if let Ok(prefix) = f[..1].parse::<i64>() {
            return code / 100 == prefix;
        }
    }
    if let Some((lo, hi)) = f.split_once('-') {
        if let (Ok(lo), Ok(hi)) = (lo.parse::<i64>(), hi.parse::<i64>()) {
            return code >= lo && code <= hi;
        }
    }
    false
}

pub(super) async fn handle_requests(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    if cmd.get("clear").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.tracked_requests.clear();
        return Ok(json!({ "cleared": true }));
    }

    if !state.request_tracking {
        state.request_tracking = true;
        state.refresh_active_iframe_sessions().await;
        if let Some(ref mgr) = state.browser {
            if let Ok(session_id) = mgr.active_session_id() {
                let _ = mgr
                    .client
                    .send_command_no_params("Network.enable", Some(session_id))
                    .await;
            }
        }
    }

    let filter = cmd.get("filter").and_then(|v| v.as_str());
    let type_filter = cmd.get("type").and_then(|v| v.as_str());
    let method_filter = cmd.get("method").and_then(|v| v.as_str());
    let status_filter = cmd.get("status").and_then(|v| v.as_str());

    let type_list: Vec<String> = type_filter
        .map(|t| t.split(',').map(|s| s.trim().to_lowercase()).collect())
        .unwrap_or_default();

    let requests: Vec<&TrackedRequest> = state
        .tracked_requests
        .iter()
        .filter(|r| {
            if let Some(f) = filter {
                if !r.url.contains(f) {
                    return false;
                }
            }
            if !type_list.is_empty() && !type_list.contains(&r.resource_type.to_lowercase()) {
                return false;
            }
            if let Some(m) = method_filter {
                if !r.method.eq_ignore_ascii_case(m) {
                    return false;
                }
            }
            if let Some(s) = status_filter {
                if !matches_status_filter(r.status, s) {
                    return false;
                }
            }
            true
        })
        .collect();

    Ok(json!({ "requests": requests }))
}

pub(super) async fn handle_request_detail(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let request_id = cmd
        .get("requestId")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'requestId' parameter")?;

    // Redirect hops share a request id; the last one is the response a body
    // belongs to.
    let entry = state
        .tracked_requests
        .iter()
        .rev()
        .find(|r| r.request_id == request_id)
        .ok_or("Request not found")?;

    let mut result = serde_json::to_value(entry).unwrap_or(json!({}));

    if let Some(ref mgr) = state.browser {
        if let Ok(session_id) = mgr.active_session_id() {
            if let Ok(body_result) = mgr
                .client
                .send_command(
                    "Network.getResponseBody",
                    Some(json!({ "requestId": request_id })),
                    Some(session_id),
                )
                .await
            {
                let base64_encoded = body_result
                    .get("base64Encoded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let body = body_result
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if base64_encoded {
                    result["responseBody"] = json!(format!("[base64, {} chars]", body.len()));
                } else {
                    result["responseBody"] = json!(body);
                }
            }
        }
    }

    Ok(result)
}
