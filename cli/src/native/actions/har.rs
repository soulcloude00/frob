use super::*;

/// Begin capturing network traffic for a later HAR export.
pub(super) async fn handle_har_start(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    if state.flow.active() && state.har_recording {
        return Err(
            "capture is owned by --capture flow; HAR recording is already running".to_string(),
        );
    }
    let content_mode = match cmd.get("content").and_then(|v| v.as_str()) {
        Some(s) => HarContentMode::parse(s)?,
        None => HarContentMode::default(),
    };
    state.refresh_active_iframe_sessions().await;
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    // Larger buffers so response bodies survive until the periodic event
    // drain fetches them via Network.getResponseBody.
    let network_enable_params = json!({
        "maxTotalBufferSize": 100_000_000,
        "maxResourceBufferSize": 10_000_000,
    });
    mgr.client
        .send_command(
            "Network.enable",
            Some(network_enable_params.clone()),
            Some(&session_id),
        )
        .await?;
    // Also enable Network on cross-origin iframe sessions so their
    // requests are captured in the HAR output.
    for iframe_sid in &state.active_iframe_sessions {
        let _ = mgr
            .client
            .send_command(
                "Network.enable",
                Some(network_enable_params.clone()),
                Some(iframe_sid.as_str()),
            )
            .await;
    }
    state.har_recording = true;
    state.har_entries.clear();
    state.har_progress.clear();
    state.har_content_mode = content_mode;
    state.har_body_total_bytes = 0;
    Ok(json!({ "started": true }))
}

/// Stop HAR recording and write the captured requests to disk.
pub(super) async fn handle_har_stop(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    if state.flow.active() {
        return Err("capture is owned by --capture flow; use flow save".to_string());
    }
    let path = har_output_path(cmd.get("path").and_then(|v| v.as_str()));

    let entries: Vec<Value> = state.har_entries.iter().map(har_entry_to_json).collect();
    let request_count = entries.len();
    let browser = har_browser_metadata(state).await;

    let mut log = json!({
        "version": "1.2",
        "creator": {
            "name": "frob",
            "version": env!("CARGO_PKG_VERSION")
        },
        "entries": entries
    });
    if let Some(browser) = browser {
        log["browser"] = browser;
    }
    let har = json!({ "log": log });

    let har_str = serde_json::to_string_pretty(&har)
        .map_err(|e| format!("Failed to serialize HAR: {}", e))?;
    std::fs::write(&path, har_str).map_err(|e| format!("Failed to write HAR: {}", e))?;

    state.har_entries.clear();

    state.har_progress.clear();
    state.har_recording = false;
    state.har_body_total_bytes = 0;

    Ok(json!({ "path": path, "requestCount": request_count }))
}

// ---------------------------------------------------------------------------
// Saved flows
// ---------------------------------------------------------------------------

/// Convert a `HarEntry` (collected from CDP events) into a HAR 1.2 entry object.
pub(super) fn har_entry_to_json(e: &HarEntry) -> Value {
    let started_date_time = har_wall_time_to_rfc3339(e.wall_time);

    let request_cookies = e
        .request_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        .map(|(_, v)| har_parse_request_cookies(v))
        .unwrap_or_default();

    let query_string = har_parse_query_string(&e.url);

    let req_headers: Vec<Value> = e
        .request_headers
        .iter()
        .map(|(k, v)| json!({ "name": k, "value": v }))
        .collect();

    let resp_cookies: Vec<Value> = e
        .response_headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| {
            // Split on ';' first to discard attributes (Path, HttpOnly, etc.),
            // then split on '=' once to separate name from value.
            let name_value = v.split(';').next().unwrap_or("");
            let (name, value) = name_value.split_once('=').unwrap_or((name_value, ""));
            json!({ "name": name.trim(), "value": value.trim() })
        })
        .collect();

    let resp_headers: Vec<Value> = e
        .response_headers
        .iter()
        .map(|(k, v)| json!({ "name": k, "value": v }))
        .collect();

    let (timings, total_time) =
        har_compute_timings(e.cdp_timing.as_ref(), e.loading_finished_timestamp);

    let mime_type = if e.mime_type.is_empty() {
        "application/octet-stream"
    } else {
        e.mime_type.as_str()
    };

    let post_content_type = e
        .request_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("text/plain")
        .to_string();

    let mut request = json!({
        "method": e.method,
        "url": e.url,
        "httpVersion": e.http_version,
        "cookies": request_cookies,
        "headers": req_headers,
        "queryString": query_string,
        "headersSize": -1,
        "bodySize": e.request_body_size,
    });
    if let Some(body) = &e.post_data {
        request["postData"] = json!({ "mimeType": post_content_type, "text": body });
    }

    let mut content = json!({
        "size": e.response_body_size,
        "mimeType": mime_type,
    });
    if let Some(body) = &e.response_body {
        content["text"] = json!(body);
        if e.response_body_base64 {
            content["encoding"] = json!("base64");
        }
    }

    json!({
        "startedDateTime": started_date_time,
        "time": total_time,
        "request": request,
        "response": {
            "status": e.status.unwrap_or(0),
            "statusText": e.status_text,
            "httpVersion": e.http_version,
            "cookies": resp_cookies,
            "headers": resp_headers,
            "content": content,
            "redirectURL": e.redirect_url,
            "headersSize": -1,
            "bodySize": e.response_body_size,
        },
        "cache": {},
        "timings": timings,
        "_resourceType": e.resource_type,
    })
}

/// Convert a CDP headers object (`{ "Name": "value", ... }`) into a flat
/// `Vec<(name, value)>` preserving insertion order.
pub(super) fn har_extract_headers(headers_val: Option<&Value>) -> Vec<(String, String)> {
    headers_val
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a MIME type is text-like enough to embed as HAR `content.text`
/// under [`HarContentMode::Text`].
pub(super) fn har_mime_is_text(mime: &str) -> bool {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("text/")
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/ecmascript"
                | "application/x-www-form-urlencoded"
                | "application/graphql"
        )
}

/// Map a CDP `response.protocol` value to an HTTP-version string as required
/// by the HAR spec (e.g. `"h2"` → `"HTTP/2.0"`).
pub(super) fn har_cdp_protocol_to_http_version(protocol: &str) -> String {
    match protocol.to_ascii_lowercase().as_str() {
        "h2" => "HTTP/2.0".to_string(),
        "h3" => "HTTP/3.0".to_string(),
        "http/1.0" => "HTTP/1.0".to_string(),
        _ => "HTTP/1.1".to_string(),
    }
}

/// Parse query-string parameters from a URL into a HAR `queryString` array.
pub(super) fn har_parse_query_string(url_str: &str) -> Vec<Value> {
    url::Url::parse(url_str)
        .map(|u| {
            u.query_pairs()
                .map(|(k, v)| json!({ "name": k.as_ref(), "value": v.as_ref() }))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `Cookie: name1=val1; name2=val2` header value into HAR cookie objects.
pub(super) fn har_parse_request_cookies(cookie_header: &str) -> Vec<Value> {
    cookie_header
        .split(';')
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some(json!({ "name": name.trim(), "value": value.trim() }))
        })
        .collect()
}

/// Copies a CDP `Response` (from `responseReceived` or a `redirectResponse`)
/// onto its tracked request.
pub(super) fn tracked_fill_response(entry: &mut TrackedRequest, response: &Value) {
    entry.status = response.get("status").and_then(|v| v.as_i64());
    entry.mime_type = response
        .get("mimeType")
        .and_then(|v| v.as_str())
        .map(String::from);
    entry.response_headers = response.get("headers").cloned();
}

/// Copies a CDP `Response` (from `responseReceived` or a `redirectResponse`)
/// onto its HAR entry.
pub(super) fn har_fill_response(entry: &mut HarEntry, response: &Value) {
    entry.status = response.get("status").and_then(|v| v.as_i64());
    entry.status_text = response
        .get("statusText")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    entry.mime_type = response
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    entry.http_version = response
        .get("protocol")
        .and_then(|v| v.as_str())
        .map(har_cdp_protocol_to_http_version)
        .unwrap_or_else(|| "HTTP/1.1".to_string());
    entry.response_headers = har_extract_headers(response.get("headers"));
    entry.redirect_url = entry
        .response_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    entry.response_body_size = response
        .get("encodedDataLength")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    entry.cdp_timing = response.get("timing").cloned();
    entry.remote_addr = response
        .get("remoteIPAddress")
        .and_then(|v| v.as_str())
        .and_then(|ip| {
            ip.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .zip(
            response
                .get("remotePort")
                .and_then(|v| v.as_u64())
                .and_then(|p| u16::try_from(p).ok()),
        )
        .map(|(ip, port)| std::net::SocketAddr::new(ip, port));
}

/// Compute HAR `timings` and total `time` (ms) from a CDP `ResourceTiming`
/// object and the optional `Network.loadingFinished` monotonic timestamp.
///
/// CDP timing values are milliseconds relative to `requestTime` (seconds since
/// browser start). A value of `-1` means the phase did not occur.
pub(super) fn har_compute_timings(
    cdp_timing: Option<&Value>,
    loading_finished_ts: Option<f64>,
) -> (Value, f64) {
    let Some(t) = cdp_timing else {
        return (json!({ "send": 0, "wait": 0, "receive": 0 }), 0.0);
    };

    let get = |key: &str| t.get(key).and_then(|v| v.as_f64()).unwrap_or(-1.0);

    let request_time = get("requestTime");
    let dns_start = get("dnsStart");
    let dns_end = get("dnsEnd");
    let connect_start = get("connectStart");
    let connect_end = get("connectEnd");
    let ssl_start = get("sslStart");
    let ssl_end = get("sslEnd");
    let send_start = get("sendStart");
    let send_end = get("sendEnd");
    let recv_headers_start = get("receiveHeadersStart");
    let recv_headers_end = get("receiveHeadersEnd");

    let dns = if dns_start >= 0.0 && dns_end >= 0.0 {
        dns_end - dns_start
    } else {
        -1.0
    };
    let connect = if connect_start >= 0.0 && connect_end >= 0.0 {
        connect_end - connect_start
    } else {
        -1.0
    };
    let ssl = if ssl_start >= 0.0 && ssl_end >= 0.0 {
        ssl_end - ssl_start
    } else {
        -1.0
    };
    let send = (send_end - send_start).max(0.0);

    // wait: end of sending → first byte of response headers.
    let wait_end = if recv_headers_start >= 0.0 {
        recv_headers_start
    } else {
        recv_headers_end
    };
    let wait = if send_end >= 0.0 && wait_end >= send_end {
        wait_end - send_end
    } else {
        0.0
    };

    // receive: first response byte → loading complete.
    // requestTime (seconds) + recv_headers_end (ms) / 1000 = absolute headers-end timestamp.
    let receive = loading_finished_ts
        .filter(|_| request_time >= 0.0 && recv_headers_end >= 0.0)
        .map(|lf_ts| {
            let recv_start_abs = request_time + recv_headers_end / 1000.0;
            ((lf_ts - recv_start_abs) * 1000.0).max(0.0)
        })
        .unwrap_or(0.0);

    let blocked = if dns_start > 0.0 {
        dns_start
    } else if connect_start > 0.0 {
        connect_start
    } else if send_start > 0.0 {
        send_start
    } else {
        -1.0
    };

    let total: f64 = [
        if blocked > 0.0 { blocked } else { 0.0 },
        if dns >= 0.0 { dns } else { 0.0 },
        if connect >= 0.0 { connect } else { 0.0 },
        send,
        wait,
        receive,
    ]
    .iter()
    .sum();

    let mut timings = json!({ "send": send, "wait": wait, "receive": receive });
    if blocked > 0.0 {
        timings["blocked"] = json!(blocked);
    }
    if dns >= 0.0 {
        timings["dns"] = json!(dns);
    }
    if connect >= 0.0 {
        timings["connect"] = json!(connect);
    }
    if ssl >= 0.0 {
        timings["ssl"] = json!(ssl);
    }

    (timings, total)
}

/// Format a Unix epoch timestamp (seconds, fractional) as RFC 3339 using the
/// `time` crate, e.g. `"2024-03-17T10:30:00.456Z"`.
pub(super) fn har_wall_time_to_rfc3339(wall_time: f64) -> String {
    if wall_time > 0.0 {
        let nanos = (wall_time * 1_000_000_000.0).round() as i128;
        if let Ok(dt) = OffsetDateTime::from_unix_timestamp_nanos(nanos) {
            if let Ok(s) = dt.format(&Rfc3339) {
                return s;
            }
        }
    }
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(super) fn har_output_path(explicit_path: Option<&str>) -> String {
    match explicit_path {
        Some(path) => path.to_string(),
        None => {
            let dir = get_har_dir();
            let _ = std::fs::create_dir_all(&dir);
            dir.join(format!("har-{}.har", unix_timestamp_millis()))
                .to_string_lossy()
                .to_string()
        }
    }
}

pub(super) fn get_har_dir() -> PathBuf {
    if let Some(home) = std::env::home_dir() {
        home.join(".frob").join("tmp").join("har")
    } else {
        std::env::temp_dir().join("frob").join("har")
    }
}

pub(super) fn unix_timestamp_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(super) async fn har_browser_metadata(state: &DaemonState) -> Option<Value> {
    let mgr = state.browser.as_ref()?;
    if !mgr.is_connection_alive().await {
        return None;
    }

    let version = mgr
        .client
        .send_command_no_params("Browser.getVersion", None)
        .await
        .ok()?;
    browser_metadata_from_version(&version)
}

pub(super) fn browser_metadata_from_version(version: &Value) -> Option<Value> {
    let product = version.get("product").and_then(|v| v.as_str())?;
    let (name, browser_version) = product.split_once('/').unwrap_or((product, ""));
    Some(json!({
        "name": name,
        "version": browser_version,
    }))
}

// ---------------------------------------------------------------------------
// Fetch interception resolver (domain filter + routes + origin headers)
// ---------------------------------------------------------------------------
