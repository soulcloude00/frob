use super::*;

pub(super) fn handle_flow_steps(_cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    if !state.flow.active() {
        return Err(NOT_CAPTURING.to_string());
    }
    Ok(state.flow.steps())
}

pub(super) const NOT_CAPTURING: &str =
    "this session is not capturing flows; pass `--capture flow` on the next command and the session relaunches with capture on";

pub(super) async fn handle_flow_save(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    if !state.flow.active() {
        return Err(NOT_CAPTURING.to_string());
    }
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("flow save requires a name")?
        .to_string();
    crate::native::flow::validate_name(&name)?;
    let params = crate::native::flow::parse_params(cmd)?;
    let login_wall = match state.browser.as_ref() {
        Some(mgr) => mgr
            .evaluate(
                "[...document.querySelectorAll('input[type=password]')].some(e => e.offsetParent !== null)",
                None,
            )
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        None => false,
    };
    // A request the range started may still be loading; let it finish so its
    // body is on the entry. Bounded by the command timeout.
    let from = cmd.get("from").and_then(|v| v.as_u64());
    let to = cmd.get("to").and_then(|v| v.as_u64());
    let deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(state.timeout_ms(cmd));
    // A response with headers whose body gets no bytes for the network idle
    // window is a stream or a long poll.
    let idle = std::time::Duration::from_millis(crate::native::browser::NETWORK_IDLE_MS);
    loop {
        state.drain_cdp_events_background().await?;
        let stalled = |e: &HarEntry| {
            state
                .har_progress
                .get(&e.request_id)
                .is_some_and(|t| t.elapsed() >= idle)
        };
        if !state
            .flow
            .awaiting_response(from, to, &state.har_entries, &stalled)
            || tokio::time::Instant::now() >= deadline
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
    }
    let still_loading = state.flow.loading_gaps(from, to, &state.har_entries);
    let entries = state.har_entries.clone();
    let mut local_names = Vec::new();
    let mut reach_note = None;
    if let Some((host, port, addrs)) = state.flow.start_host_addresses(from, to, &entries) {
        match crate::native::flow::recorded_local_host(&host, port, &addrs).await {
            crate::native::flow::RecordedReach::Local(reach) => local_names.push((host, reach)),
            crate::native::flow::RecordedReach::Unconfirmed(note) => reach_note = Some(note),
            crate::native::flow::RecordedReach::Unknown => {}
        }
    }
    let saved = state.flow.save(crate::native::flow::SaveArgs {
        name: &name,
        from: cmd.get("from").and_then(|v| v.as_u64()),
        to: cmd.get("to").and_then(|v| v.as_u64()),
        description: cmd.get("description").and_then(|v| v.as_str()),
        params: &params,
        output: cmd.get("output").and_then(|v| v.as_str()),
        entries: &entries,
        login_wall,
        local_names: &local_names,
    });
    // A save that fails for want of requests names the ones still loading.
    let mut report = match saved {
        Ok(report) => report,
        Err(e) if !still_loading.is_empty() => {
            let loading: Vec<String> = still_loading
                .iter()
                .filter_map(|g| Some(format!("step {}: {}", g["step"], g["reason"].as_str()?)))
                .collect();
            return Err(format!("{e}; {}", loading.join("; ")));
        }
        Err(e) => return Err(e),
    };
    // Requests still loading at the deadline are not in the flow. The step
    // that started one says so instead of the generic reason.
    if let Some(gaps) = report["gaps"].as_array_mut() {
        gaps.retain(|g| {
            g["kind"] != "no-requests" || !still_loading.iter().any(|l| l["step"] == g["step"])
        });
        gaps.extend(still_loading);
        for g in gaps.iter_mut() {
            if let Some(g) = g.as_object_mut() {
                g.remove("kind");
            }
        }
    }
    if let Some(note) = reach_note {
        report["reachNote"] = json!(note);
    }
    Ok(report)
}

/// The single write gate: the first write a run sends that `--allow-write`
/// does not cover and no confirmation has approved.
pub(super) fn uncovered_flow_write<'a>(
    flow: &'a crate::native::flow::Flow,
    allow: &crate::native::flow::AllowWrite,
    confirmed: &HashSet<String>,
) -> Option<&'a crate::native::flow::Write> {
    flow.run_writes()
        .into_iter()
        .find(|w| !allow.covers(&w.id) && !confirmed.contains(&w.action()))
}

/// The policy action to confirm before the run starts. An id the flow does
/// not have is left to the run to report.
pub(super) fn unconfirmed_flow_write(
    cmd: &Value,
    flow: &crate::native::flow::Flow,
    state: &DaemonState,
) -> Option<String> {
    let allow = crate::native::flow::AllowWrite::parse(cmd.get("allowWrite")).ok()?;
    allow.check(flow).ok()?;
    uncovered_flow_write(flow, &allow, &state.confirmed_policy_actions).map(|w| w.action())
}

pub(super) async fn handle_flow_run(
    cmd: &Value,
    state: &mut DaemonState,
    flow: crate::native::flow::Flow,
) -> Result<Value, String> {
    // Commands a run sends to the browser are not part of the recording.
    state.flow.set_suspended(true);
    let result = handle_flow_run_inner(cmd, state, flow).await;
    state.flow.set_suspended(false);
    result
}

pub(super) async fn handle_flow_run_inner(
    cmd: &Value,
    state: &mut DaemonState,
    mut flow: crate::native::flow::Flow,
) -> Result<Value, String> {
    let started = std::time::Instant::now();
    let run_deadline = started + std::time::Duration::from_millis(state.timeout_ms(cmd));
    let name = flow.name.clone();
    let name = name.as_str();
    let params = crate::native::flow::parse_params(cmd)?;
    let requests = crate::native::flow::prepare(&flow, &params)?;
    let allow = crate::native::flow::AllowWrite::parse(cmd.get("allowWrite"))?;
    allow.check(&flow)?;
    if let Some(w) = uncovered_flow_write(&flow, &allow, &state.confirmed_policy_actions) {
        return Err(format!(
            "flow '{}' sends {} {} ({}); rerun with --allow-write {} to send it",
            name,
            w.method,
            w.url,
            match w.kind {
                crate::native::flow::WriteKind::Explicit => "explicit",
                crate::native::flow::WriteKind::Suspected => "suspected",
            },
            w.id
        ));
    }

    let filter = state.domain_filter.read().await.clone();
    let hosts = crate::native::flow::HostPolicy::of(&flow);
    // Every request is checked before the first one is sent, on its template
    // with references unfilled; each is checked again once they are filled.
    for prepared in &requests {
        hosts
            .check(&prepared.url)
            .map_err(|e| format!("flow '{}' step {}: {}", name, prepared.step, e))?;
    }
    let client = crate::native::flow::http_client(filter.clone(), &flow).await?;
    let results = {
        let mut senders = DaemonSenders {
            wiring: FlowWiring::new(state, &allow),
            client,
            flow: &flow,
            params: &params,
            replay_run: None,
            run_deadline,
        };
        let check = |req: &crate::native::flow::PreparedRequest| -> Result<(), String> {
            hosts
                .check(&req.url)
                .map_err(|e| format!("flow '{}' step {}: {}", name, req.step, e))?;
            // Saved destinations follow the session's current domain
            // allowlist, not the one in force when the flow was recorded.
            match filter.as_ref().and_then(|f| f.check_url(&req.url).err()) {
                Some(reason) => Err(format!("flow '{}' step {}: {}", name, req.step, reason)),
                None => Ok(()),
            }
        };
        crate::native::flow::run_requests(&flow, &requests, &mut senders, &check).await?
    };

    // The run's own result comes first: writes it sent happened whether or
    // not the flow file can be updated, so a save error never hides it.
    if let Some(outcome) = crate::native::flow::RunOutcome::stopped(&results) {
        let saved = if crate::native::flow::unverify_on_shape_change(&mut flow, &results) {
            crate::native::flow::write_flow(&flow).err()
        } else {
            None
        };
        state.flow_stop = with_save_error(outcome.data(name), saved.as_deref());
        return Err(with_save_note(outcome.message(name), saved.as_deref()));
    }

    let shape = crate::native::flow::Shape::from_cmd(cmd);
    if let Some(body) = results
        .iter()
        .find(|r| r.ok && r.id == flow.output)
        .and_then(|r| r.body.as_ref())
    {
        let unknown = crate::native::flow::unknown_fields(body, &shape);
        if !unknown.is_empty() {
            return Err(format!(
                "no item has field(s) {}. Fields: {}",
                unknown.join(", "),
                flow.item_fields.join(", ")
            ));
        }
    }
    let now = chrono::Utc::now().to_rfc3339();
    let response = crate::native::flow::finish_run(
        &mut flow,
        &results,
        &now,
        started.elapsed().as_millis(),
        &shape,
    );
    let saved = crate::native::flow::write_flow(&flow).err();
    if response["ok"] != json!(true) {
        let outcome = crate::native::flow::RunOutcome::failed(&results, &flow);
        state.flow_stop = with_save_error(outcome.data(name), saved.as_deref());
        return Err(with_save_note(outcome.message(name), saved.as_deref()));
    }
    let mut response = response;
    if let Some(e) = saved {
        response["saveError"] = json!(e);
    }
    Ok(response)
}

/// A run's stop data, with the error that kept the flow file from updating.
pub(super) fn with_save_error(data: Option<Value>, saved: Option<&str>) -> Option<Value> {
    match (data, saved) {
        (Some(mut data), Some(e)) => {
            data["saveError"] = json!(e);
            Some(data)
        }
        (None, Some(e)) => Some(json!({ "saveError": e })),
        (data, None) => data,
    }
}

pub(super) fn with_save_note(message: String, saved: Option<&str>) -> String {
    match saved {
        Some(e) => format!("{message} (the flow file was not updated: {e})"),
        None => message,
    }
}

/// The focused field's current value, when the page exposes one.
pub(super) async fn flow_active_field_value(state: &DaemonState) -> Option<String> {
    let mgr = state.browser.as_ref()?;
    mgr.evaluate(
        "(() => { const e = document.activeElement; \
         return e && typeof e.value === 'string' ? e.value : null; })()",
        None,
    )
    .await
    .ok()?
    .as_str()
    .map(String::from)
}

/// What the focused element says about itself.
pub(super) async fn flow_active_field_traits(
    state: &DaemonState,
) -> Option<crate::native::flow::FieldTraits> {
    let mgr = state.browser.as_ref()?;
    let value = mgr
        .evaluate(
            "(() => { const e = document.activeElement; if (!e) return null; \
             const label = (e.labels && e.labels[0] && e.labels[0].innerText) || ''; \
             const a = (n) => (e.getAttribute && e.getAttribute(n)) || ''; \
             return { type: e.type || '', autocomplete: a('autocomplete'), \
             hint: [a('name'), e.id, a('aria-label'), a('placeholder'), label].filter(Boolean).join(' ') }; })()",
            None,
        )
        .await
        .ok()?;
    (!value.is_null()).then(|| crate::native::flow::FieldTraits::from_value(&value))
}

/// The page's own description of the field a `fill` or `type` writes to.
pub(super) async fn flow_field_traits(
    action: &str,
    cmd: &Value,
    state: &DaemonState,
) -> Option<crate::native::flow::FieldTraits> {
    let pastes =
        action == "clipboard" && cmd.get("operation").and_then(|v| v.as_str()) == Some("paste");
    let is_text_entry = pastes
        || matches!(action, "inserttext" | "press" | "keydown" | "keyup")
        || (action == "keyboard"
            && matches!(
                cmd.get("subaction").and_then(|v| v.as_str()),
                Some("type" | "insertText")
            ));
    if is_text_entry {
        return flow_active_field_traits(state).await;
    }
    if !matches!(action, "fill" | "type") {
        return None;
    }
    let selector = cmd.get("selector").and_then(|v| v.as_str())?;
    let mgr = state.browser.as_ref()?;
    let session_id = mgr.active_session_id().ok()?.to_string();
    let value = crate::native::element::get_field_traits(
        &mgr.client,
        &session_id,
        &state.ref_map,
        selector,
        &state.iframe_sessions,
    )
    .await
    .ok()?;
    Some(crate::native::flow::FieldTraits::from_value(&value))
}

/// CDP `resourceType` of a flow's direct request: the type Fetch.requestPaused
/// reports for the page tier's `fetch()` (Chrome reports it as XHR).
pub(super) const FLOW_DIRECT_RESOURCE_TYPE: &str = "XHR";

/// The route that answers a request: the first one, in registration order,
/// whose URL pattern and resource types match and that aborts or has a
/// response. Shared by browser interception and the flow HTTP tier.
pub(super) fn select_route<'a>(
    routes: &'a [RouteEntry],
    url: &str,
    resource_type: &str,
) -> Option<&'a RouteEntry> {
    routes.iter().find(|route| {
        route_url_matches(&route.url_pattern, url)
            && (route.resource_types.is_empty()
                || route
                    .resource_types
                    .iter()
                    .any(|rt| rt.eq_ignore_ascii_case(resource_type)))
            && (route.abort || route.response.is_some())
    })
}

/// The session route that would answer this request in the page, if any.
pub(super) async fn flow_routed_response(
    state: &DaemonState,
    url: &str,
) -> Option<crate::native::flow::RoutedResponse> {
    let routes = state.routes.read().await;
    flow_route_answer(&routes, url)
}

pub(super) fn flow_route_answer(
    routes: &[RouteEntry],
    url: &str,
) -> Option<crate::native::flow::RoutedResponse> {
    let route = select_route(routes, url, FLOW_DIRECT_RESOURCE_TYPE)?;
    if route.abort {
        return Some(crate::native::flow::RoutedResponse {
            abort: true,
            status: 0,
            body: String::new(),
        });
    }
    let resp = route.response.as_ref()?;
    Some(crate::native::flow::RoutedResponse {
        abort: false,
        status: resp.status.unwrap_or(200),
        body: resp.body.clone().unwrap_or_default(),
    })
}

pub(super) async fn flow_ensure_browser(state: &mut DaemonState) -> Result<(), String> {
    if state.browser.is_none() {
        auto_launch(state, Vec::new()).await?;
        state.webmcp_enabled = state.settings.webmcp();
    }
    Ok(())
}

/// A write only the command replay sends: the policy file can deny it, and
/// it needs `--allow-write` or a confirmation like any other write.
pub(super) fn replay_write_approved(
    state: &DaemonState,
    allow: &crate::native::flow::AllowWrite,
    write: &crate::native::flow::Write,
) -> Result<(), String> {
    let action = write.action();
    let confirmed = state.confirmed_policy_actions.contains(&action);
    if let Some(policy) = state.policy.as_ref() {
        match policy.check(&action) {
            PolicyResult::Allow => {}
            PolicyResult::Deny(reason) => return Err(format!("denied by policy: {}", reason)),
            PolicyResult::RequiresConfirmation if !confirmed => {
                return Err("the policy requires confirmation".to_string())
            }
            PolicyResult::RequiresConfirmation => {}
        }
    }
    if !confirmed
        && state
            .confirm_actions
            .as_ref()
            .is_some_and(|ca| ca.requires_confirmation(&action))
    {
        return Err("--confirm-actions requires confirmation".to_string());
    }
    if allow.covers(&write.id) || confirmed {
        Ok(())
    } else {
        Err(format!("rerun with --allow-write {} to allow it", write.id))
    }
}

/// The daemon behind the flow ports: the browser session, its cookies and
/// routes, and the commands a replay runs.
pub(super) struct FlowWiring<'a> {
    pub(super) state: &'a mut DaemonState,
    allow: &'a crate::native::flow::AllowWrite,
    started_har: bool,
    /// The session's HAR content mode, to put back after a replay that
    /// needed bodies the mode would not keep.
    restore_content: Option<HarContentMode>,
    mark: usize,
}

impl<'a> FlowWiring<'a> {
    pub(super) fn new(
        state: &'a mut DaemonState,
        allow: &'a crate::native::flow::AllowWrite,
    ) -> Self {
        FlowWiring {
            state,
            allow,
            started_har: false,
            restore_content: None,
            mark: 0,
        }
    }
}

#[async_trait::async_trait]
impl crate::native::flow::Browser for FlowWiring<'_> {
    async fn cookie_header(&mut self, url: &str) -> Option<String> {
        let mgr = self.state.browser.as_ref()?;
        let sid = mgr.active_session_id().ok()?;
        cookies::get_cookies(&mgr.client, sid, Some(vec![url.to_string()]))
            .await
            .ok()
            .map(|c| crate::native::flow::cookie_header(&c))
    }

    async fn routed_response(&mut self, url: &str) -> Option<crate::native::flow::RoutedResponse> {
        flow_routed_response(self.state, url).await
    }

    async fn tab_url(&mut self) -> Option<String> {
        match self.state.browser.as_ref() {
            Some(mgr) => mgr.get_url().await.ok(),
            None => None,
        }
    }

    async fn eval(&mut self, script: &str) -> Result<Value, String> {
        match self.state.browser.as_ref() {
            Some(mgr) => mgr.evaluate(script, None).await,
            None => Err("Browser not launched".to_string()),
        }
    }

    async fn ensure_browser(&mut self) -> Result<(), String> {
        flow_ensure_browser(self.state).await
    }

    async fn begin_capture(&mut self) -> Result<usize, String> {
        self.started_har = !self.state.har_recording;
        if self.started_har {
            handle_har_start(&json!({}), self.state).await?;
        }
        // Earlier tiers' responses may still be queued; they are not the
        // replay's, and they are drained before the mode changes so they
        // gain no bodies.
        let _ = self.state.drain_cdp_events_background().await;
        if !self.started_har && self.state.har_content_mode == HarContentMode::None {
            // The replay reads its answers from bodies, so it keeps them for
            // its own requests even when the session's HAR does not.
            self.restore_content = Some(HarContentMode::None);
            self.state.har_content_mode = HarContentMode::Text;
        }
        self.mark = self.state.har_entries.len();
        Ok(self.mark)
    }

    async fn run(&mut self, cmd: &Value) -> Value {
        Box::pin(execute_command(cmd, self.state)).await
    }

    async fn captured_since(&mut self, mark: usize) -> Vec<HarEntry> {
        let _ = self.state.drain_cdp_events_background().await;
        self.state.har_entries[mark.min(self.state.har_entries.len())..].to_vec()
    }

    async fn end_capture(&mut self) {
        let _ = self.state.drain_cdp_events_background().await;
        if let Some(mode) = self.restore_content.take() {
            // Entries from before the replay keep the no-body mode they had.
            let keep = self.mark.min(self.state.har_entries.len());
            for e in &mut self.state.har_entries[..keep] {
                let size = e.response_body.take().map_or(0, |b| b.len());
                e.response_body_base64 = false;
                self.state.har_body_total_bytes =
                    self.state.har_body_total_bytes.saturating_sub(size);
            }
            self.state.har_content_mode = mode;
        }
        if self.started_har {
            self.state.har_recording = false;
            self.state.har_entries.clear();
            self.state.har_body_total_bytes = 0;
        } else {
            // The session's own capture keeps only what it held before the run.
            let keep = self.mark.min(self.state.har_entries.len());
            let dropped: usize = self.state.har_entries[keep..]
                .iter()
                .map(|e| e.response_body.as_ref().map_or(0, |b| b.len()))
                .sum();
            self.state.har_entries.truncate(keep);
            self.state.har_body_total_bytes =
                self.state.har_body_total_bytes.saturating_sub(dropped);
        }
    }

    fn write_approved(&self, write: &crate::native::flow::Write) -> Result<(), String> {
        replay_write_approved(self.state, self.allow, write)
    }
}

/// One sender per tier over the same daemon wiring.
pub(super) struct DaemonSenders<'a> {
    wiring: FlowWiring<'a>,
    client: crate::native::flow::HttpClients,
    flow: &'a crate::native::flow::Flow,
    params: &'a [(String, String)],
    replay_run: Option<crate::native::flow::ReplayRun>,
    /// When the run must be done, so the CLI is still waiting for it.
    run_deadline: std::time::Instant,
}

#[async_trait::async_trait]
impl crate::native::flow::Senders for DaemonSenders<'_> {
    async fn send(
        &mut self,
        tier: crate::native::flow::Tier,
        req: &crate::native::flow::PreparedRequest,
        ctx: &crate::native::flow::SendCtx<'_>,
    ) -> crate::native::flow::TierResult {
        use crate::native::flow::{http, page, replay, Tier};
        match tier {
            Tier::Http => http::send(&self.client, &mut self.wiring, req, self.run_deadline).await,
            Tier::Page => page::send(self.flow, &mut self.wiring, req, self.run_deadline).await,
            Tier::Replay => {
                replay::send(
                    self.flow,
                    self.params,
                    &mut self.wiring,
                    &mut self.replay_run,
                    req,
                    ctx,
                    self.run_deadline,
                )
                .await
            }
        }
    }

    fn replay_ran(&self) -> bool {
        self.replay_run.is_some()
    }
}

pub(super) fn attach_flow_availability<'a>(
    resp: &'a mut Value,
    action: &'a str,
    state: &'a mut DaemonState,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(add_flow_availability(resp, action, state))
}

pub(super) async fn add_flow_availability(resp: &mut Value, action: &str, state: &mut DaemonState) {
    if !crate::native::flow::wants_hint(action, resp) {
        return;
    }
    let url = match state.browser.as_ref() {
        Some(mgr) => mgr.get_url().await.ok(),
        None => None,
    };
    crate::native::flow::attach_availability(
        resp,
        url.as_deref(),
        &mut state.flow_hinted,
        state.flow.saved(),
    );
}

pub(super) fn handle_flow_list(cmd: &Value) -> Result<Value, String> {
    let origin = cmd.get("origin").and_then(|v| v.as_str());
    Ok(json!({ "flows": crate::native::flow::list_flows(origin) }))
}

// ---------------------------------------------------------------------------
// HAR serialization helpers
// ---------------------------------------------------------------------------
