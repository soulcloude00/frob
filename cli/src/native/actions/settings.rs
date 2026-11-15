use super::*;

/// The settings a `launch` command names, as a patch. A client that speaks
/// the launch protocol directly (rather than through the CLI) sets session
/// settings this way; the CLI sends the same settings as a patch on every
/// command.
pub(super) fn launch_envelope_patch(cmd: &Value, engine: Option<&str>) -> Patch {
    let mut entries: Vec<(Key, Requested)> = Vec::new();
    let mut set = |key: Key, value: Value| entries.push((key, Requested::Set(value)));
    let string = |field: &str| cmd.get(field).and_then(Value::as_str).map(Value::from);
    let flag = |field: &str| cmd.get(field).and_then(Value::as_bool).map(Value::Bool);
    let list = |field: &str| string_array_from_command(cmd, field).map(Value::from);

    match AttachedTarget::named_by(cmd) {
        Some(AttachedTarget::CdpUrl(url)) => set(Key::Cdp, Value::from(url)),
        Some(AttachedTarget::CdpPort(port)) => set(Key::Cdp, Value::from(port.to_string())),
        Some(AttachedTarget::AutoConnect) => set(Key::AutoConnect, Value::Bool(true)),
        Some(AttachedTarget::Provider(provider)) => set(Key::Provider, Value::from(provider)),
        None => {}
    }
    if let Some(headers) = cmd.get("cdpHeaders").filter(|h| h.is_object()) {
        set(Key::CdpHeaders, Value::from(headers.to_string()));
    }
    if let Some(headless) = cmd.get("headless").and_then(Value::as_bool) {
        set(Key::Headed, Value::Bool(!headless));
    }
    for (field, key) in [
        ("profile", Key::Profile),
        ("storageState", Key::StorageState),
        ("executablePath", Key::ExecutablePath),
        ("userAgent", Key::UserAgent),
        ("colorScheme", Key::ColorScheme),
        ("downloadPath", Key::DownloadPath),
        ("engine", Key::Engine),
        ("caCert", Key::CaCert),
    ] {
        if let Some(value) = string(field) {
            set(key, value);
        }
    }
    for (field, key) in [
        ("ignoreHTTPSErrors", Key::IgnoreHttpsErrors),
        ("allowFileAccess", Key::AllowFileAccess),
        ("hideScrollbars", Key::HideScrollbars),
        ("webgpu", Key::Webgpu),
        ("webmcp", Key::Webmcp),
        ("sharedBrowser", Key::SharedBrowser),
        ("noXvfb", Key::NoXvfb),
        ("clearCaCert", Key::ClearCaCert),
    ] {
        if let Some(value) = flag(field) {
            set(key, value);
        }
    }
    for (field, key) in [
        ("args", Key::Args),
        ("extensions", Key::Extensions),
        ("initScripts", Key::InitScripts),
        ("enable", Key::Enable),
        ("capture", Key::Capture),
    ] {
        if let Some(value) = list(field) {
            set(key, value);
        }
    }
    if let Some(proxy) = cmd.get("proxy") {
        let (server, username, password, bypass) = match proxy {
            Value::String(raw) => {
                let parsed = crate::session_settings::parse_proxy(raw);
                (Some(parsed.server), parsed.username, parsed.password, None)
            }
            other => {
                let field = |name: &str| other.get(name).and_then(Value::as_str).map(String::from);
                (
                    field("server"),
                    field("username"),
                    field("password"),
                    field("bypass"),
                )
            }
        };
        for (key, value) in [
            (Key::Proxy, server),
            (Key::ProxyUsername, username),
            (Key::ProxyPassword, password),
            (Key::ProxyBypass, bypass),
        ] {
            if let Some(value) = value {
                set(key, Value::from(value));
            }
        }
    }
    drop(set);
    // Obscura takes its proxy bypass from every launch, so an explicit null
    // clears it.
    if engine == Some("obscura") {
        match cmd.get("proxyBypass") {
            Some(Value::String(bypass)) => {
                entries.retain(|(key, _)| *key != Key::ProxyBypass);
                entries.push((
                    Key::ProxyBypass,
                    Requested::Set(Value::from(bypass.as_str())),
                ));
            }
            Some(Value::Null) => {
                entries.retain(|(key, _)| *key != Key::ProxyBypass);
                entries.push((Key::ProxyBypass, Requested::Default));
            }
            _ => {}
        }
    }
    if let Some(domains) = allowed_domains_from_launch_command(cmd) {
        entries.push((
            Key::AllowedDomains,
            if domains.is_empty() {
                Requested::Default
            } else {
                Requested::Set(Value::from(domains))
            },
        ));
    }
    Patch::from_entries(entries)
}

/// What applying a settings patch did to the session.
#[derive(Default)]
pub(super) struct SettingsOutcome {
    /// Each change, as reported in `lifecycle.settings.changes`.
    pub(super) changes: Vec<Value>,
    /// The patch asked for a relaunch whether or not anything changed.
    pub(super) forced: bool,
    /// A running browser was closed so the next launch uses the new settings.
    pub(super) relaunched_browser: bool,
    /// What to put back if the launch that follows fails.
    pub(super) pending: Option<PendingSettings>,
}

/// The launch settings in force before a patch, kept until the launch that
/// uses the new ones succeeds.
pub(super) struct PendingSettings {
    keys: Vec<Key>,
    before: Running,
    filter: Option<DomainFilter>,
    ca_cert: Option<EffectiveCaCert>,
    attached: Option<AttachedTarget>,
    cdp_headers: Option<(String, Vec<(String, String)>)>,
}

impl SettingsOutcome {
    /// Takes the launch half of a patch that was planned before the live half
    /// was applied. The launch changes were reported as pending; they are
    /// replaced by what the launch actually did.
    pub(super) fn absorb(&mut self, launch: SettingsOutcome) {
        self.changes.retain(|change| change["effect"] == "live");
        self.changes.extend(launch.changes);
        self.forced = launch.forced;
        self.relaunched_browser = launch.relaunched_browser;
        self.pending = launch.pending;
    }

    /// Called when the launch failed and the launch settings were put back.
    fn mark_rolled_back(&mut self, keys: &[Key]) {
        self.relaunched_browser = false;
        let names: Vec<&str> = keys.iter().map(|key| spec(*key).name).collect();
        for change in &mut self.changes {
            if change["effect"] == "relaunch"
                && change["option"]
                    .as_str()
                    .is_some_and(|o| names.contains(&o))
            {
                change["applied"] = json!(false);
            }
        }
    }
}

/// True for the errors that reject a launch because `--allowed-domains` cannot
/// be enforced for it (a profile, a provider, an attach, a replayed state).
pub(super) fn is_containment_rejection(error: &str) -> bool {
    error.contains("--allowed-domains is not supported")
}

/// Puts back the launch settings a failed launch tried to use, so later
/// commands launch with what worked before. Live settings stay applied.
///
/// A launch that fails after the filter was installed (the browser would not
/// start) never widens containment: the allowed domains the command asked for
/// stay installed. A launch rejected because the allowed domains cannot be
/// enforced for it is rejected whole, so they are put back too.
///
/// Returns why the stored security settings could not be put back, if so.
pub(super) async fn rollback_launch_settings(
    state: &mut DaemonState,
    outcome: &mut SettingsOutcome,
    error: &str,
) -> Option<String> {
    let pending = outcome.pending.take()?;
    let rejected = is_containment_rejection(error);
    let restored: Vec<Key> = pending
        .keys
        .iter()
        .copied()
        .filter(|key| rejected || spec(*key).persist != Persist::Secured)
        .collect();
    for key in &restored {
        state
            .settings
            .put_back(*key, pending.before.get(*key).cloned());
    }
    if rejected {
        *state.domain_filter.write().await = pending.filter;
    }
    state.effective_ca_cert = pending.ca_cert;
    state.attached_target = pending.attached;
    state.cdp_headers = pending.cdp_headers;
    outcome.mark_rolled_back(&restored);
    if rejected {
        return state
            .store
            .save(&state.settings.security())
            .err()
            .map(|e| format!("the stored security settings could not be restored: {}", e));
    }
    None
}

/// The launch error with the rollback problem, if any, appended.
pub(super) fn with_rollback_error(error: &str, rollback: Option<String>) -> String {
    match rollback {
        Some(why) => format!("{} ({})", error, why),
        None => error.to_string(),
    }
}

/// CA trust is compared by content, not by path: the same certificates from
/// another path change nothing, and an edited file at the same path is a
/// change.
pub(super) fn adjust_ca_patch(state: &mut DaemonState, patch: &mut Patch) -> Result<(), String> {
    let Some(path) = patch
        .value(Key::CaCert)
        .and_then(Value::as_str)
        .map(String::from)
    else {
        return Ok(());
    };
    let requested = resolve_effective_ca_cert(&json!({ "caCert": path }), state)?;
    let digest = |ca: &Option<EffectiveCaCert>| ca.as_ref().map(|ca| *ca.bundle.digest());
    let running_path = state.settings.get(Key::CaCert).and_then(Value::as_str);
    match ca_trust_change(
        running_path,
        digest(&state.effective_ca_cert),
        &path,
        digest(&requested),
    ) {
        CaTrustChange::Unchanged => {
            patch.remove(Key::CaCert);
            state.settings.set(Key::CaCert, Value::from(path));
        }
        CaTrustChange::EditedInPlace => patch.mark_changed_in_place(Key::CaCert),
        CaTrustChange::Replaced => {}
    }
    Ok(())
}

/// Plans `patch` against the session's settings and applies the plan. Live
/// settings change in place. Launch settings close the running browser so the
/// launch that follows uses them, and they are committed for good only once
/// that launch succeeds (`rollback_launch_settings` puts them back).
///
/// A command that never launches a browser (`launches` is false) applies only
/// the live settings. Its launch changes are reported as not applied and stay
/// pending until a command that launches carries them again.
pub(super) async fn apply_settings_patch(
    state: &mut DaemonState,
    patch: &Patch,
    launches: bool,
) -> Result<SettingsOutcome, String> {
    let mut patch = patch.clone();
    if launches {
        adjust_ca_patch(state, &mut patch)?;
    }

    let full = state.settings.plan(&patch);
    if full.is_empty() && full.ignored.is_empty() {
        return Ok(SettingsOutcome::default());
    }
    let ignored: Vec<Value> = full.ignored.iter().map(|c| c.to_ignored_json()).collect();
    let (plan, deferred) = if launches {
        (full, Vec::new())
    } else {
        let deferred: Vec<Value> = full
            .changes
            .iter()
            .filter(|change| change.effect() == Effect::Launch)
            .map(|change| change.to_json(false))
            .collect();
        (
            state.settings.plan(&patch.without_launch_settings()),
            deferred,
        )
    };
    let changed = |key: Key| plan.changes.iter().any(|change| change.key == key);

    // Load the CA trust first: a bad bundle fails before anything changes.
    let ca_cert = if changed(Key::CaCert) || changed(Key::ClearCaCert) {
        let request = match plan.next.get(Key::CaCert).and_then(Value::as_str) {
            Some(path) => json!({ "caCert": path }),
            None => json!({ "clearCaCert": true }),
        };
        Some(resolve_effective_ca_cert(&request, state)?)
    } else {
        None
    };

    // Security settings are stored before they are applied, so a failed write
    // leaves the session as it was and a stored setting never lags the running one.
    let next_security = plan.next.security();
    if next_security != state.settings.security() {
        state
            .store
            .save(&next_security)
            .map_err(|e| e.to_string())?;
    }

    let had_browser = has_active_browser_session(state);
    let relaunched_browser = plan.relaunch && had_browser;
    let mut changes: Vec<Value> = plan
        .changes
        .iter()
        .map(|change| {
            let applied = match change.effect() {
                Effect::Live => true,
                // The domain filter is installed at once, browser or not.
                Effect::Launch => had_browser || change.key == Key::AllowedDomains,
            };
            change.to_json(applied)
        })
        .collect();
    changes.extend(deferred);
    changes.extend(ignored);
    let connection_changed = [Key::Cdp, Key::AutoConnect, Key::Provider, Key::CdpHeaders]
        .into_iter()
        .any(changed);
    let touched: Vec<Key> = plan.changes.iter().map(|change| change.key).collect();
    let was = |key: Key| touched.contains(&key);

    let pending = if plan.relaunch {
        let mut keys: Vec<Key> = plan
            .changes
            .iter()
            .filter(|change| change.effect() == Effect::Launch)
            .map(|change| change.key)
            .collect();
        if keys.contains(&Key::ClearCaCert) {
            keys.push(Key::CaCert);
        }
        // Values dropped because the session went external come back with it.
        keys.extend(plan.ignored.iter().map(|change| change.key));
        Some(PendingSettings {
            keys,
            before: state.settings.clone(),
            filter: state.domain_filter.read().await.clone(),
            ca_cert: state.effective_ca_cert.clone(),
            attached: state.attached_target.clone(),
            cdp_headers: state.cdp_headers.clone(),
        })
    } else {
        None
    };
    let (next, relaunch) = (plan.next, plan.relaunch);

    if was(Key::DefaultTimeout) {
        state.default_timeout_ms = next.default_timeout_ms();
        // Later commands that omit the flag size their wait from the stored
        // config, so keep it in step.
        crate::connection::record_daemon_default_timeout(
            &state.session_id,
            next.get(Key::DefaultTimeout).and_then(Value::as_u64),
        );
    }
    if was(Key::IdleTimeout) {
        state.idle_activity.set_timeout(next.idle_timeout());
        state.idle_activity.mark();
    }
    if was(Key::NoAutoDialog) {
        state.auto_dialog = !next.no_auto_dialog();
    }
    if was(Key::ActionPolicy) || was(Key::ConfirmActions) || was(Key::AllowedDomains) {
        // An approval asked for under the old rules no longer stands.
        state.pending_confirmation = None;
    }
    if was(Key::ActionPolicy) {
        state.policy = next
            .security()
            .action_policy_path
            .as_deref()
            .map(ActionPolicy::from_path);
    }
    if was(Key::ConfirmActions) {
        state.confirm_actions = next
            .security()
            .confirm_actions
            .as_deref()
            .and_then(ConfirmActions::parse);
    }
    if was(Key::AllowedDomains) {
        *state.domain_filter.write().await = next
            .security()
            .allowed_domains
            .filter(|domains| !domains.is_empty())
            .map(|domains| DomainFilter::new(&domains.join(",")));
    }
    if let Some(ca_cert) = ca_cert {
        state.effective_ca_cert = ca_cert;
    }
    if was(Key::Cdp) || was(Key::CdpHeaders) {
        // The requested headers win over any cached for the endpoint, and
        // clearing them sends none.
        state.cdp_headers = match (
            next.get(Key::Cdp).and_then(Value::as_str),
            cdp_headers_from_settings(&next),
        ) {
            (Some(endpoint), Some(headers)) => Some((endpoint.to_string(), headers)),
            _ => None,
        };
    }
    if connection_changed {
        state.attached_target = None;
    }
    state.settings = next;

    if relaunch && had_browser {
        let _ = auto_save_restore_state(state).await;
        close_all_browser_backends(state).await?;
        state.ref_map.invalidate_all_documents();
    }

    Ok(SettingsOutcome {
        changes,
        forced: launches && patch.forces_relaunch(),
        relaunched_browser,
        pending,
    })
}

/// How a requested CA file compares to the trust in effect.
#[derive(Debug, PartialEq)]
pub(super) enum CaTrustChange {
    /// The same certificates, whichever path they come from.
    Unchanged,
    /// The same path, different certificates.
    EditedInPlace,
    Replaced,
}

pub(super) fn ca_trust_change(
    running_path: Option<&str>,
    running_digest: Option<[u8; 32]>,
    requested_path: &str,
    requested_digest: Option<[u8; 32]>,
) -> CaTrustChange {
    let same_content = running_digest.is_some() && running_digest == requested_digest;
    let same_path = running_path == Some(requested_path);
    match (same_content, same_path) {
        (true, _) => CaTrustChange::Unchanged,
        (false, true) => CaTrustChange::EditedInPlace,
        (false, false) => CaTrustChange::Replaced,
    }
}

/// Reports what a settings patch did in `lifecycle.settings`. A failed
/// response carries it too, so the caller can see what was applied before the
/// failure.
pub(super) fn attach_settings_outcome(resp: &mut Value, outcome: &SettingsOutcome) {
    if outcome.changes.is_empty() && !outcome.forced {
        return;
    }
    let report = json!({ "changes": outcome.changes, "forcedRelaunch": outcome.forced });
    if resp.get("success").and_then(Value::as_bool) == Some(false) {
        let data = resp
            .as_object_mut()
            .map(|obj| obj.entry("data").or_insert_with(|| json!({})));
        if let Some(data) = data.and_then(Value::as_object_mut) {
            let lifecycle = data.entry("lifecycle").or_insert_with(|| json!({}));
            if let Some(lifecycle) = lifecycle.as_object_mut() {
                lifecycle.insert("settings".to_string(), report);
            }
        }
        return;
    }
    let Some(data) = resp.get_mut("data").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(lifecycle) = data
        .entry("lifecycle")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        lifecycle.insert("settings".to_string(), report);
    }
}
