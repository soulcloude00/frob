use super::*;

pub(super) fn append_launch_mutator_policy_actions_for(
    actions: &mut Vec<String>,
    plugins: &[crate::plugins::PluginConfig],
) {
    for plugin in crate::plugins::resolved_plugins_with_capability(
        plugins,
        crate::plugins::CAPABILITY_LAUNCH_MUTATE,
    ) {
        actions.push(crate::plugins::plugin_policy_action(
            &plugin.name,
            crate::plugins::CAPABILITY_LAUNCH_MUTATE,
        ));
    }
}

pub(super) fn append_browser_provider_policy_action_for(
    actions: &mut Vec<String>,
    provider: &str,
    plugins: &[crate::plugins::PluginConfig],
) {
    if crate::plugins::find_plugin(plugins, provider).is_some_and(|plugin| {
        crate::plugins::plugin_has_capability(plugin, crate::plugins::CAPABILITY_BROWSER_PROVIDER)
    }) {
        actions.push(crate::plugins::plugin_policy_action(
            provider,
            crate::plugins::CAPABILITY_BROWSER_PROVIDER,
        ));
    }
}

pub(super) fn append_credential_policy_action_for(
    actions: &mut Vec<String>,
    provider: &str,
    plugins: &[crate::plugins::PluginConfig],
) {
    if crate::plugins::find_plugin(plugins, provider).is_some_and(|plugin| {
        crate::plugins::plugin_has_capability(plugin, crate::plugins::CAPABILITY_CREDENTIAL_READ)
    }) {
        actions.push(crate::plugins::plugin_policy_action(
            provider,
            crate::plugins::CAPABILITY_CREDENTIAL_READ,
        ));
    }
}

pub(super) fn plugins_from_command_or_env(cmd: &Value) -> Vec<crate::plugins::PluginConfig> {
    cmd.get("plugins")
        .and_then(|v| serde_json::from_value::<Vec<crate::plugins::PluginConfig>>(v.clone()).ok())
        .unwrap_or_else(crate::plugins::plugins_from_env)
}

pub(super) fn reset_restore_runtime_state(state: &mut DaemonState) {
    state.restore_status = "pending".to_string();
    state.restore_status_detail = None;
    state.restore_skipped_origins.clear();
    state.restore_loaded_path = None;
    state.restore_load_failed = false;
    state.restore_validation_pending = false;
    state.restore_save_status = "not_attempted".to_string();
    state.restore_saved_path = None;
}

pub(super) fn command_restore_check_fields(
    cmd: &Value,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let has_check_field = cmd.get("restoreCheckUrl").is_some()
        || cmd.get("restoreCheckText").is_some()
        || cmd.get("restoreCheckFn").is_some();
    if !has_check_field {
        return None;
    }

    Some((
        cmd.get("restoreCheckUrl")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        cmd.get("restoreCheckText")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        cmd.get("restoreCheckFn")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
    ))
}

pub(super) fn restore_checks_are_configured(
    checks: &(Option<String>, Option<String>, Option<String>),
) -> bool {
    checks.0.is_some() || checks.1.is_some() || checks.2.is_some()
}

pub(super) fn reconcile_restore_check_change(
    state: &mut DaemonState,
    checks: &(Option<String>, Option<String>, Option<String>),
) {
    if restore_checks_are_configured(checks) {
        if matches!(
            state.restore_status.as_str(),
            "loaded" | "loaded_but_invalid"
        ) {
            state.restore_status = "loaded".to_string();
            state.restore_status_detail = None;
            state.restore_load_failed = false;
            state.restore_validation_pending = true;
        }
    } else {
        state.restore_validation_pending = false;
        if state.restore_status == "loaded_but_invalid" {
            state.restore_status = "loaded".to_string();
            state.restore_status_detail = None;
            state.restore_load_failed = false;
        }
    }
}

pub(super) fn apply_restore_config_from_command(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<(), String> {
    validate_restore_config_from_command(cmd)?;

    let restore_key = cmd.get("restoreKey").and_then(|v| v.as_str());
    let old_checks = (
        state.restore_check_url.clone(),
        state.restore_check_text.clone(),
        state.restore_check_fn.clone(),
    );

    if let Some(restore_key) = restore_key {
        if !restore_key.is_empty() {
            if state.session_name.as_deref() != Some(restore_key) {
                reset_restore_runtime_state(state);
            }
            state.session_name = Some(restore_key.to_string());
            if state.restore_status == "not_configured" {
                state.restore_status = "pending".to_string();
            }
        }
    }
    if let Some(policy) = cmd.get("restoreSave").map(|v| v.as_str().unwrap_or("auto")) {
        state.restore_save = policy.to_string();
    }
    if let Some(new_checks) = command_restore_check_fields(cmd) {
        state.restore_check_url = new_checks.0.clone();
        state.restore_check_text = new_checks.1.clone();
        state.restore_check_fn = new_checks.2.clone();
        if old_checks != new_checks {
            reconcile_restore_check_change(state, &new_checks);
        }
    }

    Ok(())
}

pub(super) fn validate_restore_config_from_command(cmd: &Value) -> Result<(), String> {
    let restore_key = cmd.get("restoreKey").and_then(|v| v.as_str());
    if let Some(restore_key) = restore_key {
        if !restore_key.is_empty() && !is_valid_session_name(restore_key) {
            return Err(session_name_error(restore_key));
        }
    }

    let restore_save = cmd.get("restoreSave").map(|v| v.as_str().unwrap_or("auto"));
    if let Some(policy) = restore_save {
        if !matches!(policy, "auto" | "always" | "never") {
            return Err(format!(
                "Invalid restore save policy '{}'. Use auto, always, or never.",
                policy
            ));
        }
    }

    Ok(())
}

pub(super) fn command_changes_restore_key(cmd: &Value, state: &DaemonState) -> bool {
    cmd.get("restoreKey")
        .and_then(|v| v.as_str())
        .filter(|key| !key.is_empty())
        .is_some_and(|key| state.session_name.as_deref() != Some(key))
}

/// Whether the session holds a browser of any backend: CDP, a provider
/// session, or a WebDriver session (Safari, iOS).
pub(super) fn has_active_browser_session(state: &DaemonState) -> bool {
    state.browser.is_some()
        || state.active_provider_session.is_some()
        || state.webdriver_backend.is_some()
        || state.appium.is_some()
        || state.safari_driver.is_some()
}

pub(super) async fn close_before_implicit_relaunch(state: &mut DaemonState) -> Result<(), String> {
    let browser_use = state
        .active_provider_session
        .as_ref()
        .is_some_and(|active| provider_is_browser_use(&active.session.provider));
    let result = close_all_browser_backends(state).await;
    if browser_use {
        result
    } else {
        Ok(())
    }
}

pub(super) async fn apply_restore_config_after_confirmation(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<bool, String> {
    let restore_key_changed = command_changes_restore_key(cmd, state);
    let had_browser = has_active_browser_session(state);

    if restore_key_changed && had_browser {
        let _ = auto_save_restore_state(state).await;
        close_before_implicit_relaunch(state).await?;
    }

    apply_restore_config_from_command(cmd, state)?;
    Ok(restore_key_changed && had_browser)
}

pub(super) const BROWSER_USE_ATTACH_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(8);

pub(super) fn provider_is_browser_use(provider: &str) -> bool {
    matches!(
        provider.to_lowercase().as_str(),
        "browser-use" | "browseruse"
    )
}

pub(super) async fn attach_provider_cdp(
    provider: &str,
    conn: &providers::ProviderConnection,
    ws_headers: Option<Vec<(String, String)>>,
    state: &mut DaemonState,
    plugins: &[crate::plugins::PluginConfig],
) -> Result<BrowserManager, String> {
    remember_active_provider_session(state, conn.session.clone(), plugins);
    if provider_is_browser_use(provider)
        && !url::Url::parse(&conn.ws_url).is_ok_and(|url| {
            matches!(url.scheme(), "ws" | "wss" | "http" | "https") && url.host_str().is_some()
        })
    {
        return Err("Browser Use did not return a usable cdpUrl".to_string());
    }
    let attach = async {
        if conn.direct_page {
            BrowserManager::connect_cdp_direct(&conn.ws_url).await
        } else if ws_headers.is_some() {
            BrowserManager::connect_cdp_with_headers(&conn.ws_url, ws_headers).await
        } else {
            BrowserManager::connect_cdp(&conn.ws_url).await
        }
    };
    if provider_is_browser_use(provider) {
        match tokio::time::timeout(BROWSER_USE_ATTACH_DEADLINE, attach).await {
            Ok(result) => result.map_err(|_| "Browser Use CDP attachment failed".to_string()),
            Err(_) => Err(format!(
                "Browser Use CDP attachment timed out after {}s",
                BROWSER_USE_ATTACH_DEADLINE.as_secs()
            )),
        }
    } else {
        attach.await
    }
}

pub(super) fn combine_provider_failure(context: String, cleanup_error: Option<String>) -> String {
    match cleanup_error {
        Some(cleanup_error) => format!(
            "{} (provider cleanup also failed: {})",
            context, cleanup_error
        ),
        None => context,
    }
}

pub(super) fn remember_active_provider_session(
    state: &mut DaemonState,
    session: Option<providers::ProviderSession>,
    plugins: &[crate::plugins::PluginConfig],
) {
    state.active_provider_connection = true;
    state.active_provider_session = session.map(|session| ActiveProviderSession {
        session,
        plugins: plugins.to_vec(),
    });
}

pub(super) async fn close_active_provider_session(state: &mut DaemonState) -> Result<(), String> {
    if let Some(active) = state.active_provider_session.as_ref() {
        providers::close_provider_session_with_plugins(&active.session, &active.plugins).await?;
    }
    state.active_provider_session = None;
    state.active_provider_connection = false;
    Ok(())
}

pub(crate) async fn close_current_browser(state: &mut DaemonState) -> Result<(), String> {
    let browser = state.browser.take();
    state.launch_identity = None;
    state.webmcp_enabled = false;
    state.network_auto_attach_installed = false;
    state.iframe_sessions.clear();
    state.active_iframe_sessions.clear();
    state.webmcp.clear_all();
    state.screencasting = false;
    state.reset_input_state();
    state.update_stream_client().await;

    let close_error = if let Some(mut mgr) = browser {
        mgr.close().await.err()
    } else {
        None
    };

    let provider_close_error = close_active_provider_session(state).await.err();
    let close_error = match (close_error, provider_close_error) {
        (Some(browser_error), Some(provider_error)) => Some(format!(
            "{} (also failed to release provider browser: {})",
            browser_error, provider_error
        )),
        (error, None) | (None, error) => error,
    };
    if let Some(err) = close_error {
        return Err(err);
    }
    Ok(())
}

/// Close every browser backend owned by the daemon.
///
/// Lifecycle shutdown paths use this instead of `close_current_browser`
/// because Safari and iOS sessions live outside `state.browser`.
pub(crate) async fn close_all_browser_backends(state: &mut DaemonState) -> Result<(), String> {
    let close_result = close_current_browser(state).await;

    if let Some(ref mut webdriver) = state.webdriver_backend {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), webdriver.close()).await;
    }
    state.webdriver_backend = None;
    if let Some(ref mut appium) = state.appium {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), appium.close()).await;
    }
    state.appium = None;
    if let Some(ref mut driver) = state.safari_driver {
        driver.kill();
    }
    state.safari_driver = None;
    state.backend_type = BackendType::Cdp;

    close_result
}

pub(super) async fn close_after_network_control_failure(
    state: &mut DaemonState,
    error: String,
) -> Result<(), String> {
    let close_error = close_current_browser(state).await.err();
    Err(match close_error {
        Some(close_error) => format!(
            "Failed to install browser network controls: {} (also failed to close browser: {})",
            error, close_error
        ),
        None => format!("Failed to install browser network controls: {}", error),
    })
}

/// Returns whether an action can run without implicitly launching a browser.
/// `auth login --no-navigate` must never launch a browser or manufacture an
/// about:blank page to satisfy the command.
pub(super) fn is_auth_login_no_navigate(cmd: &Value) -> bool {
    cmd.get("action").and_then(Value::as_str) == Some("auth_login")
        && cmd
            .get("noNavigate")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

/// Whether `cmd` may launch or relaunch the session's browser. Commands that
/// cannot apply only the live settings, so launch settings and a forced
/// relaunch wait for the next command that can.
pub(crate) fn command_launches_browser(cmd: &Value) -> bool {
    let action = cmd.get("action").and_then(Value::as_str).unwrap_or("");
    (!skip_launch_action(action) || action == "launch") && !is_auth_login_no_navigate(cmd)
}

pub(super) fn skip_launch_action(action: &str) -> bool {
    if action == INTERNAL_DAEMON_SHUTDOWN_ACTION {
        return true;
    }

    matches!(
        action,
        "" | "launch"
            | "close"
            | "read"
            | "har_stop"
            | "flow_save"
            | "flow_steps"
            | "flow_run"
            | "flow_list"
            | "credentials_set"
            | "credentials_get"
            | "credentials_delete"
            | "credentials_list"
            | "auth_save"
            | "auth_show"
            | "auth_delete"
            | "auth_list"
            | "confirm"
            | "deny"
            | "state_list"
            | "state_show"
            | "state_clear"
            | "state_clean"
            | "state_rename"
            | "device_list"
            | "stream_enable"
            | "stream_disable"
            | "stream_status"
            | "session_info"
            | "cdp_url"
            | "webmcp_result"
            | "webmcp_cancel"
    )
}

pub(super) fn should_validate_restore_after_action(action: &str) -> bool {
    action != "launch"
}

/// A flow run that stopped on a site condition reports it as the failure's data.
#[inline(never)]
pub(super) fn attach_flow_stop(resp: &mut Value, state: &mut DaemonState) {
    if let Some(stop) = state.flow_stop.take() {
        if resp.get("success") != Some(&json!(true)) {
            resp["code"] = stop["status"].clone();
            resp["data"] = stop;
        }
    }
}

/// Parks a `flow_run` whose writes the policy does not cover, for confirmation.
#[inline(never)]
pub(super) fn park_unconfirmed_flow_write(
    id: &str,
    cmd: &Value,
    action: &str,
    run_flow: Option<&crate::native::flow::Flow>,
    state: &mut DaemonState,
) -> Option<Value> {
    if action != "flow_run" {
        return None;
    }
    let flow = run_flow?;
    let policy_action = unconfirmed_flow_write(cmd, flow, state)?;
    state.pending_confirmation = Some(PendingConfirmation {
        action: policy_action.clone(),
        cmd: cmd.clone(),
        approved_actions: state.confirmed_policy_actions.iter().cloned().collect(),
        flow: Some(Box::new(flow.clone())),
    });
    Some(json!({
        "id": id,
        "success": true,
        "data": {
            "confirmation_required": true,
            "confirmation_id": id,
            "action": policy_action,
        },
    }))
}

/// Boxed here, inside the callee, so the large flow futures never sit in the
/// stack frame of `execute_command`.
pub(super) fn handle_flow_command<'a>(
    action: &'a str,
    cmd: &'a Value,
    state: &'a mut DaemonState,
    run_flow: Option<Box<crate::native::flow::Flow>>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>> {
    Box::pin(async move { run_flow_command(action, cmd, state, run_flow).await })
}

pub(super) async fn run_flow_command(
    action: &str,
    cmd: &Value,
    state: &mut DaemonState,
    run_flow: Option<Box<crate::native::flow::Flow>>,
) -> Result<Value, String> {
    match action {
        "flow_steps" => handle_flow_steps(cmd, state),
        "flow_save" => handle_flow_save(cmd, state).await,
        "flow_run" => match run_flow {
            Some(flow) => handle_flow_run(cmd, state, *flow).await,
            None => Err("flow run requires a name".to_string()),
        },
        _ => handle_flow_list(cmd),
    }
}

pub(super) fn begin_flow_step<'a>(
    action: &'a str,
    cmd: &'a Value,
    state: &'a mut DaemonState,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(record_flow_step(action, cmd, state))
}

pub(super) async fn record_flow_step(action: &str, cmd: &Value, state: &mut DaemonState) {
    if state.flow.wants_context(action) {
        let from_url = match state.browser.as_ref() {
            Some(mgr) => mgr.get_url().await.ok(),
            None => None,
        };
        let traits = flow_field_traits(action, cmd, state).await;
        state
            .flow
            .begin(action, cmd, &state.ref_map, from_url, traits.as_ref());
    } else {
        state.flow.begin(action, cmd, &state.ref_map, None, None);
    }
}

/// The flow a `flow_run` command runs: the one a confirmation parked, or the
/// named file.
#[inline(never)]
pub(super) fn load_run_flow(
    cmd: &Value,
    action: &str,
    state: &mut DaemonState,
) -> Result<Option<Box<crate::native::flow::Flow>>, String> {
    if action != "flow_run" {
        return Ok(None);
    }
    if let Some(flow) = state.preloaded_flow.take() {
        return Ok(Some(flow));
    }
    let Some(name) = cmd.get("name").and_then(|v| v.as_str()) else {
        return Err("flow run requires a name".to_string());
    };
    crate::native::flow::load_flow(name, cmd.get("origin").and_then(|v| v.as_str()))
        .map(|flow| Some(Box::new(flow)))
}

pub(super) fn policy_actions_for_command(
    cmd: &Value,
    action: &str,
    needs_implicit_launch: bool,
    attached: Option<&AttachedTarget>,
    flow: Option<&crate::native::flow::Flow>,
    settings: &Running,
) -> Vec<String> {
    let mut actions = vec![action.to_string()];
    // `a11y <url>` performs a real browser navigation before the audit. Keep
    // navigation deny and confirmation policies effective for the compound
    // command instead of treating it as a read-only audit.
    if action == "a11y" && cmd.get("url").and_then(|v| v.as_str()).is_some() {
        actions.push("navigate".to_string());
    }
    if let Some(flow) = flow.filter(|_| action == "flow_run") {
        actions.extend(flow.run_writes().iter().map(|w| w.action()));
    }
    if action == "auth_login" {
        if let Some(provider) = cmd.get("credentialProvider").and_then(|v| v.as_str()) {
            let plugins = plugins_from_command_or_env(cmd);
            append_credential_policy_action_for(&mut actions, provider, &plugins);
        }
    }
    if action == "launch" {
        let plugins = plugins_from_command_or_env(cmd);
        // A target the envelope names wins, then the external target the
        // settings name, then the browser the session is attached to.
        let mut launch = cmd.clone();
        if AttachedTarget::named_by(cmd).is_none() {
            if let Some((field, value)) = settings_launch_target(settings) {
                if let Some(obj) = launch.as_object_mut() {
                    obj.insert(field.to_string(), value);
                }
            } else if let Some(resolved) = resolve_launch_target(cmd, attached) {
                launch = resolved;
            }
        }
        append_launch_policy_actions(&mut actions, &launch, &plugins);
    } else if !skip_launch_action(action) && needs_implicit_launch {
        let plugins = plugins_from_command_or_env(cmd);
        if let Some((field, value)) = settings_launch_target(settings) {
            append_launch_policy_actions(&mut actions, &json!({ field: value }), &plugins);
            return actions;
        }
        if let Some(launch) = resolve_launch_target(&json!({ "action": "launch" }), attached) {
            append_launch_policy_actions(&mut actions, &launch, &plugins);
            return actions;
        }
        let provider_launch = settings
            .provider()
            .map(|provider| provider.to_lowercase())
            .filter(|provider| !provider.is_empty() && provider != "ios" && provider != "safari");
        if let Some(provider) = provider_launch {
            append_browser_provider_policy_action_for(&mut actions, &provider, &plugins);
        } else {
            append_launch_mutator_policy_actions_for(&mut actions, &plugins);
        }
    }
    actions
}

/// The external browser the session's settings connect to, as the launch field
/// that names it. Local sessions and the WebDriver providers name none.
pub(super) fn settings_launch_target(settings: &Running) -> Option<(&'static str, Value)> {
    match settings.connection() {
        Connection::Provider(provider)
            if !matches!(provider.to_lowercase().as_str(), "" | "ios" | "safari") =>
        {
            Some(("provider", json!(provider.to_lowercase())))
        }
        Connection::Cdp(cdp) => Some(("cdpUrl", json!(cdp))),
        Connection::AutoConnect => Some(("autoConnect", json!(true))),
        _ => None,
    }
}

pub(super) fn append_launch_policy_actions(
    actions: &mut Vec<String>,
    launch: &Value,
    plugins: &[crate::plugins::PluginConfig],
) {
    // The same rule execution uses for what a launch connects to.
    match AttachedTarget::named_by(launch) {
        Some(AttachedTarget::Provider(provider)) => {
            append_browser_provider_policy_action_for(actions, &provider, plugins);
        }
        Some(_) => {}
        None => append_launch_mutator_policy_actions_for(actions, plugins),
    }
}

/// The command future is large; boxing it keeps it off the caller's stack,
/// which a debug-build test thread would otherwise overflow.
pub async fn execute_command(cmd: &Value, state: &mut DaemonState) -> Value {
    let action = cmd.get("action").and_then(Value::as_str).unwrap_or("");
    crate::timing::mark(&format!("daemon {action} start"));
    let resp = Box::pin(execute_command_flow(cmd, state)).await;
    crate::timing::mark(&format!("daemon {action} end"));
    resp
}

pub(super) async fn execute_command_flow(cmd: &Value, state: &mut DaemonState) -> Value {
    let outer = state.flow.enter();
    let mut resp = execute_command_inner(cmd, state).await;
    if let Some(seq) = state.flow.exit(outer) {
        let ok = resp.get("success") == Some(&json!(true));
        let url = match state.browser.as_ref() {
            Some(mgr) => mgr.get_url().await.ok(),
            None => None,
        };
        let (wants_value, wants_traits) = state.flow.after_reads(seq);
        let value = if wants_value {
            flow_active_field_value(state).await
        } else {
            None
        };
        let traits = if wants_traits {
            flow_active_field_traits(state).await
        } else {
            None
        };
        state.flow.end(seq, ok, url, value, traits);
        resp["recorded"] = json!({ "seq": seq });
    }
    let action = cmd.get("action").and_then(|v| v.as_str()).unwrap_or("");
    if !skip_launch_action(action) {
        warn_if_browser_outdated(state, &mut resp).await;
    }
    resp
}

/// Adds a warning to the response when the connected browser is outdated with known CVEs. Checks each browser product once and never fails.
///
/// The check needs a release feed from the network, so it runs in the background instead of
/// holding up the response (a slow or blocked network used to add up to 3 s to the first
/// command of every session). Its warning rides on the first response after it finishes.
pub(super) async fn warn_if_browser_outdated(state: &mut DaemonState, response: &mut Value) {
    if state
        .browser_security_pending
        .as_ref()
        .is_some_and(|task| task.is_finished())
    {
        if let Some(task) = state.browser_security_pending.take() {
            if let Ok(Some(w)) = task.await {
                append_warning(response, &w);
            }
        }
    }
    let disabled = env::var("FROB_NO_VERSION_CHECK").is_ok_and(|v| v == "1" || v == "true");
    if state.active_provider_connection || disabled {
        return;
    }
    let Some(mgr) = state.browser.as_mut() else {
        return;
    };
    if mgr.product_checked() {
        return;
    }
    let Ok(Ok(version)) = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        mgr.client
            .send_command_no_params("Browser.getVersion", None),
    )
    .await
    else {
        return;
    };
    mgr.mark_product_checked();
    let Some(product) = version.get("product").and_then(|v| v.as_str()) else {
        return;
    };
    if state.browser_security_checked.as_deref() == Some(product) {
        return;
    }
    state.browser_security_checked = Some(product.to_string());
    let product = product.to_string();
    state.browser_security_pending = Some(tokio::spawn(async move {
        crate::browser_security::outdated_warning(&product).await
    }));
}

pub(super) async fn execute_command_inner(cmd: &Value, state: &mut DaemonState) -> Value {
    let action = cmd.get("action").and_then(|v| v.as_str()).unwrap_or("");
    // Unlike normal auth login, no-navigation mode must never launch a
    // browser or manufacture an about:blank page to satisfy the command.
    let auth_login_no_navigate = is_auth_login_no_navigate(cmd);
    if let Some(mode @ ("instant" | "smooth" | "human")) =
        cmd.get("defaultInputMode").and_then(Value::as_str)
    {
        // Only an explicit session setting persists. inputMode is an override
        // for this command, including --human and MCP's human argument.
        state.input_mode = mode.to_string();
    }
    let id = cmd
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let cmd_start = std::time::Instant::now();

    // A retry of the request that wedged the tab must not run on its blank
    // replacement: `url` would report about:blank as a success.
    if let Some((_, report)) = state
        .undelivered_wedge
        .as_ref()
        .filter(|(wedged_id, _)| *wedged_id == id)
    {
        let mut resp = error_response(&id, &report.error_message());
        resp["data"] = report.data();
        return resp;
    }

    if let Err(err) = validate_restore_config_from_command(cmd) {
        return error_response(&id, &err);
    }

    // Invalid inputs are rejected before expensive setup: an unsupported
    // `find` action must fail here, not after a browser launch and a locator
    // resolution that can mask it with "element not found".
    if let Err(err) = validate_find_subaction(action, cmd) {
        return error_response(&id, &err);
    }

    if action == INTERNAL_DAEMON_SHUTDOWN_ACTION {
        let mut resp = match close_session(state, false).await {
            Ok(data) => success_response(&id, data),
            Err(e) => error_response(&id, &crate::native::browser::to_ai_friendly_error(&e)),
        };
        inject_lifecycle(&mut resp, state, false, false, false);
        return resp;
    }

    if let Some(ref server) = state.stream_server {
        let mut broadcast_cmd;
        let has_internal_fields = cmd.get("plugins").is_some()
            || cmd.get("settings").is_some()
            || cmd.get("pinTab").is_some()
            || cmd.get("restoreKey").is_some()
            || cmd.get("restoreSave").is_some()
            || cmd.get("restoreCheckUrl").is_some()
            || cmd.get("restoreCheckText").is_some()
            || cmd.get("restoreCheckFn").is_some();
        let cmd_for_broadcast = if has_internal_fields {
            broadcast_cmd = cmd.clone();
            if let Some(obj) = broadcast_cmd.as_object_mut() {
                obj.remove("plugins");
                obj.remove("settings");
                obj.remove("pinTab");
                obj.remove("restoreKey");
                obj.remove("restoreSave");
                obj.remove("restoreCheckUrl");
                obj.remove("restoreCheckText");
                obj.remove("restoreCheckFn");
            }
            &broadcast_cmd
        } else {
            cmd
        };
        server.broadcast_command(action, &id, cmd_for_broadcast);
    }

    // Drain and apply pending CDP events (console, errors, screencast frames, target lifecycle)
    if let Err(e) = state.drain_cdp_events_background().await {
        return error_response(&id, &crate::native::browser::to_ai_friendly_error(&e));
    }

    // After the drain, so late requests from the previous command keep its tag.
    begin_flow_step(action, cmd, state).await;

    // Keep element resolution in sync with the `frame` selection (see
    // element::set_active_frame for why this is mirrored).
    crate::native::element::set_active_frame(state.active_frame_id.as_deref());

    // `--pin-tab` from the client enables strict tab binding even when the
    // daemon was started without the flag, and `--no-pin-tab` (pinTab: false)
    // disables a sticky pin restored from disk or enabled earlier. Both are
    // persisted with the binding so the setting survives daemon restarts;
    // absence of the field leaves the current state untouched.
    match cmd.get("pinTab").and_then(|v| v.as_bool()) {
        Some(pin) if pin != state.pin_tab => {
            // Persist the pinned state before committing it to live state, so a
            // load/save failure leaves the daemon exactly as it was instead of
            // reporting an error while the isolation mode already flipped.
            match tab_binding::load(&state.session_id) {
                Ok(Some(mut binding)) => {
                    if binding.pinned != pin {
                        binding.pinned = pin;
                        if let Err(e) = tab_binding::save(&state.session_id, &binding) {
                            return error_response(
                                &id,
                                &format!(
                                    "cannot persist the pin-tab setting: {} — the change was \
                                     not applied",
                                    e
                                ),
                            );
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return error_response(
                        &id,
                        &format!(
                            "cannot read the session tab binding to update the pin state: {} \
                             — delete the binding file to reset the session",
                            e
                        ),
                    );
                }
            }
            // Persistence succeeded (or there was nothing to write): commit the
            // live state, and force a re-persist from the snapshot next time.
            state.pin_tab = pin;
            if let Some(ref mut mgr) = state.browser {
                mgr.set_pin_tab(pin);
            }
            state.last_persisted_binding = None;
        }
        _ => {}
    }

    // The settings this invocation set ride on the command. Live settings
    // apply now, before the policy checks, so a changed policy governs this
    // command too. Launch settings wait until the launch itself: a command that
    // is denied or needs confirmation leaves the browser and its settings alone.
    let launches = command_launches_browser(cmd);
    let mut launch_patch: Option<Patch> = None;
    let mut settings_outcome = if action == "close" {
        SettingsOutcome::default()
    } else {
        let patch = match Patch::from_wire(cmd.get("settings")) {
            Ok(patch) => patch,
            Err(e) => return error_response(&id, &e.to_string()),
        };
        if let Err(e) = check_capture_setting(&patch) {
            return error_response(&id, &e);
        }
        let outcome = match apply_settings_patch(state, &patch, false).await {
            Ok(outcome) => outcome,
            Err(e) => return error_response(&id, &e),
        };
        if launches {
            let mut patch = patch;
            if let Err(e) = adjust_ca_patch(state, &mut patch) {
                return error_response(&id, &e);
            }
            launch_patch = Some(patch);
        }
        outcome
    };
    // The settings the browser will be launched with, when the patch changes them.
    let launch_plan = launch_patch
        .as_ref()
        .map(|patch| state.settings.plan(patch))
        .filter(|plan| plan.relaunch);

    let skip_launch = skip_launch_action(action);
    let restore_key_change_needs_launch = !skip_launch
        && command_changes_restore_key(cmd, state)
        && has_active_browser_session(state);
    let needs_launch = if !skip_launch && !auth_login_no_navigate {
        // Check if existing connection is stale and needs re-launch.
        // This must happen before policy evaluation so plugin capability
        // actions are gated when recovery relaunches would invoke plugins.
        if restore_key_change_needs_launch || launch_plan.is_some() {
            true
        } else if let Some(ref mut mgr) = state.browser {
            mgr.has_process_exited() || !mgr.is_connection_alive().await
        } else {
            state.webdriver_backend.is_none()
        }
    } else {
        false
    };
    let mut lifecycle_reused = false;
    let mut lifecycle_launched = false;
    let mut lifecycle_relaunched_browser = false;
    // The flow is read once per command: policy, confirmation and the run all
    // see the same file.
    let mut run_flow = match load_run_flow(cmd, action, state) {
        Ok(flow) => flow,
        Err(e) => return error_response(&id, &e),
    };
    // A launch that changes where the session connects no longer resumes the
    // browser it was attached to, so that attachment does not decide its policy.
    let switches_connection = launch_plan.as_ref().is_some_and(|plan| {
        plan.changes.iter().any(|change| {
            matches!(
                change.key,
                Key::Cdp | Key::AutoConnect | Key::Provider | Key::CdpHeaders
            )
        })
    });
    let policy_actions = policy_actions_for_command(
        cmd,
        action,
        needs_launch,
        state
            .attached_target
            .as_ref()
            .filter(|_| !switches_connection),
        run_flow.as_deref(),
        launch_plan
            .as_ref()
            .map_or(&state.settings, |plan| &plan.next),
    );

    // Hot-reload and check action policy
    if let Some(ref mut policy) = state.policy {
        policy.reload();
        let mut confirmation_required: Option<String> = None;
        for policy_action in &policy_actions {
            match policy.check(policy_action) {
                PolicyResult::Allow => {}
                PolicyResult::Deny(reason) => {
                    let mut resp = error_response(
                        &id,
                        &format!("Action '{}' denied by policy: {}", policy_action, reason),
                    );
                    attach_settings_outcome(&mut resp, &settings_outcome);
                    return resp;
                }
                PolicyResult::RequiresConfirmation => {
                    if !state.confirmed_policy_actions.contains(policy_action)
                        && confirmation_required.is_none()
                    {
                        confirmation_required = Some(policy_action.to_string());
                    }
                }
            }
        }
        if let Some(policy_action) = confirmation_required {
            state.pending_confirmation = Some(PendingConfirmation {
                action: policy_action.clone(),
                cmd: cmd.clone(),
                approved_actions: state.confirmed_policy_actions.iter().cloned().collect(),
                flow: run_flow.clone(),
            });
            let mut resp = json!({
                "id": id,
                "success": true,
                "data": {
                    "confirmation_required": true,
                    "confirmation_id": id,
                    "action": policy_action
                },
            });
            attach_settings_outcome(&mut resp, &settings_outcome);
            return resp;
        }
    }

    // Check FROB_CONFIRM_ACTIONS (category-based, independent of policy file)
    if action != "confirm" && action != "deny" {
        if let Some(ref ca) = state.confirm_actions {
            for policy_action in &policy_actions {
                if state.confirmed_policy_actions.contains(policy_action) {
                    continue;
                }
                if ca.requires_confirmation(policy_action) {
                    state.pending_confirmation = Some(PendingConfirmation {
                        action: policy_action.to_string(),
                        cmd: cmd.clone(),
                        approved_actions: state.confirmed_policy_actions.iter().cloned().collect(),
                        flow: run_flow.clone(),
                    });
                    let mut resp = json!({
                        "id": id,
                        "success": true,
                        "data": {
                            "confirmation_required": true,
                            "confirmation_id": id,
                            "action": policy_action,
                        },
                    });
                    attach_settings_outcome(&mut resp, &settings_outcome);
                    return resp;
                }
            }
        }
    }

    // A write `--allow-write` does not cover needs an explicit confirmation,
    // whatever the policy file and --confirm-actions say.
    if let Some(resp) = park_unconfirmed_flow_write(&id, cmd, action, run_flow.as_deref(), state) {
        return resp;
    }

    let restore_transition_closed_browser = match async {
        if auth_login_no_navigate && command_changes_restore_key(cmd, state) {
            return Err(
                "auth login --no-navigate cannot switch restore sessions; select the session containing the active login page"
                    .to_string(),
            );
        }
        apply_restore_config_after_confirmation(cmd, state).await
    }
    .await
    {
        Ok(closed_browser) => closed_browser,
        Err(err) => {
            let mut resp = error_response(&id, &err);
            attach_settings_outcome(&mut resp, &settings_outcome);
            return resp;
        }
    };

    if !skip_launch {
        if needs_launch {
            // Past the policy gates: the launch settings take effect now.
            if let (Some(patch), Some(_)) = (launch_patch.as_ref(), launch_plan.as_ref()) {
                match apply_settings_patch(state, patch, true).await {
                    Ok(outcome) => settings_outcome.absorb(outcome),
                    Err(e) => {
                        let mut resp = error_response(&id, &e);
                        attach_settings_outcome(&mut resp, &settings_outcome);
                        return resp;
                    }
                }
            }
            lifecycle_relaunched_browser |= settings_outcome.relaunched_browser;
            lifecycle_relaunched_browser |= restore_transition_closed_browser
                || state.browser.is_some()
                || state.active_provider_session.is_some();
            if state.browser.is_some() || state.active_provider_session.is_some() {
                let _ = auto_save_restore_state(state).await;
            }
            let strict_state = launch_patch
                .as_ref()
                .is_some_and(|patch| patch.value(Key::StorageState).is_some());
            let launched = auto_launch_with(
                state,
                plugins_from_command_or_env(cmd),
                &Value::Null,
                strict_state,
            )
            .await;
            let launched = match launched {
                Ok(_) => Box::pin(start_flow_capture(state)).await,
                Err(e) => Err(e),
            };
            if let Err(e) = launched {
                let context = auto_launch_error_context(matches!(
                    state.settings.connection(),
                    Connection::Cdp(_)
                ));
                let rollback = rollback_launch_settings(state, &mut settings_outcome, &e).await;
                let e = with_rollback_error(&e, rollback);
                let mut resp = error_response(&id, &format!("{}: {}", context, e));
                attach_settings_outcome(&mut resp, &settings_outcome);
                return resp;
            }
            state.webmcp_enabled = state.settings.webmcp();
            lifecycle_launched = true;
        } else {
            lifecycle_reused = true;
        }

        if !auth_login_no_navigate {
            if let Some(ref mut mgr) = state.browser {
                if mgr.page_count() == 0 {
                    // ensure_page itself skips creation for a pinned tab_gone
                    // session, so a closed sole tab stays tab_gone instead of
                    // silently recovering onto a fresh blank page.
                    let _ = mgr.ensure_page().await;
                }
            }
        }
    }

    // WebDriver backend: reject unsupported CDP-only actions
    if matches!(state.backend_type, BackendType::WebDriver)
        && WEBDRIVER_UNSUPPORTED_ACTIONS.contains(&action)
    {
        let mut resp = error_response(
            &id,
            &format!(
                "Action '{}' is not supported on the WebDriver backend",
                action
            ),
        );
        // The launch above may have run; report what it applied.
        attach_settings_outcome(&mut resp, &settings_outcome);
        return resp;
    }

    // A pending confirm/prompt dialog blocks the renderer's main thread, so
    // any command that touches the page would hang until the client read
    // timeout. Fail fast with instructions instead. Actions in skip_launch
    // never touch the page; dialog/screenshot/url/title are browser-side.
    // Only a dialog on the ACTIVE tab blocks: one on a background tab leaves
    // the active tab's renderer responsive.
    if let Some(ref dialog) = state.pending_dialog {
        let active_session = state
            .browser
            .as_ref()
            .and_then(|m| m.active_session_id().ok().map(|s| s.to_string()));
        let on_active_tab = match (&dialog.session_id, &active_session) {
            (Some(dialog_sid), Some(active_sid)) => dialog_sid == active_sid,
            // No session on the event = top-level page dialog; no browser = be safe.
            _ => true,
        };
        // Tab and session management must stay usable: switching or closing
        // tabs is exactly how an agent escapes a tab blocked by a dialog.
        let read_touches_active_tab = action == "read"
            && cmd.get("url").is_none()
            && cmd.get("llms").is_none()
            && !cmd
                .get("requireMd")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        let safe_during_dialog = (skip_launch && !read_touches_active_tab)
            || matches!(
                action,
                "dialog"
                    | "screenshot"
                    | "url"
                    | "title"
                    | "tab_list"
                    | "tab_new"
                    | "tab_switch"
                    | "tab_close"
            );
        if on_active_tab && !safe_during_dialog {
            let mut resp = error_response(
                &id,
                &format!(
                    "A JavaScript {} dialog is blocking the page: \"{}\". Resolve it with `dialog accept` or `dialog dismiss`, then retry `{}`.",
                    dialog.dialog_type, dialog.message, action
                ),
            );
            attach_settings_outcome(&mut resp, &settings_outcome);
            return resp;
        }
    }

    // Observe after the action: record the page now, describe the change once it settles.
    let observe_before = if observe::wanted(cmd, action) && state.browser.is_some() {
        Some(observe::before(state, action).await)
    } else {
        None
    };

    let mut result = match action {
        "launch" => {
            let applied = match launch_patch.as_ref() {
                Some(patch) => apply_settings_patch(state, patch, true)
                    .await
                    .map(|outcome| settings_outcome.absorb(outcome)),
                None => Ok(()),
            };
            lifecycle_relaunched_browser |= settings_outcome.relaunched_browser;
            let result = match applied {
                Ok(()) => handle_launch(cmd, state).await,
                Err(e) => Err(e),
            };
            match result {
                Ok(data) => {
                    state.webmcp_enabled = state.settings.webmcp();
                    Ok(data)
                }
                Err(e) => {
                    let rollback = rollback_launch_settings(state, &mut settings_outcome, &e).await;
                    Err(with_rollback_error(&e, rollback))
                }
            }
        }
        "flow_steps" | "flow_save" | "flow_run" | "flow_list" => {
            handle_flow_command(action, cmd, state, run_flow.take()).await
        }
        _ => run_command(action, cmd, state).await,
    };

    if let Some(before) = observe_before {
        if result.is_ok() {
            let observation = observe::after(state, before).await;
            if let Ok(Value::Object(ref mut data)) = result {
                data.insert("observe".to_string(), observation);
            }
        }
    }

    if result.is_ok() && should_validate_restore_after_action(action) {
        validate_restore_if_pending(state).await;
    }

    // Stamp browser-touching commands so periodic autosave waits for an
    // active command burst to settle before collecting state. Stamped even on
    // error: a failed click can still have navigated.
    if !skip_launch {
        state.last_command_finished = Some(std::time::Instant::now());
    }

    let mut wedge = None;
    if result.is_ok() {
        if let Some(mgr) = state.browser.as_mut() {
            mgr.clear_wedge_suspect_if_responsive().await;
        }
    }
    if let Err(ref e) = result {
        if e.contains("CDP command timed out") {
            // A dialog opened by this command blocks the renderer like a
            // wedge does; its event is only seen once events are drained.
            let _ = state.drain_cdp_events_background().await;
            let dialog_session = state
                .pending_dialog
                .as_ref()
                .and_then(|d| d.session_id.clone());
            let may_replace = may_replace_wedged_tab(state);
            if let Some(mgr) = state.browser.as_mut() {
                wedge = mgr
                    .handle_command_timeout(dialog_session.as_deref(), may_replace)
                    .await;
            }
            if let Some(old_session_id) = wedge
                .as_ref()
                .filter(|w| w.replacement_target_id.is_some())
                .map(|w| w.session_id.clone())
            {
                if let Err(e) = adopt_replacement_tab(state, &old_session_id).await {
                    result = Err(e);
                    wedge = None;
                }
            }
        }
    }

    // Persist the session-to-tab binding (write-on-change) so a fresh daemon
    // re-attaches to this session's tab instead of adopting the browser's
    // most recently active one.
    let persist_error = if !skip_launch {
        maybe_persist_tab_binding(state)
    } else {
        None
    };

    let mut resp = match (result, &wedge) {
        (_, Some(report)) => {
            let mut resp = error_response(&id, &report.error_message());
            resp["data"] = report.data();
            resp
        }
        (Ok(data), None) => success_response(&id, data),
        (Err(e), None) => error_response(&id, &crate::native::browser::to_ai_friendly_error(&e)),
    };
    attach_tab_gone_data(&mut resp, state);
    attach_flow_stop(&mut resp, state);
    if resp.get("success").and_then(Value::as_bool) == Some(false) {
        attach_settings_outcome(&mut resp, &settings_outcome);
    }
    if let Some(report) = wedge {
        state.undelivered_wedge = Some((id.clone(), report));
    } else if let Some((_, report)) = state.undelivered_wedge.take() {
        // The wedged command's response never reached the client.
        append_warning(&mut resp, &report.error_message());
    }

    // A failed binding write is retried on the next command, but a pinned
    // session must know its isolation may not survive a daemon restart.
    if let Some(err) = persist_error {
        let hint = if state.pin_tab {
            " --pin-tab isolation may not survive a daemon restart until the binding persists."
        } else {
            ""
        };
        append_warning(
            &mut resp,
            &format!(
                "Failed to persist the session tab binding: {}.{}",
                err, hint
            ),
        );
    }
    inject_lifecycle(
        &mut resp,
        state,
        lifecycle_reused,
        lifecycle_launched,
        lifecycle_relaunched_browser,
    );
    attach_settings_outcome(&mut resp, &settings_outcome);

    // Re-drain so a dialog opened by THIS command is reflected in the warning
    // below; events are otherwise only drained at the start of a command.
    if let Err(e) = state.drain_cdp_events_background().await {
        resp = error_response(&id, &crate::native::browser::to_ai_friendly_error(&e));
        attach_tab_gone_data(&mut resp, state);
        inject_lifecycle(
            &mut resp,
            state,
            lifecycle_reused,
            lifecycle_launched,
            lifecycle_relaunched_browser,
        );
        attach_settings_outcome(&mut resp, &settings_outcome);
    }
    attach_webmcp_availability(&mut resp, action, state).await;
    attach_flow_availability(&mut resp, action, state).await;

    // Auto-report pending JavaScript dialog so agents know why commands may hang
    if action != "dialog" {
        if let Some(ref dialog) = state.pending_dialog {
            append_warning(
                &mut resp,
                &format!(
                    "A JavaScript {} dialog is blocking the page: \"{}\" — use `dialog accept` or `dialog dismiss` to resolve it",
                    dialog.dialog_type, dialog.message
                ),
            );
        }
    }

    if let Some(ref server) = state.stream_server {
        let duration_ms = cmd_start.elapsed().as_millis() as u64;
        let success = resp
            .get("status")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s == "success");
        let data = resp.get("data").cloned().unwrap_or(Value::Null);
        server.broadcast_result(&id, action, success, &data, duration_ms);

        if let Some(ref mgr) = state.browser {
            let tabs = mgr.tab_list();
            let session_id = mgr.active_session_id().ok().map(|s| s.to_string());
            server
                .bind_cdp_session_and_broadcast_tabs(session_id, &tabs)
                .await;
        }
    }

    resp
}

pub(super) fn auto_launch_error_context(has_cdp: bool) -> &'static str {
    if has_cdp {
        "CDP connection failed"
    } else {
        "Auto-launch failed"
    }
}

// ---------------------------------------------------------------------------
// Session-to-tab binding
// ---------------------------------------------------------------------------

pub(super) async fn handle_confirm(_cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let pending = state
        .pending_confirmation
        .take()
        .ok_or("No pending confirmation")?;

    let mut approved_actions = pending.approved_actions.clone();
    if !approved_actions.iter().any(|a| a == &pending.action) {
        approved_actions.push(pending.action.clone());
    }
    let previous_confirmed = std::mem::replace(
        &mut state.confirmed_policy_actions,
        approved_actions.into_iter().collect(),
    );
    state.preloaded_flow = pending.flow.clone();
    let replay = confirmation_replay_command(&pending.cmd);
    let result = Box::pin(execute_command(&replay, state)).await;
    state.preloaded_flow = None;
    state.confirmed_policy_actions = previous_confirmed;

    Ok(json!({ "confirmed": true, "action": pending.action, "result": result }))
}

/// The parked command as `confirm` replays it: without its action policy and
/// confirm actions, which already applied when it was parked, so the replay
/// runs under the session's current policy. Its allowed domains stay, since
/// they apply only at the launch the replay runs; a change to the session's
/// allowed domains drops the pending confirmation instead.
pub(super) fn confirmation_replay_command(cmd: &Value) -> Value {
    let mut replay = cmd.clone();
    if let Some(object) = replay.as_object_mut() {
        if let Ok(patch) = Patch::from_wire(object.get("settings")) {
            match patch.without_live_security_settings().to_wire() {
                Some(wire) => {
                    object.insert("settings".to_string(), wire);
                }
                None => {
                    object.remove("settings");
                }
            }
        }
    }
    replay
}

pub(super) async fn handle_deny(_cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let pending = state
        .pending_confirmation
        .take()
        .ok_or("No pending confirmation")?;

    Ok(json!({ "denied": true, "action": pending.action }))
}

// ---------------------------------------------------------------------------
// iOS handlers (stub)
// ---------------------------------------------------------------------------
