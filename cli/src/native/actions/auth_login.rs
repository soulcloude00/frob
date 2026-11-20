use super::*;

pub(super) async fn handle_credentials_set(cmd: &Value) -> Result<Value, String> {
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name'")?;
    let username = cmd
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'username'")?;
    let password = cmd
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'password'")?;
    let url = cmd.get("url").and_then(|v| v.as_str());
    auth::credentials_set(name, username, password, url)
}

pub(super) async fn handle_credentials_get(cmd: &Value) -> Result<Value, String> {
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name'")?;
    auth::credentials_get(name)
}

pub(super) async fn handle_credentials_delete(cmd: &Value) -> Result<Value, String> {
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name'")?;
    auth::credentials_delete(name)
}

pub(super) async fn handle_credentials_list() -> Result<Value, String> {
    auth::credentials_list()
}

pub(super) async fn handle_auth_show(cmd: &Value) -> Result<Value, String> {
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name'")?;
    auth::auth_show(name)
}

pub(super) async fn handle_http_credentials(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let username = cmd
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'username' parameter")?;
    let password = cmd
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'password' parameter")?;

    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("{}:{}", username, password),
    );

    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), format!("Basic {}", encoded));
    network::set_extra_headers(&mgr.client, &session_id, &headers).await?;
    // Network.setExtraHTTPHeaders replaces the target's complete global
    // header set, so keep the same replacement ready for tabs opened later.
    state.session_setup.extra_headers = Some(headers);

    Ok(json!({ "set": true }))
}

// ---------------------------------------------------------------------------
// Auth handlers
// ---------------------------------------------------------------------------

pub(super) struct AuthLoginElement {
    ref_map: RefMap,
}

impl AuthLoginElement {
    pub(super) async fn fill(
        &self,
        client: &crate::native::cdp::client::CdpClient,
        session_id: &str,
        value: &str,
    ) -> Result<(), String> {
        let (object_id, effective_session_id) = crate::native::element::resolve_element_object_id(
            client,
            session_id,
            &self.ref_map,
            AUTH_LOGIN_ELEMENT_REF,
            &HashMap::new(),
        )
        .await?;
        let result = client
            .send_command_typed::<_, crate::native::cdp::types::EvaluateResult>(
                "Runtime.callFunctionOn",
                &crate::native::cdp::types::CallFunctionOnParams {
                    function_declaration: r#"function(value) {
                        const editable = () => this.isConnected &&
                            !this.matches(':disabled') && !this.readOnly;
                        const focused = () => editable() &&
                            this.ownerDocument.activeElement === this;
                        if (!editable()) return false;
                        this.focus();
                        if (!focused()) return false;
                        if (typeof this.select === 'function') {
                            this.select();
                        } else if (this.isContentEditable ||
                            this.contentEditable === 'true' ||
                            this.contentEditable === 'plaintext-only') {
                            const range = this.ownerDocument.createRange();
                            range.selectNodeContents(this);
                            const selection = this.ownerDocument.getSelection();
                            selection.removeAllRanges();
                            selection.addRange(range);
                        } else {
                            return false;
                        }
                        if (!focused()) return false;
                        if (typeof this.ownerDocument.execCommand === 'function') {
                            const inserted = this.ownerDocument.execCommand(
                                value === '' ? 'delete' : 'insertText', false, value
                            );
                            return inserted && this.isConnected;
                        }
                        if (this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement) {
                            const prototype = this instanceof HTMLInputElement
                                ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
                            Object.getOwnPropertyDescriptor(prototype, 'value').set.call(this, value);
                        } else {
                            this.textContent = value;
                        }
                        this.dispatchEvent(new Event('input', { bubbles: true }));
                        return this.isConnected;
                    }"#
                    .to_string(),
                    object_id: Some(object_id.clone()),
                    arguments: Some(vec![crate::native::cdp::types::CallArgument {
                        value: Some(json!(value)),
                        object_id: None,
                    }]),
                    return_by_value: Some(true),
                    await_promise: Some(false),
                },
                Some(&effective_session_id),
            )
            .await;
        let mut object_ids = std::collections::HashSet::from([object_id]);
        if let Ok(result) = &result {
            object_ids.extend(result.result.object_id.clone());
            object_ids.extend(
                result
                    .exception_details
                    .as_ref()
                    .and_then(|details| details.exception.as_ref())
                    .and_then(|exception| exception.object_id.clone()),
            );
        }
        for object_id in object_ids {
            let _ = client
                .send_command(
                    "Runtime.releaseObject",
                    Some(json!({ "objectId": object_id })),
                    Some(&effective_session_id),
                )
                .await;
        }
        let result = result?;
        if result.exception_details.is_some() || result.result.value != Some(json!(true)) {
            return Err(
                "Could not fill the selected login field; it may have changed or lost focus"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// Wait for any selector in `selectors` to match a usable element and retain
/// that element's stable backend node identity for the subsequent interaction.
///
/// This is used by `auth_login` auto-detection so SPA login forms can render
/// after initial navigation without requiring global network-idle.
pub(super) async fn wait_for_any_selector(
    client: &crate::native::cdp::client::CdpClient,
    session_id: &str,
    selectors: &[&str],
    timeout_ms: u64,
) -> Result<AuthLoginElement, String> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);

    loop {
        for selector in selectors {
            let expression = format!(
                r#"(() => {{
                    return Array.from(document.querySelectorAll({sel})).find((el) => {{
                        const r = el.getBoundingClientRect();
                        const s = window.getComputedStyle(el);
                        const opacity = parseFloat(s.opacity || '1');
                        const isVisible =
                            r.width > 0 &&
                            r.height > 0 &&
                            s.visibility !== 'hidden' &&
                            s.display !== 'none' &&
                            (!Number.isFinite(opacity) || opacity > 0);

                        if (!isVisible || el.matches(':disabled')) return false;
                        if (el instanceof HTMLInputElement && el.type === 'hidden') return false;
                        if ((el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) && el.readOnly) return false;
                        return true;
                    }}) || null;
                }})()"#,
                sel = serde_json::to_string(selector).unwrap_or_default()
            );

            let result: crate::native::cdp::types::EvaluateResult = client
                .send_command_typed(
                    "Runtime.evaluate",
                    &crate::native::cdp::types::EvaluateParams {
                        expression,
                        return_by_value: Some(false),
                        await_promise: Some(true),
                    },
                    Some(session_id),
                )
                .await?;

            if result.exception_details.is_some()
                || result.result.subtype.as_deref() != Some("node")
            {
                let mut object_ids = std::collections::HashSet::new();
                object_ids.extend(result.result.object_id);
                object_ids.extend(
                    result
                        .exception_details
                        .and_then(|details| details.exception)
                        .and_then(|exception| exception.object_id),
                );
                for object_id in object_ids {
                    let _ = client
                        .send_command(
                            "Runtime.releaseObject",
                            Some(json!({ "objectId": object_id })),
                            Some(session_id),
                        )
                        .await;
                }
                continue;
            }

            let Some(object_id) = result.result.object_id else {
                continue;
            };
            let describe = client
                .send_command(
                    "DOM.describeNode",
                    Some(json!({ "objectId": object_id })),
                    Some(session_id),
                )
                .await;
            let _ = client
                .send_command(
                    "Runtime.releaseObject",
                    Some(json!({ "objectId": object_id })),
                    Some(session_id),
                )
                .await;
            let Ok(describe) = describe else {
                continue;
            };
            let Some(backend_node_id) = describe
                .pointer("/node/backendNodeId")
                .and_then(Value::as_i64)
            else {
                continue;
            };
            let mut ref_map = RefMap::new();
            ref_map.add_exact_backend_node(AUTH_LOGIN_ELEMENT_REF.to_string(), backend_node_id);
            return Ok(AuthLoginElement { ref_map });
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(format!("Wait timed out after {}ms", timeout_ms));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(
            AUTH_LOGIN_SELECTOR_POLL_INTERVAL_MS,
        ))
        .await;
    }
}

pub(super) async fn handle_auth_save(cmd: &Value) -> Result<Value, String> {
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name'")?;
    let url = cmd
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url'")?;
    let username = cmd
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'username'")?;
    let password = cmd
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'password'")?;
    let username_selector = cmd.get("usernameSelector").and_then(|v| v.as_str());
    let password_selector = cmd.get("passwordSelector").and_then(|v| v.as_str());
    let submit_selector = cmd.get("submitSelector").and_then(|v| v.as_str());
    auth::auth_save(
        name,
        url,
        username,
        password,
        username_selector,
        password_selector,
        submit_selector,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AuthLoginOrigin(pub(super) String);

#[derive(Debug, Clone)]
pub(super) struct AuthLoginBoundPage {
    target_id: String,
    session_id: String,
}

/// Parse a URL into the normalized HTTP(S) origin used to constrain
/// `auth login --no-navigate`. Paths, queries, and fragments are intentionally
/// excluded, and URL serialization normalizes explicit default ports.
pub(super) fn auth_login_origin(raw_url: &str, label: &str) -> Result<AuthLoginOrigin, String> {
    let has_unambiguous_authority = raw_url
        .split_once(':')
        .map(|(_, rest)| rest)
        .is_some_and(|rest| rest.starts_with("//") && !rest.starts_with("///"));
    let parsed = url::Url::parse(raw_url).map_err(|_| {
        format!(
            "Invalid {} URL: expected an absolute HTTP(S) URL with a host",
            label
        )
    })?;
    if !has_unambiguous_authority
        || !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
    {
        return Err(format!(
            "Invalid {} URL: expected an absolute HTTP(S) URL with a host",
            label
        ));
    }

    Ok(AuthLoginOrigin(parsed.origin().ascii_serialization()))
}

/// Bind no-navigation login to the current top-level page before any external
/// credential provider is invoked. This rejects missing, blank, opaque, and
/// otherwise unusable pages without reading a secret unnecessarily.
pub(super) async fn bind_auth_login_active_page(
    state: &DaemonState,
) -> Result<AuthLoginBoundPage, String> {
    let mgr = state
        .browser
        .as_ref()
        .ok_or(AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR)?;
    if mgr.page_count() == 0 {
        return Err(AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR.to_string());
    }
    let target_id = mgr
        .active_target_id()
        .map_err(|_| AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR.to_string())?
        .to_string();
    let session_id = mgr
        .active_session_id()
        .map_err(|_| AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR.to_string())?
        .to_string();
    let current_url = mgr
        .get_url()
        .await
        .map_err(|_| AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR.to_string())?;
    auth_login_origin(&current_url, "current page")
        .map_err(|_| AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR.to_string())?;

    Ok(AuthLoginBoundPage {
        target_id,
        session_id,
    })
}

/// Ensure credential-bearing work still targets the page that was active
/// when no-navigation login began, and that its live document remains on the
/// expected origin. Diagnostics contain origins only, never URL paths or
/// credential values.
pub(super) async fn validate_auth_login_active_page(
    mgr: &BrowserManager,
    bound_page: &AuthLoginBoundPage,
    expected_origin: &AuthLoginOrigin,
) -> Result<(), String> {
    let active_target_id = mgr.active_target_id().map_err(|_| {
        "Active browser page changed during auth login --no-navigate; retry on the intended login page"
            .to_string()
    })?;
    let active_session_id = mgr.active_session_id().map_err(|_| {
        "Active browser page changed during auth login --no-navigate; retry on the intended login page"
            .to_string()
    })?;
    if active_target_id != bound_page.target_id || active_session_id != bound_page.session_id {
        return Err(
            "Active browser page changed during auth login --no-navigate; retry on the intended login page"
                .to_string(),
        );
    }

    let current_url = mgr.get_url().await.map_err(|_| {
        "Active browser page changed during auth login --no-navigate; retry on the intended login page"
            .to_string()
    })?;
    let current_origin = auth_login_origin(&current_url, "current page")?;
    ensure_auth_login_origin_matches(&current_origin, expected_origin)
}

pub(super) fn ensure_auth_login_origin_matches(
    current_origin: &AuthLoginOrigin,
    expected_origin: &AuthLoginOrigin,
) -> Result<(), String> {
    if current_origin == expected_origin {
        return Ok(());
    }
    Err(format!(
        "Current page origin {} does not match credential origin {}",
        current_origin.0, expected_origin.0
    ))
}

pub(super) async fn handle_auth_login(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let name = cmd
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name'")?;
    let no_navigate = cmd
        .get("noNavigate")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let bound_page = if no_navigate {
        Some(bind_auth_login_active_page(state).await?)
    } else {
        None
    };
    if !no_navigate && state.browser.is_none() {
        return Err("Browser not launched".to_string());
    }
    let url_override = cmd.get("url").and_then(|v| v.as_str());
    let override_origin = if no_navigate {
        url_override
            .map(|url| auth_login_origin(url, "credential"))
            .transpose()?
    } else {
        None
    };
    if let (Some(bound_page), Some(override_origin)) = (&bound_page, &override_origin) {
        let mgr = state
            .browser
            .as_ref()
            .ok_or(AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR)?;
        validate_auth_login_active_page(mgr, bound_page, override_origin).await?;
    }
    let cred = if let Some(provider) = cmd.get("credentialProvider").and_then(|v| v.as_str()) {
        let command_plugins = cmd
            .get("plugins")
            .and_then(|v| {
                serde_json::from_value::<Vec<crate::plugins::PluginConfig>>(v.clone()).ok()
            })
            .unwrap_or_else(crate::plugins::plugins_from_env);
        let resolved = crate::plugins::resolve_credential_with_plugins(
            provider,
            &command_plugins,
            crate::plugins::CredentialResolveRequest {
                profile_name: name,
                item_ref: cmd.get("credentialItem").and_then(|v| v.as_str()),
                url: url_override,
            },
        )
        .await?;
        auth::AuthProfile {
            name: name.to_string(),
            url: url_override
                .map(String::from)
                .or(resolved.url)
                .unwrap_or_default(),
            username: resolved.username,
            password: resolved.password,
            username_selector: resolved.username_selector,
            password_selector: resolved.password_selector,
            submit_selector: resolved.submit_selector,
            created_at: None,
            last_login_at: None,
        }
    } else {
        let mut profile = auth::credentials_get_full(name)?;
        if let Some(url) = url_override {
            profile.url = url.to_string();
        }
        profile
    };
    if cred.url.is_empty() {
        return Err("Credential has no URL".to_string());
    }
    let auth::AuthProfile {
        url,
        username,
        password,
        username_selector: stored_username_selector,
        password_selector: stored_password_selector,
        submit_selector: stored_submit_selector,
        ..
    } = cred;

    // The login fills this password itself; a capture must not keep it.
    state.flow.add_secret(&password);
    let auth_timeout_ms = state.timeout_ms(cmd);
    let mgr = state.browser.as_mut().ok_or("Browser not launched")?;
    let expected_origin = if no_navigate {
        let origin = auth_login_origin(&url, "credential")?;
        validate_auth_login_active_page(
            mgr,
            bound_page
                .as_ref()
                .expect("no-navigation login must bind an active page"),
            &origin,
        )
        .await?;
        // Auth login always locates its form in the top-level document. Ignore
        // a prior `frame` selection for this command without mutating the
        // caller's persistent frame selection.
        crate::native::element::set_active_frame(None);
        Some(origin)
    } else {
        mgr.navigate(&url, AUTH_LOGIN_WAIT_UNTIL).await?;
        None
    };

    let session_id = mgr.active_session_id()?.to_string();

    let preferred_user_selectors = [
        "input[type=email]",
        "input[name=email]",
        "input[id=email]",
        "input[autocomplete=email]",
        "input[autocomplete=username]",
        "input[name=username]",
        "input[name*=email i]",
        "input[name*=user i]",
        "input[id*=email i]",
        "input[id*=user i]",
        "input[type=text][name*=email i]",
        "input[type=text][name*=user i]",
        "input[type=text][id*=email i]",
        "input[type=text][id*=user i]",
        "input[type=text][autocomplete=email]",
        "input[type=text][autocomplete=username]",
    ];
    let fallback_user_selectors = ["input[type=text]", "input:not([type])"];
    let auto_submit_selectors = [
        "button[type=submit]",
        "input[type=submit]",
        "button:not([type])",
    ];

    let username_sel = cmd
        .get("usernameSelector")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or(stored_username_selector);
    let password_sel = cmd
        .get("passwordSelector")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or(stored_password_selector);
    let submit_sel = cmd
        .get("submitSelector")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or(stored_submit_selector);

    // Find and fill username
    let user_element = if let Some(s) = username_sel {
        wait_for_any_selector(&mgr.client, &session_id, &[s.as_str()], auth_timeout_ms)
            .await
            .map_err(|_| format!("Timed out waiting for username selector '{}'", s))?
    } else {
        let preferred_window_ms = auth_timeout_ms.min(AUTH_LOGIN_PREFERRED_SELECTOR_WINDOW_MS);
        let fallback_window_ms = auth_timeout_ms.saturating_sub(preferred_window_ms);

        match wait_for_any_selector(
            &mgr.client,
            &session_id,
            &preferred_user_selectors,
            preferred_window_ms,
        )
        .await
        {
            Ok(selector) => selector,
            Err(_) => {
                if fallback_window_ms == 0 {
                    return Err(format!(
                        "Timed out waiting for username field (preferred selectors for {}ms: {})",
                        preferred_window_ms,
                        preferred_user_selectors.join(", ")
                    ));
                }

                wait_for_any_selector(
                    &mgr.client,
                    &session_id,
                    &fallback_user_selectors,
                    fallback_window_ms,
                )
                .await
                .map_err(|_| {
                    format!(
                        "Timed out waiting for username field (preferred selectors for {}ms: {}; fallback selectors for {}ms: {})",
                        preferred_window_ms,
                        preferred_user_selectors.join(", "),
                        fallback_window_ms,
                        fallback_user_selectors.join(", ")
                    )
                })?
            }
        }
    };
    if let (Some(bound_page), Some(expected_origin)) = (&bound_page, &expected_origin) {
        validate_auth_login_active_page(mgr, bound_page, expected_origin).await?;
    }
    user_element
        .fill(&mgr.client, &session_id, &username)
        .await?;

    // Find and fill password
    let pass_sel = password_sel.unwrap_or_else(|| "input[type=password]".to_string());
    let pass_element = wait_for_any_selector(
        &mgr.client,
        &session_id,
        &[pass_sel.as_str()],
        auth_timeout_ms,
    )
    .await
    .map_err(|_| format!("Timed out waiting for password selector '{}'", pass_sel))?;
    if let (Some(bound_page), Some(expected_origin)) = (&bound_page, &expected_origin) {
        validate_auth_login_active_page(mgr, bound_page, expected_origin).await?;
    }
    pass_element
        .fill(&mgr.client, &session_id, &password)
        .await?;

    // Find and click submit
    let submit_element = if let Some(s) = submit_sel {
        wait_for_any_selector(&mgr.client, &session_id, &[s.as_str()], auth_timeout_ms)
            .await
            .map_err(|_| format!("Timed out waiting for submit selector '{}'", s))?
    } else {
        wait_for_any_selector(
            &mgr.client,
            &session_id,
            &auto_submit_selectors,
            auth_timeout_ms,
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out waiting for submit button (tried selectors: {})",
                auto_submit_selectors.join(", ")
            )
        })?
    };
    if let (Some(bound_page), Some(expected_origin)) = (&bound_page, &expected_origin) {
        validate_auth_login_active_page(mgr, bound_page, expected_origin).await?;
    }
    let result = interaction::click(
        &mgr.client,
        &session_id,
        &submit_element.ref_map,
        AUTH_LOGIN_ELEMENT_REF,
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

    // Wait for navigation after submit (with fallback timeout)
    let mut rx = mgr.client.subscribe();
    let post_submit_timeout_ms = auth_timeout_ms.min(10_000);
    let deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(post_submit_timeout_ms);
    let mut navigated = false;

    loop {
        let result = tokio::time::timeout_at(deadline, rx.recv()).await;
        match result {
            Ok(Ok(event)) => {
                if event.session_id.as_deref() == Some(&session_id) {
                    match event.method.as_str() {
                        "Page.frameNavigated" | "Page.loadEventFired" => {
                            navigated = true;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }

    if !navigated {
        let fallback_sleep_ms = auth_timeout_ms.min(2_000);
        if fallback_sleep_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(fallback_sleep_ms)).await;
        }
    }

    Ok(json!({ "loggedIn": true, "name": name }))
}

// ---------------------------------------------------------------------------
// Confirmation handlers (stub)
// ---------------------------------------------------------------------------
