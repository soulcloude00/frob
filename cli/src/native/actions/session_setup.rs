use super::*;

/// Upper bound for preparing an iframe, worker, or page session that the
/// event drain or auto-attach reported. Every command drains CDP events
/// first, so this setup runs on the command path: a session that neither
/// answers nor reports its detach would otherwise hold every queued command
/// for the 30 s CDP timeout of each command it sends.
pub(super) const AUTO_ATTACHED_SETUP_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);

/// `kind` and `id` name what is being prepared in the timeout error, e.g.
/// `"session"` and a session id, or `"target"` and a target id when the
/// session does not exist yet.
pub(super) async fn bounded_auto_attached_setup(
    kind: &str,
    id: &str,
    setup: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    tokio::time::timeout(AUTO_ATTACHED_SETUP_TIMEOUT, setup)
        .await
        .unwrap_or_else(|_| Err(format!("Timed out preparing auto-attached {} {}", kind, id)))
}

/// What the background handler did with a session that auto-attach reported.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AutoAttachedSetup {
    /// Controls (if any) are installed and the target was resumed.
    Resumed,
    /// The session detached during setup; its target is gone.
    Detached,
    /// Setup failed; the target stays paused so it cannot run uncontrolled.
    Failed,
}

/// Installs network controls on a session that browser-level auto-attach
/// reported, then resumes it. This runs in the background handler that also
/// answers `Fetch.requestPaused` for the page, so a dead child session must
/// not stall it.
pub(super) async fn setup_auto_attached_session(
    client: &CdpClient,
    sid: &str,
    target_info: Option<&TargetInfo>,
    waiting_for_debugger: bool,
    domain_filter: Option<&DomainFilter>,
    has_proxy_creds: bool,
) -> AutoAttachedSetup {
    let target_needs_controls = target_info.is_some_and(target_supports_network_controls);
    let controls_active = domain_filter.is_some() || has_proxy_creds;
    let controls_result = match target_info {
        Some(target) if controls_active && target_needs_controls => {
            bounded_auto_attached_setup("session", sid, async {
                prepare_network_control_target_session(client, sid, target).await?;
                if target_is_worker_like(target) {
                    install_worker_network_controls_for_session(
                        client,
                        sid,
                        domain_filter,
                        has_proxy_creds,
                        target,
                    )
                    .await
                } else {
                    install_network_controls_for_session(
                        client,
                        sid,
                        domain_filter,
                        has_proxy_creds,
                        waiting_for_debugger,
                    )
                    .await
                }
            })
            .await
        }
        _ => Ok(()),
    };

    match controls_result {
        Ok(()) => {
            let _ = client
                .send_command_no_wait("Runtime.runIfWaitingForDebugger", None, Some(sid))
                .await;
            AutoAttachedSetup::Resumed
        }
        Err(error) if is_session_detached_error(&error) => {
            log_detached_during_setup("auto-attached", &error);
            AutoAttachedSetup::Detached
        }
        Err(error) => {
            eprintln!(
                "Failed to apply browser network controls to auto-attached target: {}",
                error
            );
            AutoAttachedSetup::Failed
        }
    }
}

/// An auto-attached session that detached during setup took its target with
/// it, so there is nothing left to control or resume. Short-lived iframes do
/// this routinely; it is not worth a warning.
pub(super) fn log_detached_during_setup(kind: &str, error: &str) {
    if env::var_os("FROB_DEBUG").is_some() {
        let _ = writeln!(
            std::io::stderr(),
            "[cdp] {} session detached during setup: {}",
            kind,
            error
        );
    }
}

pub(super) async fn prepare_auto_attached_session(
    client: &CdpClient,
    session_id: &str,
) -> Result<(), String> {
    client
        .send_command_no_params("Page.enable", Some(session_id))
        .await?;
    client
        .send_command_no_params("Runtime.enable", Some(session_id))
        .await?;
    client
        .send_command_no_params("Network.enable", Some(session_id))
        .await?;
    let _ = client
        .send_command(
            "Target.setAutoAttach",
            Some(json!({
                "autoAttach": true,
                "waitForDebuggerOnStart": true,
                "flatten": true
            })),
            Some(session_id),
        )
        .await;
    Ok(())
}

pub(super) fn target_is_worker_like(target: &TargetInfo) -> bool {
    matches!(
        target.target_type.as_str(),
        "worker" | "service_worker" | "shared_worker"
    )
}

pub(super) fn target_supports_worker_fetch_controls(target: &TargetInfo) -> bool {
    target.target_type == "service_worker"
}

pub(super) fn target_supports_network_controls(target: &TargetInfo) -> bool {
    target.target_type == "iframe" || should_track_target(target) || target_is_worker_like(target)
}

pub(super) async fn prepare_network_control_target_session(
    client: &CdpClient,
    session_id: &str,
    target: &TargetInfo,
) -> Result<(), String> {
    if target_is_worker_like(target) {
        Ok(())
    } else {
        prepare_auto_attached_session(client, session_id).await
    }
}

pub(super) async fn install_network_controls_for_session(
    client: &CdpClient,
    session_id: &str,
    filter: Option<&DomainFilter>,
    handle_auth_requests: bool,
    paused_before_document: bool,
) -> Result<(), String> {
    if let Some(filter) = filter {
        network::install_domain_filter(
            client,
            session_id,
            &filter.allowed_domains,
            handle_auth_requests,
            paused_before_document,
        )
        .await?;
    } else if handle_auth_requests {
        network::install_domain_filter_fetch(client, session_id, true).await?;
    }

    Ok(())
}

pub(super) async fn install_worker_network_controls_for_session(
    client: &CdpClient,
    session_id: &str,
    filter: Option<&DomainFilter>,
    handle_auth_requests: bool,
    target: &TargetInfo,
) -> Result<(), String> {
    if filter.is_some() {
        if target_supports_worker_fetch_controls(target) {
            network::install_domain_filter_fetch(client, session_id, handle_auth_requests).await?;
        }
    } else if handle_auth_requests && target_supports_worker_fetch_controls(target) {
        network::install_domain_filter_fetch(client, session_id, true).await?;
    }

    Ok(())
}

pub(super) async fn install_active_network_controls(
    state: &mut DaemonState,
    handle_auth_requests: bool,
) -> Result<(), String> {
    let filter = state.domain_filter.read().await.clone();
    if !network_controls_required(filter.as_ref(), handle_auth_requests) {
        return Ok(());
    }

    let direct_page = {
        let mgr = state
            .browser
            .as_ref()
            .ok_or("Browser is not available for network control installation")?;
        mgr.is_direct_page_connection()
    };
    if direct_page && filter.is_some() {
        return Err(direct_page_allowed_domains_error());
    }

    if !state.network_auto_attach_installed && !direct_page {
        {
            let mgr = state
                .browser
                .as_ref()
                .ok_or("Browser is not available for network control installation")?;
            mgr.enable_browser_auto_attach_pub().await?;
        }
        state.network_auto_attach_installed = true;
    }

    let mgr = state
        .browser
        .as_ref()
        .ok_or("Browser is not available for network control installation")?;
    let session_ids = network_control_session_ids(mgr)?;

    for session_id in session_ids {
        mgr.prepare_domains_pub(&session_id).await?;
        install_network_controls_for_session(
            &mgr.client,
            &session_id,
            filter.as_ref(),
            handle_auth_requests,
            false,
        )
        .await?;
        mgr.resume_if_waiting_pub(&session_id).await?;
    }

    if let Some(ref filter) = filter {
        network::sanitize_existing_pages(&mgr.client, &mgr.pages_list(), filter).await;
    }

    Ok(())
}

/// The wedged tab was replaced by a blank one: drop the old tab's refs and
/// frame scope, and give the new tab the session setup and network controls
/// (headers, routes, domain filter) that `tab new` installs.
pub(super) async fn adopt_replacement_tab(
    state: &mut DaemonState,
    old_session_id: &str,
) -> Result<(), String> {
    state.webmcp.clear_page_scope(old_session_id);
    state.webmcp.observations.remove(old_session_id);
    state.ref_map.begin_snapshot();
    state.active_iframe_sessions.clear();
    state.active_frame_id = None;
    state.webmcp.clear_invocations();
    let session_id = state
        .browser
        .as_ref()
        .ok_or("Browser not launched")?
        .active_session_id()?
        .to_string();
    let has_proxy_creds = state.proxy_credentials.read().await.is_some();
    apply_session_setup(state, &session_id).await?;
    install_network_controls_or_close(state, has_proxy_creds).await?;
    state.drain_cdp_events_background().await?;
    state.refresh_active_iframe_sessions().await;
    Ok(())
}

/// Replacing a wedged tab opens one tab and closes another, so it runs only
/// when policy would let the agent do both itself without a confirmation.
pub(super) fn may_replace_wedged_tab(state: &DaemonState) -> bool {
    ["tab_new", "tab_close"].iter().all(|action| {
        let confirmed = state.confirmed_policy_actions.contains(*action);
        let policy_ok = state
            .policy
            .as_ref()
            .is_none_or(|policy| match policy.check(action) {
                PolicyResult::Allow => true,
                PolicyResult::RequiresConfirmation => confirmed,
                PolicyResult::Deny(_) => false,
            });
        let category_ok = confirmed
            || !state
                .confirm_actions
                .as_ref()
                .is_some_and(|ca| ca.requires_confirmation(action));
        policy_ok && category_ok
    })
}

/// Add to the response's warning instead of replacing one already set.
pub(super) fn append_warning(resp: &mut Value, warning: &str) {
    let merged = match resp.get("warning").and_then(Value::as_str) {
        Some(existing) => format!("{}\n{}", existing, warning),
        None => warning.to_string(),
    };
    resp["warning"] = json!(merged);
}

pub(super) async fn install_network_controls_or_close(
    state: &mut DaemonState,
    handle_auth_requests: bool,
) -> Result<(), String> {
    if let Err(error) = install_active_network_controls(state, handle_auth_requests).await {
        return close_after_network_control_failure(state, error).await;
    }
    Ok(())
}

pub(super) async fn install_network_controls_or_resume_prepared_session(
    state: &mut DaemonState,
    handle_auth_requests: bool,
    session_id: &str,
) -> Result<(), String> {
    let filter = state.domain_filter.read().await.clone();
    if network_controls_required(filter.as_ref(), handle_auth_requests) {
        install_network_controls_or_close(state, handle_auth_requests).await
    } else {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        mgr.resume_if_waiting_pub(session_id).await
    }
}

pub(super) const BROWSER_USE_SETUP_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(18);

pub(super) async fn browser_use_setup_timeout(state: &mut DaemonState) -> String {
    let message = "Browser Use setup timed out; inspect Browser Use Cloud before retrying";
    match tokio::time::timeout(
        std::time::Duration::from_secs(4),
        close_current_browser(state),
    )
    .await
    {
        Ok(Ok(())) => message.to_string(),
        Ok(Err(error)) => combine_provider_failure(message.to_string(), Some(error)),
        Err(_) => {
            let recovery = state
                .active_provider_session
                .as_ref()
                .and_then(|active| {
                    (active.session.provider == "browser-use")
                        .then(|| uuid::Uuid::parse_str(&active.session.session_id).ok())
                        .flatten()
                })
                .map(|id| {
                    format!(
                        "; browser {} remains pending; retry close or stop it in Browser Use Cloud",
                        id
                    )
                })
                .unwrap_or_default();
            format!("{}; cleanup timed out{}", message, recovery)
        }
    }
}

/// True when [`apply_session_setup`] has anything to replay onto a new tab.
pub(super) async fn session_setup_pending(state: &DaemonState) -> bool {
    !state.session_setup.is_empty()
        || !state.routes.read().await.is_empty()
        || !state.origin_headers.read().await.is_empty()
}

/// Replay the session-scoped setup the user configured on the active page
/// (see [`SessionSetup`]) onto a tab the daemon just created, plus the
/// `Fetch.enable` needed for `route` and origin-scoped `--headers` to reach
/// the shared fetch handler from that tab.
///
/// Call after `Page`/`Network` are enabled on `session_id` and before its
/// first navigation, so init scripts, UA, and headers cover the initial
/// document. Emulation failures are ignored: a call the engine rejects
/// should not abort creating the tab.
pub(super) async fn apply_session_setup(
    state: &mut DaemonState,
    session_id: &str,
) -> Result<(), String> {
    let client = state
        .browser
        .as_ref()
        .ok_or("Browser not launched")?
        .client
        .clone();
    let setup = state.session_setup.clone();

    if let Some(ref ua) = setup.user_agent {
        let _ = client
            .send_command(
                "Emulation.setUserAgentOverride",
                Some(json!({ "userAgent": ua })),
                Some(session_id),
            )
            .await;
    }

    if let Some(ref emulated) = setup.emulated_media {
        let mut params = json!({});
        if let Some(ref m) = emulated.media {
            params["media"] = Value::String(m.clone());
        }
        if !emulated.features.is_empty() {
            params["features"] = Value::Array(
                emulated
                    .features
                    .iter()
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect(),
            );
        }
        let _ = client
            .send_command("Emulation.setEmulatedMedia", Some(params), Some(session_id))
            .await;
    }

    if let Some(ref tz) = setup.timezone {
        let _ = client
            .send_command(
                "Emulation.setTimezoneOverride",
                Some(json!({ "timezoneId": tz })),
                Some(session_id),
            )
            .await;
    }

    if let Some(ref locale) = setup.locale {
        let _ = client
            .send_command(
                "Emulation.setLocaleOverride",
                Some(json!({ "locale": locale })),
                Some(session_id),
            )
            .await;
    }

    if let Some((lat, lon, accuracy)) = setup.geolocation {
        let _ = client
            .send_command(
                "Emulation.setGeolocationOverride",
                Some(json!({
                    "latitude": lat,
                    "longitude": lon,
                    "accuracy": accuracy.unwrap_or(1.0),
                })),
                Some(session_id),
            )
            .await;
    }

    if let Some(ref headers) = setup.extra_headers {
        network::set_extra_headers(&client, session_id, headers).await?;
    }

    if let Some(offline) = setup.offline {
        network::set_offline(&client, session_id, offline).await?;
    }

    for (index, script) in setup.init_scripts.iter().enumerate() {
        let result = client
            .send_command(
                "Page.addScriptToEvaluateOnNewDocument",
                Some(json!({ "source": &script.source })),
                Some(session_id),
            )
            .await?;
        let identifier = result
            .get("identifier")
            .and_then(Value::as_str)
            .ok_or("Page.addScriptToEvaluateOnNewDocument returned no identifier")?;
        let tracked = state
            .session_setup
            .init_scripts
            .get_mut(index)
            .ok_or("Init script setup changed while it was being applied")?;
        tracked
            .session_identifiers
            .insert(session_id.to_string(), identifier.to_string());
    }

    // `route` and origin-scoped `--headers` are resolved by the background
    // fetch handler for any session, but only sessions with Fetch enabled
    // emit requestPaused. Domain filtering and proxy auth install their own
    // Fetch.enable; this covers the case where neither is active.
    let has_routes = !state.routes.read().await.is_empty();
    let has_origin_headers = !state.origin_headers.read().await.is_empty();
    if has_routes || has_origin_headers {
        let patterns = build_fetch_patterns(state).await;
        let params = build_fetch_enable_params(state, patterns).await;
        client
            .send_command("Fetch.enable", Some(params), Some(session_id))
            .await?;
    }

    Ok(())
}
