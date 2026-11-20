use super::*;

/// Persist the session's tab binding when it changed. Called after every
/// browser-touching command; skipped while the bound tab is gone so the
/// stale binding (needed to re-derive the tab_gone state after a daemon
/// restart) is not overwritten by the fallback tab.
///
/// Returns the write error, if any. `last_persisted_binding` is only updated
/// after a successful durable write, so a transient failure is retried on
/// the next command instead of being silently treated as persisted (which
/// would let a daemon restart drop the binding and, for a pinned session,
/// the strict isolation boundary with it).
pub(super) fn maybe_persist_tab_binding(state: &mut DaemonState) -> Option<String> {
    let mgr = state.browser.as_ref()?;
    let (target_id, url) = mgr.binding_snapshot()?;
    let binding = tab_binding::TabBinding {
        target_id,
        url: tab_binding::sanitize_url(&url),
        pinned: state.pin_tab,
    };
    if state.last_persisted_binding.as_ref() == Some(&binding) {
        return None;
    }
    match tab_binding::save(&state.session_id, &binding) {
        Ok(()) => {
            state.last_persisted_binding = Some(binding);
            None
        }
        Err(e) => Some(e),
    }
}

/// Re-apply this session's persisted tab binding after attaching to a
/// browser over CDP, instead of keeping the default selection (the first
/// target returned by `Target.getTargets`, i.e. the browser's most recently
/// active tab, which in a shared browser is usually another session's tab).
///
/// Returns true when the session ends up with an established selection:
/// re-bound to the persisted target, entered the `tab_gone` state (pin-tab,
/// bound tab dead), or bound to a fresh tab (pin-tab, no prior binding).
/// Returns false when no binding existed and the legacy selection stands.
pub(super) async fn apply_tab_binding_on_attach(state: &mut DaemonState) -> Result<bool, String> {
    let session = state.session_id.clone();
    // A corrupt or unreadable binding file is a recovery error, not a
    // first-time session: it may have carried pinned=true, and silently
    // starting fresh would drop the strict isolation boundary.
    let binding = tab_binding::load(&session).map_err(|e| {
        format!(
            "{} — delete the file to reset the session's tab binding \
             (re-run with --pin-tab afterwards if the session was pinned)",
            e
        )
    })?;
    if binding.as_ref().is_some_and(|b| b.pinned) {
        state.pin_tab = true;
    }
    let pin = state.pin_tab;
    if state.browser.is_none() {
        return Ok(false);
    }
    if let Some(ref mut mgr) = state.browser {
        mgr.set_pin_tab(pin);
    }
    let established = match binding {
        Some(b) => {
            let restored = state
                .browser
                .as_mut()
                .map(|mgr| mgr.restore_target_binding(&b.target_id, &b.url))
                .unwrap_or(false);
            if restored {
                // Attach only enables the CDP domains on the first target; the
                // restored tab needs them too (like tab_switch), or
                // lifecycle-dependent commands such as navigate hang. The tab
                // may have been discarded by Memory Saver, so revive it before
                // enabling domains instead of hanging on Page.enable.
                if let Some(ref mgr) = state.browser {
                    mgr.revive_and_enable_active().await?;
                }
            }
            if restored || pin {
                // Restored, or entered the tab_gone state (the stale binding
                // file is intentionally kept so the state is re-derived if
                // the daemon restarts again before the agent re-binds).
                true
            } else {
                // Legacy sessions fall back to the first target; drop the
                // stale binding.
                tab_binding::clear(&session);
                state.last_persisted_binding = None;
                false
            }
        }
        None => {
            if pin {
                // A pinned session never implicitly adopts an existing tab:
                // start it on a fresh one.
                if let Some(ref mut mgr) = state.browser {
                    mgr.tab_new(None, None).await?;
                }
                true
            } else {
                false
            }
        }
    };
    if established {
        if let Some(err) = maybe_persist_tab_binding(state) {
            // Strict pinning is only as durable as the binding file: fail
            // the attach instead of pretending the pin is in place.
            if state.pin_tab {
                return Err(format!(
                    "cannot persist the pinned tab binding: {} — \
                     --pin-tab requires a writable socket directory",
                    err
                ));
            }
        }
    }
    Ok(established)
}

/// Run `apply_tab_binding_on_attach`, but tear the connection down on failure.
/// The attach paths set `state.browser` before calling this; without the
/// rollback a binding-recovery error would leave a live but un-recovered
/// browser, and the next command would skip the attach path and act on the
/// wrong tab.
pub(super) async fn apply_tab_binding_on_attach_or_rollback(
    state: &mut DaemonState,
) -> Result<bool, String> {
    match apply_tab_binding_on_attach(state).await {
        Ok(v) => Ok(v),
        Err(e) => {
            let _ = rollback_failed_launch(state).await;
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Auto-launch
// ---------------------------------------------------------------------------

/// Open a fresh tab so auto-connect navigations don't hijack the user's
/// existing tabs. Used when no persisted binding re-established a tab.
pub(super) async fn open_fresh_tab_for_auto_connect(state: &mut DaemonState) -> Result<(), String> {
    let Some(ref mut mgr) = state.browser else {
        return Ok(());
    };
    mgr.tab_new(None, None).await?;
    let session_id = mgr.active_session_id()?.to_string();
    let _ = mgr
        .client
        .send_command("Page.bringToFront", None, Some(&session_id))
        .await;
    // Best effort: a failed write is retried after the next command.
    let _ = maybe_persist_tab_binding(state);
    Ok(())
}

pub(super) async fn current_allowed_domains(state: &DaemonState) -> Vec<String> {
    state
        .domain_filter
        .read()
        .await
        .as_ref()
        .map(|filter| filter.allowed_domains.clone())
        .unwrap_or_default()
}

pub(super) fn network_control_session_ids_from_pages(
    pages: &[crate::native::browser::PageInfo],
    active_session_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut session_ids: Vec<String> = Vec::new();
    for page in pages {
        if !session_ids.iter().any(|sid| sid == &page.session_id) {
            session_ids.push(page.session_id.clone());
        }
    }

    if session_ids.is_empty() {
        let active = active_session_id.ok_or("No active page")?;
        session_ids.push(active.to_string());
    }

    Ok(session_ids)
}

pub(super) fn network_control_session_ids(mgr: &BrowserManager) -> Result<Vec<String>, String> {
    let pages = mgr.pages_list();
    let active_session_id = if pages.is_empty() {
        Some(mgr.active_session_id()?)
    } else {
        None
    };
    network_control_session_ids_from_pages(&pages, active_session_id)
}

pub(super) fn should_blank_existing_url(url: &str, filter: &DomainFilter) -> bool {
    if url.is_empty() || url == "about:blank" {
        return false;
    }
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .host_str()
                .map(|hostname| !filter.is_allowed(hostname))
        })
        .unwrap_or(false)
}

pub(super) fn check_url_allowed_by_filter(
    filter: Option<&DomainFilter>,
    url: &str,
) -> Result<(), String> {
    if let Some(filter) = filter {
        if url != "about:blank" {
            filter.check_url(url)?;
        }
    }
    Ok(())
}

pub(super) fn network_controls_required(
    filter: Option<&DomainFilter>,
    handle_auth_requests: bool,
) -> bool {
    filter.is_some() || handle_auth_requests
}

pub(super) fn should_defer_url_until_network_controls(
    filter: Option<&DomainFilter>,
    handle_auth_requests: bool,
    url: Option<&str>,
) -> Result<bool, String> {
    let Some(url) = url else {
        return Ok(false);
    };

    check_url_allowed_by_filter(filter, url)?;
    Ok(filter.is_some_and(|filter| !filter.allowed_domains.is_empty()) || handle_auth_requests)
}

pub(super) struct AllowedDomainsLaunchSupport<'a> {
    allowed_domains: &'a [String],
    cdp_url: Option<&'a str>,
    cdp_port: Option<u64>,
    auto_connect: bool,
    profile: Option<&'a str>,
    provider_name: Option<&'a str>,
    args: &'a [String],
    restore_key: Option<&'a str>,
    storage_state: Option<&'a str>,
}

pub(super) fn ensure_allowed_domains_supported_for_launch(
    support: AllowedDomainsLaunchSupport<'_>,
) -> Result<(), String> {
    if support.allowed_domains.is_empty() {
        return Ok(());
    }

    if support
        .restore_key
        .is_some_and(|key| !key.trim().is_empty())
    {
        return Err(
            "--allowed-domains is not supported with --restore because saved state can replay origins before frob can verify they are in the allowlist"
                .to_string(),
        );
    }

    if support
        .storage_state
        .is_some_and(|path| !path.trim().is_empty())
    {
        return Err(
            "--allowed-domains is not supported with --state/storageState because loading state replays saved origins"
                .to_string(),
        );
    }

    if support.cdp_url.is_some() || support.cdp_port.is_some() {
        return Err(
            "--allowed-domains is not supported with --cdp because WebRTC containment cannot be installed before existing page scripts run"
                .to_string(),
        );
    }

    if support.auto_connect {
        return Err(
            "--allowed-domains is not supported with --auto-connect because WebRTC containment cannot be installed before existing page scripts run"
                .to_string(),
        );
    }

    if support.profile.is_some() {
        return Err(
            "--allowed-domains is not supported with --profile because Chrome may restore existing pages before network containment is installed"
                .to_string(),
        );
    }

    if let Some(provider) = support.provider_name {
        match provider.to_lowercase().as_str() {
            "ios" => {
                return Err(
                    "--allowed-domains is not supported with the iOS provider because WebRTC containment cannot be enforced"
                        .to_string(),
                );
            }
            "safari" => {
                return Err(
                    "--allowed-domains is not supported with the Safari provider because WebRTC containment cannot be enforced"
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    if let Some(arg) = allowed_domains_disallowed_chrome_arg(support.args) {
        return Err(format!(
            "--allowed-domains is not supported with --args containing {} because Chrome may restore or open pages before network containment is installed",
            arg
        ));
    }

    Ok(())
}

pub(super) fn direct_page_allowed_domains_error() -> String {
    "--allowed-domains is not supported with direct-page browser providers because worker and popup containment require browser-level Target auto-attach"
        .to_string()
}

pub(super) async fn ensure_state_replay_supported_by_active_domain_filter(
    state: &DaemonState,
    source: &str,
) -> Result<(), String> {
    let filter = state.domain_filter.read().await;
    if filter
        .as_ref()
        .is_some_and(|filter| !filter.allowed_domains.is_empty())
    {
        return Err(format!(
            "--allowed-domains is not supported with {} because loading state replays saved origins",
            source
        ));
    }
    Ok(())
}

pub(super) async fn restore_domain_filter(state: &mut DaemonState, filter: &Option<DomainFilter>) {
    let mut current = state.domain_filter.write().await;
    *current = filter.clone();
}

pub(super) fn chrome_switch_name(arg: &str) -> Option<&str> {
    let trimmed = arg.trim();
    trimmed
        .strip_prefix("--")
        .or_else(|| trimmed.strip_prefix('/'))
        .or_else(|| trimmed.strip_prefix('-'))
        .and_then(|switch| switch.split(['=', ' ']).next())
        .filter(|name| !name.is_empty() && !name.contains(['/', '\\']))
}

pub(super) fn is_startup_url_arg(arg: &str) -> bool {
    let trimmed = arg.trim().to_ascii_lowercase();
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
}

pub(super) fn is_positional_chrome_arg(arg: &str) -> bool {
    !arg.trim().is_empty() && chrome_switch_name(arg).is_none()
}

/// Raw Chrome args can select an existing profile or open startup pages before
/// CDP interception and init scripts are installed.
pub(super) fn allowed_domains_disallowed_chrome_arg(args: &[String]) -> Option<&'static str> {
    for arg in args {
        if is_startup_url_arg(arg) {
            return Some("a startup URL");
        }
        if is_positional_chrome_arg(arg) {
            return Some("a startup URL or path");
        }
        let Some(name) = chrome_switch_name(arg) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "user-data-dir" => return Some("--user-data-dir"),
            "profile-directory" => return Some("--profile-directory"),
            "restore-last-session" => return Some("--restore-last-session"),
            "restore-session" => return Some("--restore-session"),
            "app" => return Some("--app"),
            "app-id" => return Some("--app-id"),
            "app-launch-url-for-shortcuts-menu-item" => {
                return Some("--app-launch-url-for-shortcuts-menu-item")
            }
            "load-and-launch-app" => return Some("--load-and-launch-app"),
            _ => {}
        }
    }

    None
}

pub(super) async fn auto_launch(
    state: &mut DaemonState,
    plugins: Vec<crate::plugins::PluginConfig>,
) -> Result<Value, String> {
    auto_launch_with(state, plugins, &Value::Null, false).await
}

/// Launches or attaches the session's browser from its settings. `cmd` is the
/// launch command that asked for it, when there is one; it carries only what
/// the settings do not (device selection for the WebDriver providers).
///
/// `strict_state` is set when the command named a `--state` file: a file that
/// cannot be loaded then fails the launch instead of launching without state.
pub(super) async fn auto_launch_with(
    state: &mut DaemonState,
    plugins: Vec<crate::plugins::PluginConfig>,
    cmd: &Value,
    strict_state: bool,
) -> Result<Value, String> {
    if state.attached_target.is_some() && cmd.is_null() {
        // Resume the attached browser; the launch handler resolves the target.
        let launch = json!({ "action": "launch", "plugins": plugins });
        return Box::pin(handle_launch(&launch, state)).await;
    }
    let connection = state.settings.connection();
    if matches!(connection, Connection::Cdp(_) | Connection::AutoConnect)
        || !matches!(&connection, Connection::Provider(provider) if provider_is_browser_use(provider))
    {
        return auto_launch_inner(state, plugins, cmd, strict_state).await;
    }
    let credentials = state.proxy_credentials.read().await.clone();
    let scripts = state.plugin_init_scripts.clone();
    let setup = state.session_setup.clone();
    let result = match tokio::time::timeout(
        BROWSER_USE_SETUP_DEADLINE,
        auto_launch_inner(state, plugins, cmd, strict_state),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(browser_use_setup_timeout(state).await),
    };
    if result.is_err() {
        *state.proxy_credentials.write().await = credentials;
        state.plugin_init_scripts = scripts;
        state.session_setup = setup;
    }
    result
}

pub(super) async fn auto_launch_inner(
    state: &mut DaemonState,
    plugins: Vec<crate::plugins::PluginConfig>,
    cmd: &Value,
    strict_state: bool,
) -> Result<Value, String> {
    if has_active_browser_session(state) {
        close_before_implicit_relaunch(state).await?;
    }
    let mut options = state.settings.launch_options();
    let effective_ca_cert = state.effective_ca_cert.clone();
    apply_effective_ca_cert(&mut options, &effective_ca_cert);
    state.plugin_init_scripts.clear();
    state.session_setup = SessionSetup::default();

    // Use the stream server's viewport dimensions for --window-size so the
    // content area matches the desired viewport from the start.
    if let Some(ref server) = state.stream_server {
        options.viewport_size = Some(server.viewport().await);
    }
    let engine = state.settings.engine();
    let connection = state.settings.connection();
    let cdp = match &connection {
        Connection::Cdp(cdp) => Some(cdp.clone()),
        _ => None,
    };
    let auto_connect = matches!(connection, Connection::AutoConnect);
    let provider = match &connection {
        Connection::Provider(provider) => Some(provider.clone()),
        _ => None,
    };
    validate_ca_cert_launch_mode(
        &options,
        engine.as_deref(),
        !matches!(connection, Connection::Local),
    )?;
    let enable_features = state.settings.enable_features();
    let init_script_paths = state.settings.init_scripts();
    let allowed_domains = current_allowed_domains(state).await;
    options.restrict_webrtc = !allowed_domains.is_empty();
    // Extract storage_state before options is moved into BrowserManager::launch.
    let storage_state_path = options.storage_state.clone();
    let restore_key = state.session_name.clone();

    // Store proxy credentials for Fetch.authRequired handling
    // Only a browser frob launches itself gets them: an external
    // browser is never answered proxy challenges on the session's behalf.
    let local = matches!(connection, Connection::Local);
    let has_proxy_auth = local && options.proxy_username.is_some();
    if has_proxy_auth {
        let mut creds = state.proxy_credentials.write().await;
        *creds = Some((
            options.proxy_username.clone().unwrap_or_default(),
            options.proxy_password.clone().unwrap_or_default(),
        ));
    } else if !local {
        *state.proxy_credentials.write().await = None;
    }
    // Nor is a state file replayed into one.
    let storage_state_path = if local { storage_state_path } else { None };
    let storage_state = storage_state_path.as_deref();

    // An attached browser is whatever it is: an inherited engine name would
    // mislabel it, so only a local launch or a provider reads the engine.
    let engine = if matches!(connection, Connection::Cdp(_) | Connection::AutoConnect) {
        None
    } else {
        engine
    };
    state.engine = engine.as_deref().unwrap_or("chrome").to_string();
    write_engine_file(&state.session_id, &state.engine);
    write_extensions_file(&state.session_id, &state.settings);

    if let Some(cdp) = cdp {
        ensure_allowed_domains_supported_for_launch(AllowedDomainsLaunchSupport {
            allowed_domains: &allowed_domains,
            cdp_url: Some(cdp.as_str()),
            cdp_port: None,
            auto_connect: false,
            profile: options.profile.as_deref(),
            provider_name: None,
            args: &options.args,
            restore_key: restore_key.as_deref(),
            storage_state,
        })?;
        let headers =
            cdp_headers_for(state, &cdp).or_else(|| cdp_headers_from_settings(&state.settings));
        let mgr = BrowserManager::connect_cdp_when_listening(&cdp, headers.clone()).await?;
        state.cdp_headers = headers.map(|headers| (cdp.clone(), headers));
        state.reset_input_state();
        state.browser = Some(mgr);
        state.launch_identity = Some(state.settings.launch_identity());
        state.subscribe_to_browser_events();
        state.start_fetch_handler();
        state.start_dialog_handler();
        apply_tab_binding_on_attach_or_rollback(state).await?;
        state.update_stream_client().await;
        install_network_controls_or_close(state, has_proxy_auth).await?;
        apply_launch_init_scripts(state, &enable_features, &init_script_paths).await;
        try_auto_restore_state(state).await;
        load_storage_state_for_launch(state, &storage_state_path, strict_state).await?;
        return Ok(json!({}));
    }

    if auto_connect {
        ensure_allowed_domains_supported_for_launch(AllowedDomainsLaunchSupport {
            allowed_domains: &allowed_domains,
            cdp_url: None,
            cdp_port: None,
            auto_connect: true,
            profile: options.profile.as_deref(),
            provider_name: None,
            args: &options.args,
            restore_key: restore_key.as_deref(),
            storage_state,
        })?;
        state.reset_input_state();
        state.browser = Some(BrowserManager::connect_auto().await?);
        if !apply_tab_binding_on_attach_or_rollback(state).await? {
            if let Err(e) = open_fresh_tab_for_auto_connect(state).await {
                let _ = rollback_failed_launch(state).await;
                return Err(e);
            }
        }
        state.launch_identity = Some(state.settings.launch_identity());
        state.subscribe_to_browser_events();
        state.start_fetch_handler();
        state.start_dialog_handler();
        state.update_stream_client().await;
        install_network_controls_or_close(state, has_proxy_auth).await?;
        apply_launch_init_scripts(state, &enable_features, &init_script_paths).await;
        try_auto_restore_state(state).await;
        load_storage_state_for_launch(state, &storage_state_path, strict_state).await?;
        return Ok(json!({}));
    }

    // Cloud provider: when the session's settings name a provider, connect via
    // the provider API instead of launching a local Chrome instance, so a
    // command that arrives before an explicit "launch" uses it too.
    if let Some(provider) = provider {
        let p = provider.to_lowercase();
        ensure_allowed_domains_supported_for_launch(AllowedDomainsLaunchSupport {
            allowed_domains: &allowed_domains,
            cdp_url: None,
            cdp_port: None,
            auto_connect: false,
            profile: options.profile.as_deref(),
            provider_name: Some(p.as_str()),
            args: &options.args,
            restore_key: restore_key.as_deref(),
            storage_state,
        })?;
        // ios and safari are WebDriver-backed device providers.
        if p == "ios" {
            // A relaunch keeps the device the session was set up with.
            let mut ios_cmd = if cmd.is_object() {
                cmd.clone()
            } else {
                json!({})
            };
            if ios_cmd.get("deviceName").is_none() {
                if let Some(device) = state.settings.get(Key::Device).and_then(Value::as_str) {
                    ios_cmd["deviceName"] = json!(device);
                }
            }
            let result = launch_ios(&ios_cmd, state).await;
            if result.is_ok() {
                state.effective_ca_cert = effective_ca_cert;
            }
            return result;
        }
        if p == "safari" {
            let result = launch_safari(cmd, state).await;
            if result.is_ok() {
                state.effective_ca_cert = effective_ca_cert;
            }
            return result;
        }
        if !p.is_empty() {
            let conn =
                providers::connect_provider_with_plugins(&p, &plugins, &state.settings).await?;
            let provider_metadata = conn.metadata.clone();
            if conn.direct_page && !allowed_domains.is_empty() {
                if let Some(ref ps) = conn.session {
                    let _ = providers::close_provider_session_with_plugins(ps, &plugins).await;
                }
                return Err(direct_page_allowed_domains_error());
            }
            let ws_headers = if p == "agentcore" {
                providers::take_agentcore_ws_headers()
            } else {
                None
            };
            match attach_provider_cdp(&p, &conn, ws_headers, state, &plugins).await {
                Ok(mgr) => {
                    state.reset_input_state();
                    state.browser = Some(mgr);
                    state.launch_identity = Some(state.settings.launch_identity());
                    state.subscribe_to_browser_events();
                    state.start_fetch_handler();
                    state.start_dialog_handler();
                    state.update_stream_client().await;
                    write_provider_file(&state.session_id, &p);
                    install_network_controls_or_close(state, has_proxy_auth).await?;
                    apply_launch_init_scripts(state, &enable_features, &init_script_paths).await;
                    try_auto_restore_state(state).await;
                    load_storage_state_for_launch(state, &storage_state_path, strict_state).await?;
                    state.effective_ca_cert = effective_ca_cert;
                    let mut extras = json!({ "provider": provider });
                    if let Some(info) = providers::get_agentcore_info() {
                        extras["agentCoreSessionId"] = json!(info.session_id);
                        extras["agentCoreLiveViewUrl"] = json!(info.live_view_url);
                    } else if let Some(metadata) = provider_metadata {
                        extras["providerMetadata"] = metadata;
                    }
                    return Ok(extras);
                }
                Err(e) => {
                    let cleanup_error = close_active_provider_session(state).await.err();
                    return Err(combine_provider_failure(
                        format!("Provider '{}' connection failed: {}", p, e),
                        cleanup_error,
                    ));
                }
            }
        }
    }

    ensure_allowed_domains_supported_for_launch(AllowedDomainsLaunchSupport {
        allowed_domains: &allowed_domains,
        cdp_url: None,
        cdp_port: None,
        auto_connect: false,
        profile: options.profile.as_deref(),
        provider_name: None,
        args: &options.args,
        restore_key: restore_key.as_deref(),
        storage_state,
    })?;

    apply_launch_mutator_plugins(state, &mut options, plugins).await?;
    validate_ca_cert_launch_mode(&options, engine.as_deref(), false)?;
    ensure_allowed_domains_supported_for_launch(AllowedDomainsLaunchSupport {
        allowed_domains: &allowed_domains,
        cdp_url: None,
        cdp_port: None,
        auto_connect: false,
        profile: options.profile.as_deref(),
        provider_name: None,
        args: &options.args,
        restore_key: restore_key.as_deref(),
        storage_state,
    })?;
    write_extensions_file_from_paths(
        &state.session_id,
        &state.settings,
        options.extensions.as_deref(),
    );
    if let Some(ref ca) = effective_ca_cert {
        options.prepared_nss_home = Some(prepare_nss_home(&ca.bundle)?);
    }
    state.session_setup = SessionSetup::from_launch_options(&options);
    let mgr = BrowserManager::launch(options, engine.as_deref()).await?;
    state.reset_input_state();
    state.browser = Some(mgr);
    state.launch_identity = Some(state.settings.launch_identity());
    state.effective_ca_cert = effective_ca_cert;
    state.subscribe_to_browser_events();
    state.start_fetch_handler();
    state.start_dialog_handler();
    crate::timing::mark("launch handlers started");
    state.update_stream_client().await;
    crate::timing::mark("launch stream client");
    install_network_controls_or_close(state, has_proxy_auth).await?;
    crate::timing::mark("launch network controls");

    apply_launch_init_scripts(state, &enable_features, &init_script_paths).await;
    crate::timing::mark("launch init scripts");
    try_auto_restore_state(state).await;
    crate::timing::mark("launch auto restore");
    load_storage_state_for_launch(state, &storage_state_path, strict_state).await?;
    crate::timing::mark("launch storage state");
    Ok(json!({}))
}

pub(super) fn string_array_from_command(cmd: &Value, key: &str) -> Option<Vec<String>> {
    cmd.get(key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    })
}

pub(super) fn allowed_domains_from_launch_command(cmd: &Value) -> Option<Vec<String>> {
    let value = cmd.get("allowedDomains")?;
    let raw_domains: Vec<&str> = match value {
        Value::String(domains) => domains.split(',').collect(),
        Value::Array(domains) => domains.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    Some(
        raw_domains
            .into_iter()
            .map(|domain| domain.trim().to_lowercase())
            .filter(|domain| !domain.is_empty())
            .collect(),
    )
}

pub(super) async fn apply_launch_init_scripts(
    state: &mut DaemonState,
    enable_features: &[String],
    init_script_paths: &[String],
) {
    let mut sources: Vec<String> = Vec::new();

    for feature in enable_features {
        match feature.as_str() {
            "react-devtools" | "react" => {
                sources.push(react::INSTALL_HOOK_JS.to_string());
            }
            other => {
                eprintln!("warning: unknown --enable feature '{}'", other);
            }
        }
    }

    for path in init_script_paths {
        match fs::read_to_string(path) {
            Ok(source) => sources.push(source),
            Err(e) => {
                eprintln!("warning: failed to read --init-script '{}': {}", path, e);
            }
        }
    }

    sources.extend(state.plugin_init_scripts.iter().cloned());

    let Some(mgr) = state.browser.as_ref() else {
        return;
    };
    let Ok(session_id) = mgr.active_session_id().map(str::to_string) else {
        return;
    };

    let mut registered = Vec::with_capacity(sources.len());
    for source in sources {
        let session_identifier = mgr
            .add_script_to_evaluate(&source)
            .await
            .unwrap_or_default();
        registered.push((source, session_identifier));
    }
    for (source, session_identifier) in registered {
        state
            .session_setup
            .register_init_script(source, session_id.clone(), session_identifier);
    }
}

pub(super) async fn apply_launch_mutator_plugins(
    state: &mut DaemonState,
    options: &mut LaunchOptions,
    plugins: Vec<crate::plugins::PluginConfig>,
) -> Result<(), String> {
    state.plugin_init_scripts.clear();
    if plugins.is_empty() {
        return Ok(());
    }

    let request = json!({
        "session": state.session_id,
        "launchOptions": {
            "headless": options.headless,
            "engine": state.settings.engine().unwrap_or_else(|| "chrome".to_string()),
            "args": options.args.clone(),
            "extensions": options.extensions.clone(),
            "userAgent": options.user_agent.clone(),
            "colorScheme": options.color_scheme.clone(),
            "downloadPath": options.download_path.clone(),
            "hideScrollbars": options.hide_scrollbars,
            "allowFileAccess": options.allow_file_access,
            "webgpu": options.webgpu,
            "noXvfb": options.no_xvfb,
        }
    });

    for mutation in crate::plugins::launch_mutations_from_plugins(&plugins, request).await? {
        options.args.extend(mutation.args);
        if !mutation.extensions.is_empty() {
            options
                .extensions
                .get_or_insert_with(Vec::new)
                .extend(mutation.extensions);
        }
        if let Some(user_agent) = mutation.user_agent {
            options.user_agent = Some(user_agent);
        }
        state.plugin_init_scripts.extend(mutation.init_scripts);
    }

    Ok(())
}

pub(super) async fn try_auto_restore_state(state: &mut DaemonState) {
    state.restore_skipped_origins.clear();
    let session_name = match state.session_name.as_deref() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            state.restore_status = "not_configured".to_string();
            state.restore_status_detail = None;
            state.restore_loaded_path = None;
            state.restore_load_failed = false;
            state.restore_validation_pending = false;
            return;
        }
    };
    if let Some(path) = state::find_auto_state_file(&session_name) {
        if let Some(ref mgr) = state.browser {
            if let Ok(session_id) = mgr.active_session_id() {
                match state::load_state(&mgr.client, session_id, &path).await {
                    Ok(skipped) => {
                        state.restore_status = "loaded".to_string();
                        state.restore_status_detail = None;
                        state.restore_skipped_origins = skipped;
                        state.restore_loaded_path = Some(path.clone());
                        state.restore_load_failed = false;
                        state.restore_validation_pending = state.restore_check_url.is_some()
                            || state.restore_check_text.is_some()
                            || state.restore_check_fn.is_some();
                    }
                    Err(err) => {
                        state.restore_status = "load_failed".to_string();
                        state.restore_status_detail = Some(err);
                        state.restore_loaded_path = Some(path);
                        state.restore_load_failed = true;
                        state.restore_validation_pending = false;
                    }
                }
            }
        }
    } else {
        state.restore_status = "missing".to_string();
        state.restore_status_detail = None;
        state.restore_loaded_path = None;
        state.restore_load_failed = false;
        state.restore_validation_pending = false;
    }
}

pub(super) async fn validate_restore_if_pending(state: &mut DaemonState) {
    if !state.restore_validation_pending {
        return;
    }
    state.restore_validation_pending = false;

    match validate_restored_state(state).await {
        Ok(()) => {
            state.restore_status = "loaded".to_string();
            state.restore_status_detail = None;
            state.restore_load_failed = false;
        }
        Err(err) => {
            state.restore_status = "loaded_but_invalid".to_string();
            state.restore_status_detail = Some(err);
            state.restore_load_failed = true;
        }
    }
}

pub(super) async fn validate_restored_state(state: &DaemonState) -> Result<(), String> {
    let Some(ref mgr) = state.browser else {
        return Ok(());
    };
    let session_id = mgr.active_session_id()?.to_string();
    let timeout_ms = state.default_timeout_ms.min(2_000);

    if let Some(ref pattern) = state.restore_check_url {
        let url = mgr.get_url().await.unwrap_or_default();
        if !route_url_matches(pattern, &url) {
            return Err(format!(
                "restore URL validation failed: '{}' did not match '{}'",
                url, pattern
            ));
        }
    }

    if let Some(ref text) = state.restore_check_text {
        wait_for_text(&mgr.client, &session_id, text, timeout_ms).await?;
    }

    if let Some(ref expression) = state.restore_check_fn {
        wait_for_function(&mgr.client, &session_id, expression, timeout_ms).await?;
    }

    Ok(())
}

pub(super) fn mark_explicit_storage_state_loaded(state: &mut DaemonState, path: &str) {
    if state.session_name.is_none() && state.restore_status == "not_configured" {
        return;
    }

    state.restore_status = "loaded".to_string();
    state.restore_status_detail = None;
    state.restore_loaded_path = Some(path.to_string());
    state.restore_load_failed = false;
    state.restore_validation_pending = false;
    state.restore_save_status = "not_attempted".to_string();
    state.restore_saved_path = None;
}

/// Quiet time required after the last command before a periodic autosave may
/// run, so a multi-second save never stalls an active command burst.
pub(super) const AUTOSAVE_QUIET_PERIOD_MS: u64 = 2_000;

/// Whether the daemon's periodic tick should attempt an autosave right now.
///
/// Timing and dialog pre-checks only; restore-key, save-policy, and browser
/// gating live in `maybe_autosave_restore_state` and `auto_save_restore_state`.
pub(super) fn autosave_due(state: &DaemonState, interval_ms: u64) -> bool {
    if interval_ms == 0 {
        return false;
    }
    // A JS dialog blocks the renderer's main thread, so the storage-collection
    // evaluate would hang until its CDP timeout. Wait for the dialog instead.
    if state.pending_dialog.is_some() {
        return false;
    }
    let now = std::time::Instant::now();
    if let Some(t) = state.last_command_finished {
        if now.duration_since(t) < std::time::Duration::from_millis(AUTOSAVE_QUIET_PERIOD_MS) {
            return false;
        }
    }
    if let Some(t) = state.last_autosave_attempt {
        if now.duration_since(t) < std::time::Duration::from_millis(interval_ms) {
            return false;
        }
    }
    true
}

/// Periodically persist session state while a browser is open with a restore
/// key configured, so a browser the user closes by hand (which kills CDP and
/// makes save-on-close impossible) loses at most roughly `interval_ms` of
/// state. Saves stay eligible even without new commands: the page itself can
/// mutate cookies and storage while idle (token refreshes, background
/// requests), so idle sessions are re-saved on every interval too.
///
/// Called from the daemon's background tick. Failures are expected while a
/// page is mid-navigation; the next interval retries, and
/// `auto_save_restore_state` records the status either way.
pub(crate) async fn maybe_autosave_restore_state(state: &mut DaemonState, interval_ms: u64) {
    if !autosave_due(state, interval_ms) {
        return;
    }
    // Sessions without a restore key, or with saving disabled, never autosave.
    if state.session_name.is_none() || state.restore_save == "never" {
        return;
    }
    // No browser means nothing to collect from.
    if state.browser.is_none() {
        return;
    }
    state.last_autosave_attempt = Some(std::time::Instant::now());
    let _ = auto_save_restore_state(state).await;
}

pub(crate) async fn auto_save_restore_state(
    state: &mut DaemonState,
) -> Result<Option<String>, String> {
    validate_restore_if_pending(state).await;

    let Some(session_name) = state.session_name.clone() else {
        state.restore_save_status = "not_configured".to_string();
        state.restore_saved_path = None;
        return Ok(None);
    };

    match state.restore_save.as_str() {
        "never" => {
            state.restore_save_status = "disabled".to_string();
            state.restore_saved_path = None;
            return Ok(None);
        }
        "auto" if state.restore_load_failed => {
            state.restore_save_status = "skipped_restore_failed".to_string();
            state.restore_saved_path = None;
            return Ok(None);
        }
        "auto" | "always" => {}
        other => {
            state.restore_save_status = "invalid_policy".to_string();
            state.restore_saved_path = None;
            return Err(format!(
                "Invalid restore save policy '{}'. Use auto, always, or never.",
                other
            ));
        }
    }

    let Some(ref mgr) = state.browser else {
        state.restore_save_status = "no_browser".to_string();
        state.restore_saved_path = None;
        return Ok(None);
    };
    let active_session_id = mgr.active_session_id()?.to_string();

    match state::save_auto_state_transactional(
        &mgr.client,
        &active_session_id,
        &session_name,
        &state.session_id,
        mgr.visited_origins(),
    )
    .await
    {
        Ok(path) => {
            state.restore_save_status = "saved".to_string();
            state.restore_saved_path = Some(path.clone());
            // Saves from any path (close, relaunch, restore-key change) reset
            // the periodic interval so the tick doesn't immediately re-save.
            state.last_autosave_attempt = Some(std::time::Instant::now());
            Ok(Some(path))
        }
        Err(err) => {
            state.restore_save_status = "error".to_string();
            state.restore_saved_path = None;
            Err(err)
        }
    }
}

/// Load storage state if a path is configured.
///
/// Explicit launch should surface this error. Best-effort callers can ignore
/// the returned `Result` and keep their previous behavior.
pub(super) async fn load_storage_state(
    state: &mut DaemonState,
    path: &Option<String>,
) -> Result<(), String> {
    if let Some(ref path) = path {
        ensure_state_replay_supported_by_active_domain_filter(state, "--state/storageState")
            .await?;
        let mut skipped = None;
        if let Some(ref mgr) = state.browser {
            if let Ok(session_id) = mgr.active_session_id() {
                skipped = Some(state::load_state(&mgr.client, session_id, path).await?);
            }
        }
        if let Some(skipped) = skipped {
            mark_explicit_storage_state_loaded(state, path);
            state.restore_skipped_origins.extend(skipped);
        }
    }

    Ok(())
}

pub(super) async fn rollback_failed_launch(state: &mut DaemonState) -> Result<(), String> {
    let close_result = close_current_browser(state).await;
    state.ref_map.invalidate_all_documents();
    close_result
}

pub(super) async fn load_storage_state_or_rollback(
    state: &mut DaemonState,
    path: &Option<String>,
) -> Result<(), String> {
    if let Err(err) = load_storage_state(state, path).await {
        if let Err(close_err) = rollback_failed_launch(state).await {
            return Err(format!(
                "{} (also failed to roll back browser after launch: {})",
                err, close_err
            ));
        }
        return Err(err);
    }

    Ok(())
}

/// Loads the storage state a launch was set up with. A command that named the
/// state file fails the launch when it cannot be loaded; a launch that only
/// inherits it continues without the state.
pub(super) async fn load_storage_state_for_launch(
    state: &mut DaemonState,
    path: &Option<String>,
    strict: bool,
) -> Result<(), String> {
    if strict {
        load_storage_state_or_rollback(state, path).await
    } else {
        let _ = load_storage_state(state, path).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 1 handlers
// ---------------------------------------------------------------------------

/// Optional WebSocket connect headers for generic CDP URLs (`cdpUrl` / `--cdp`).
/// Headers from the last explicit CDP launch, when it named this endpoint.
pub(super) fn cdp_headers_for(
    state: &DaemonState,
    endpoint: &str,
) -> Option<Vec<(String, String)>> {
    state
        .cdp_headers
        .as_ref()
        .filter(|(saved, _)| saved == endpoint)
        .map(|(_, headers)| headers.clone())
}

/// Headers the session was configured with alongside its CDP target.
pub(super) fn cdp_headers_from_settings(settings: &Running) -> Option<Vec<(String, String)>> {
    let raw = settings.cdp_headers()?;
    let headers: Value = serde_json::from_str(&raw).ok()?;
    cdp_headers_from_command(&json!({ "cdpHeaders": headers }))
}

/// A relaunch rewritten to the attached CDP target keeps that target's headers.
pub(super) fn attach_saved_cdp_headers(launch: &mut Value, state: &DaemonState) {
    let endpoint = match (launch.get("cdpUrl"), launch.get("cdpPort")) {
        (Some(Value::String(url)), _) => url.clone(),
        (_, Some(port)) => port.to_string(),
        _ => return,
    };
    if let Some(headers) = cdp_headers_for(state, &endpoint) {
        let map: serde_json::Map<String, Value> = headers
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect();
        launch["cdpHeaders"] = Value::Object(map);
    }
}

/// Distinct from page/origin `--headers` (Fetch interception).
pub(super) fn cdp_headers_from_command(cmd: &Value) -> Option<Vec<(String, String)>> {
    let map = cmd.get("cdpHeaders")?.as_object()?;
    if map.is_empty() {
        return None;
    }
    let headers: Vec<(String, String)> = map
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    if headers.is_empty() {
        None
    } else {
        Some(headers)
    }
}

pub(super) async fn handle_launch(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    handle_launch_with_deadline(cmd, state, BROWSER_USE_SETUP_DEADLINE).await
}

pub(super) async fn handle_launch_with_deadline(
    cmd: &Value,
    state: &mut DaemonState,
    deadline: std::time::Duration,
) -> Result<Value, String> {
    let mut resolved = resolve_launch_target(cmd, state.attached_target.as_ref());
    if let Some(launch) = resolved.as_mut() {
        attach_saved_cdp_headers(launch, state);
    }
    let cmd = resolved.as_ref().unwrap_or(cmd);
    let result = handle_launch_for_target(cmd, state, deadline).await;
    if result.is_ok() {
        state.attached_target = AttachedTarget::from_launch_command(cmd);
        Box::pin(start_flow_capture(state)).await?;
    }
    result
}

/// Starts flow capture on a freshly launched browser when the session's
/// `capture` setting asks for it. A session that already captures keeps its
/// steps.
pub(super) async fn start_flow_capture(state: &mut DaemonState) -> Result<(), String> {
    if !state.settings.captures_flow() || state.flow.active() {
        return Ok(());
    }
    Box::pin(handle_har_start(&json!({}), state)).await?;
    let page_url = match state.browser.as_ref() {
        Some(mgr) => mgr.get_url().await.ok(),
        None => None,
    };
    state.flow.begin_capture(page_url);
    Ok(())
}

/// Rejects capture modes the daemon cannot record, before they are stored.
pub(super) fn check_capture_setting(patch: &Patch) -> Result<(), String> {
    match patch.value(Key::Capture) {
        Some(Value::Array(modes)) => {
            let modes: Vec<String> = modes
                .iter()
                .filter_map(|m| m.as_str().map(String::from))
                .collect();
            crate::native::flow::parse_capture(&modes)
        }
        _ => Ok(()),
    }
}

pub(super) async fn handle_launch_for_target(
    cmd: &Value,
    state: &mut DaemonState,
    deadline: std::time::Duration,
) -> Result<Value, String> {
    if cmd.get("cdpUrl").and_then(Value::as_str).is_some()
        || cmd.get("cdpPort").and_then(Value::as_u64).is_some()
        || cmd.get("autoConnect").and_then(Value::as_bool) == Some(true)
        || !cmd
            .get("provider")
            .and_then(Value::as_str)
            .is_some_and(provider_is_browser_use)
    {
        return handle_launch_inner(cmd, state).await;
    }
    let filter = state.domain_filter.read().await.clone();
    let credentials = state.proxy_credentials.read().await.clone();
    let scripts = state.plugin_init_scripts.clone();
    let setup = state.session_setup.clone();
    let result = match tokio::time::timeout(deadline, handle_launch_inner(cmd, state)).await {
        Ok(result) => result,
        Err(_) => Err(browser_use_setup_timeout(state).await),
    };
    if result.is_err() {
        restore_domain_filter(state, &filter).await;
        *state.proxy_credentials.write().await = credentials;
        state.plugin_init_scripts = scripts;
        state.session_setup = setup;
    }
    result
}

pub(super) async fn handle_launch_inner(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let engine = cmd
        .get("engine")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| state.settings.engine());
    let patch = launch_envelope_patch(cmd, engine.as_deref());
    check_capture_setting(&patch)?;
    let had_browser_before_launch = has_active_browser_session(state);

    let mut outcome = apply_settings_patch(state, &patch, true).await?;
    let reusable = !outcome.relaunched_browser
        && match (state.browser.as_mut(), state.webdriver_backend.as_ref()) {
            (Some(mgr), _) => !mgr.has_process_exited() && mgr.is_connection_alive().await,
            (None, Some(webdriver)) => webdriver.is_connection_alive().await,
            (None, None) => false,
        };
    if reusable {
        return Ok(json!({ "launched": true, "reused": true, "relaunchedBrowser": false }));
    }

    // The command's own settings can name the file too (bare `open`).
    let strict_state = patch.value(Key::StorageState).is_some()
        || Patch::from_wire(cmd.get("settings"))
            .is_ok_and(|settings| settings.value(Key::StorageState).is_some());
    match auto_launch_with(state, plugins_from_command_or_env(cmd), cmd, strict_state).await {
        Ok(extras) => {
            let mut result = json!({
                "launched": true,
                "relaunchedBrowser": had_browser_before_launch,
            });
            if let (Some(result), Some(extras)) = (result.as_object_mut(), extras.as_object()) {
                result.extend(extras.clone());
            }
            Ok(result)
        }
        Err(e) => {
            let rollback = rollback_launch_settings(state, &mut outcome, &e).await;
            Err(with_rollback_error(&e, rollback))
        }
    }
}

pub(super) async fn launch_ios(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let device_name = cmd.get("deviceName").and_then(|v| v.as_str());
    let device_udid = cmd.get("udid").and_then(|v| v.as_str());
    let platform_version = cmd.get("platformVersion").and_then(|v| v.as_str());

    // Select device (or use default)
    let device = ios::select_device(device_name, device_udid)?;

    // Boot simulator if it's not real and not already booted
    if !device.is_real && device.state != "Booted" {
        ios::boot_simulator(&device.udid)?;
    }

    // Start Appium
    let mut appium = AppiumManager::connect_or_launch(Some(&device.udid)).await?;

    // Create iOS Safari session
    appium
        .create_ios_session(Some(&device.name), platform_version)
        .await?;

    // Create a WebDriverBackend from the Appium session for common commands
    if let Some(sid) = appium.client.session_id_pub().map(String::from) {
        let wd_client =
            crate::native::webdriver::client::WebDriverClient::new_with_session(4723, sid);
        state.webdriver_backend = Some(WebDriverBackend::new(wd_client));
    }

    state.appium = Some(appium);
    state.backend_type = BackendType::WebDriver;
    state.engine = "safari".to_string();
    write_engine_file(&state.session_id, &state.engine);
    write_provider_file(&state.session_id, "ios");
    write_extensions_file(&state.session_id, &state.settings);
    state.reset_input_state();

    Ok(json!({
        "launched": true,
        "provider": "ios",
        "device": device.name,
        "udid": device.udid,
        "backend": "webdriver",
    }))
}

pub(super) async fn launch_safari(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let port: u16 = cmd
        .get("port")
        .and_then(|v| v.as_u64())
        .map(|p| p as u16)
        .unwrap_or(0);
    let driver_port = if port > 0 { port } else { 0 };

    // Find a free port if none specified
    let actual_port = if driver_port > 0 {
        driver_port
    } else {
        // Use any available high port
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("Failed to find free port: {}", e))?;
        listener
            .local_addr()
            .map_err(|e| format!("Failed to get local address: {}", e))?
            .port()
    };

    let driver = safari::launch_safaridriver(actual_port)?;
    let mut client = crate::native::webdriver::client::WebDriverClient::new(actual_port);

    client
        .create_session(serde_json::json!({
            "browserName": "safari",
        }))
        .await?;

    state.safari_driver = Some(driver);
    state.webdriver_backend = Some(WebDriverBackend::new(client));
    state.backend_type = BackendType::WebDriver;
    state.engine = "safari".to_string();
    write_engine_file(&state.session_id, &state.engine);
    write_provider_file(&state.session_id, "safari");
    write_extensions_file(&state.session_id, &state.settings);
    state.reset_input_state();

    Ok(json!({
        "launched": true,
        "provider": "safari",
        "port": actual_port,
        "backend": "webdriver",
    }))
}
