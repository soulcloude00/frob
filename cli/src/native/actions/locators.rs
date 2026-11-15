use super::*;

/// The exact set of `find` actions `execute_subaction` dispatches. Shared by
/// the validation guard, the error message, and the accepted-actions test,
/// so drift between the guard and the match arms fails a test instead of
/// silently reopening the "Unknown subaction: type" bug this fixes.
pub(crate) const FIND_ACTIONS: &[&str] = &["click", "fill", "check", "hover", "text"];

/// The daemon commands that dispatch a `find` subaction through
/// `execute_subaction` after resolving their locator.
pub(super) const FIND_SUBACTION_COMMANDS: &[&str] = &[
    "getbyrole",
    "getbytext",
    "getbylabel",
    "getbyplaceholder",
    "getbyalttext",
    "getbytitle",
    "getbytestid",
    "nth",
];

/// Reject an unsupported `find` action before any browser launch or locator
/// resolution. The guard inside `execute_subaction` only runs after both, so
/// on a missing element it never runs at all: the caller fails first with an
/// "element not found" error that names the wrong fault, after paying for a
/// browser launch to say it. Validation order follows the input, not the
/// setup cost.
pub(super) fn validate_find_subaction(action: &str, cmd: &Value) -> Result<(), String> {
    if !FIND_SUBACTION_COMMANDS.contains(&action) {
        return Ok(());
    }
    let subaction = cmd
        .get("subaction")
        .and_then(|v| v.as_str())
        .unwrap_or("click");
    if !FIND_ACTIONS.contains(&subaction) {
        return Err(format!(
            "Unknown action '{}' for find. Valid actions: {}.",
            subaction,
            FIND_ACTIONS.join(", ")
        ));
    }
    Ok(())
}

pub(super) async fn execute_subaction(
    cmd: &Value,
    state: &mut DaemonState,
    selector: &str,
) -> Result<Value, String> {
    let subaction = cmd
        .get("subaction")
        .and_then(|v| v.as_str())
        .unwrap_or("click");

    // Validate before dispatching: an unsupported action (e.g. "type",
    // "focus", "uncheck", all real standalone commands, just not `find`
    // actions) gets a message naming the valid set here.
    if !FIND_ACTIONS.contains(&subaction) {
        return Err(format!(
            "Unknown action '{}' for find. Valid actions: {}.",
            subaction,
            FIND_ACTIONS.join(", ")
        ));
    }

    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();

    match subaction {
        "click" => {
            let result = interaction::click(
                &mgr.client,
                &session_id,
                &state.ref_map,
                selector,
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
            if result.dialog_opened {
                state.pending_pointer_release = result.pending_release;
                return Ok(json!({ "clicked": selector, "dialogOpened": true }));
            }
            Ok(json!({ "clicked": selector }))
        }
        "fill" => {
            let value = cmd
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'value' for fill subaction")?;
            interaction::fill(
                &mgr.client,
                &session_id,
                &state.ref_map,
                selector,
                value,
                &state.iframe_sessions,
            )
            .await?;
            Ok(json!({ "filled": selector }))
        }
        "check" => {
            if let Some(position) = interaction::check(
                &mgr.client,
                &session_id,
                &state.ref_map,
                selector,
                &state.iframe_sessions,
            )
            .await?
            {
                (state.mouse_state.x, state.mouse_state.y) = position;
                if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
                    cursor.record(position.0, position.1, state.mouse_state.buttons);
                }
            }
            Ok(json!({ "checked": selector }))
        }
        "hover" => {
            let position = interaction::hover(
                &mgr.client,
                &session_id,
                &state.ref_map,
                selector,
                &state.iframe_sessions,
            )
            .await?;
            (state.mouse_state.x, state.mouse_state.y) = position;
            if let Ok(mut cursor) = state.recording_state.shared_cursor.lock() {
                cursor.record(position.0, position.1, state.mouse_state.buttons);
            }
            Ok(json!({ "hovered": selector }))
        }
        "text" => {
            let text = crate::native::element::get_element_text(
                &mgr.client,
                &session_id,
                &state.ref_map,
                selector,
                &state.iframe_sessions,
            )
            .await?;
            Ok(json!({ "text": text }))
        }
        // Unreachable today; a real Err rather than unreachable!() in case
        // this ever drifts out of sync with the guard above.
        _ => Err(format!(
            "Internal error: action '{}' passed validation but has no handler.",
            subaction
        )),
    }
}

pub(super) fn build_role_selector(role: &str, name: Option<&str>, exact: bool) -> String {
    match name {
        Some(n) => {
            let exact_str = if exact { ", exact: true" } else { "" };
            format!("getByRole('{}', {{ name: '{}'{} }})", role, n, exact_str)
        }
        None => format!("getByRole('{}')", role),
    }
}

/// role="none" and role="presentation" (ARIA synonyms) exist to strip an
/// element's semantics, so Chrome prunes such elements from the AX tree
/// entirely (divs, lists, imgs) or keeps them only as ignored nodes
/// (tables). The accessibility tree can never answer a query for them.
pub(super) fn is_presentational_role(role: &str) -> bool {
    role.eq_ignore_ascii_case("none") || role.eq_ignore_ascii_case("presentation")
}

/// Match presentational roles against the explicit `role` attribute in the
/// DOM, the only place the author's intent survives (see
/// `is_presentational_role`). ARIA treats the role attribute as an ordered
/// fallback list whose first supported token is the operative role, so
/// `role="button none"` is a button and must not answer a query for "none",
/// while `role="none button"` must. Matching is literal per synonym: a query
/// for "none" only matches an operative "none", mirroring Playwright's
/// literal role comparison. Name matching approximates accessible-name
/// precedence (aria-labelledby, then aria-label, then text content) with
/// whitespace normalized; non-exact is a case-insensitive substring check,
/// exact is a case-sensitive whole-name comparison.
pub(super) async fn handle_presentational_getbyrole(
    cmd: &Value,
    state: &mut DaemonState,
    role: &str,
    name: Option<&str>,
    exact: bool,
) -> Result<Value, String> {
    let name_check = match name {
        Some(n) => {
            let name_json = serde_json::to_string(n).unwrap_or_default();
            if exact {
                format!("if (norm(nameOf(el)) !== norm({name_json})) continue;")
            } else {
                format!(
                    "if (!norm(nameOf(el)).toLowerCase().includes(norm({name_json}).toLowerCase())) continue;"
                )
            }
        }
        None => String::new(),
    };

    let role_json = serde_json::to_string(&role.to_ascii_lowercase()).unwrap_or_default();
    // ARIA roles Playwright recognizes (WAI-ARIA 1.2). The operative role is the
    // first token that is a defined role, so omitting one (e.g. `mark`) lets a
    // later presentational token wrongly win.
    // A function of `root` (the document to search), so it can run against the
    // top document or a selected frame's document, not just the global one.
    let locate_body = format!(
        r#"(root) => {{
            const VALID_ROLES = new Set(['alert','alertdialog','application','article','banner','blockquote','button','caption','cell','checkbox','code','columnheader','combobox','complementary','contentinfo','definition','deletion','dialog','directory','document','emphasis','feed','figure','form','generic','grid','gridcell','group','heading','img','insertion','link','list','listbox','listitem','log','main','mark','marquee','math','meter','menu','menubar','menuitem','menuitemcheckbox','menuitemradio','navigation','none','note','option','paragraph','presentation','progressbar','radio','radiogroup','region','row','rowgroup','rowheader','scrollbar','search','searchbox','separator','slider','spinbutton','status','strong','subscript','superscript','switch','tab','table','tablist','tabpanel','term','textbox','time','timer','toolbar','tooltip','tree','treegrid','treeitem']);
            const norm = s => (s || '').replace(/\s+/g, ' ').trim();
            const nameOf = el => {{
                const lb = el.getAttribute('aria-labelledby');
                if (lb) {{
                    const t = lb.trim().split(/\s+/)
                        .map(id => {{ const r = root.getElementById(id); return r ? r.textContent : ''; }})
                        .join(' ');
                    if (norm(t)) return t;
                }}
                const al = el.getAttribute('aria-label');
                if (al !== null && norm(al)) return al;
                return el.textContent || '';
            }};
            const role = {role_json};
            const isPresentational = role === 'none' || role === 'presentation';
            // Global ARIA states/properties (WAI-ARIA 1.2); their presence, not any
            // aria-* attribute, triggers presentational-roles conflict resolution.
            const GLOBAL_ARIA = new Set(['aria-atomic','aria-busy','aria-controls','aria-current','aria-describedby','aria-description','aria-details','aria-disabled','aria-dropeffect','aria-errormessage','aria-flowto','aria-grabbed','aria-haspopup','aria-hidden','aria-invalid','aria-keyshortcuts','aria-label','aria-labelledby','aria-live','aria-owns','aria-relevant','aria-roledescription']);
            const isFocusable = el => el.tabIndex >= 0 || el.hasAttribute('tabindex');
            for (const el of root.querySelectorAll('[role]')) {{
                const tokens = (el.getAttribute('role') || '').trim().toLowerCase().split(/\s+/);
                const operative = tokens.find(t => VALID_ROLES.has(t));
                if (operative !== role) continue;
                // ARIA presentational-roles conflict resolution: none/presentation
                // is ignored on a focusable element or one carrying global ARIA
                // states/properties, so it keeps its implicit role and must not
                // answer a query for none/presentation.
                if (isPresentational && (isFocusable(el) || el.getAttributeNames().some(a => GLOBAL_ARIA.has(a)))) continue;
                {name_check}
                el.setAttribute('data-frob-located', 'true');
                return true;
            }}
            return false;
        }}"#
    );

    let located = {
        let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
        let top_session = mgr.active_session_id()?.to_string();
        eval_body_in_active_frame(
            mgr,
            state.active_frame_id.as_deref(),
            &top_session,
            &state.iframe_sessions,
            &locate_body,
        )
        .await?
    };

    if !located.as_bool().unwrap_or(false) {
        return Err(format!(
            "No element found: {}",
            build_role_selector(role, name, exact)
        ));
    }

    let selector = "[data-frob-located='true']";
    let action_result = execute_subaction(cmd, state, selector).await;

    // Clean up the marker in whichever document it was set in.
    if let Some(mgr) = state.browser.as_ref() {
        if let Ok(top_session) = mgr.active_session_id() {
            let top_session = top_session.to_string();
            let _ = eval_body_in_active_frame(
                mgr,
                state.active_frame_id.as_deref(),
                &top_session,
                &state.iframe_sessions,
                "(root) => { root.querySelector('[data-frob-located]')?.removeAttribute('data-frob-located'); }",
            )
            .await;
        }
    }

    action_result
}

/// Evaluate a `(root) => {...}` body against the active frame's document, or the
/// top document when no frame is selected. Runtime.evaluate cannot target a
/// same-origin child frame, so that case runs the body against the frame owner's
/// contentDocument (as element resolution does); an OOPIF has its own session
/// where `document` is already the frame document.
pub(super) async fn eval_body_in_active_frame(
    mgr: &BrowserManager,
    frame_id: Option<&str>,
    top_session: &str,
    iframe_sessions: &HashMap<String, String>,
    body: &str,
) -> Result<Value, String> {
    match frame_id {
        Some(fid) if !iframe_sessions.contains_key(fid) => {
            let owner =
                crate::native::element::frame_owner_object_id(&mgr.client, top_session, fid)
                    .await?;
            let func = format!(
                "function() {{ const d = this.contentDocument; if (!d) return null; return ({body})(d); }}"
            );
            let res = mgr
                .client
                .send_command(
                    "Runtime.callFunctionOn",
                    Some(serde_json::json!({
                        "objectId": owner,
                        "functionDeclaration": func,
                        "returnByValue": true,
                    })),
                    Some(top_session),
                )
                .await?;
            Ok(res
                .get("result")
                .and_then(|r| r.get("value"))
                .cloned()
                .unwrap_or(Value::Null))
        }
        _ => {
            let session = frame_id
                .and_then(|f| iframe_sessions.get(f))
                .map(|s| s.as_str())
                .unwrap_or(top_session);
            let res: crate::native::cdp::types::EvaluateResult = mgr
                .client
                .send_command_typed(
                    "Runtime.evaluate",
                    &crate::native::cdp::types::EvaluateParams {
                        expression: format!("({body})(document)"),
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(session),
                )
                .await?;
            Ok(res.result.value.unwrap_or(Value::Null))
        }
    }
}

pub(super) async fn handle_getbyrole(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let role = cmd
        .get("role")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'role' parameter")?;
    let name = cmd.get("name").and_then(|v| v.as_str());
    let exact = cmd.get("exact").and_then(|v| v.as_bool()).unwrap_or(false);

    // The AX tree cannot answer these, so they fall back to a DOM match on the
    // explicit `role` attribute: none/presentation are pruned from the tree, and
    // Chrome collapses `directory` into `list` (only the attribute tells them apart).
    if is_presentational_role(role) || role.eq_ignore_ascii_case("directory") {
        return handle_presentational_getbyrole(cmd, state, role, name, exact).await;
    }

    // Query the accessibility tree via CDP: the browser engine is the
    // authoritative source for implicit roles (e.g. <h2> -> "heading",
    // <a href> -> "link"), which a CSS selector cannot approximate.
    let (ax_params, effective_session_id) = crate::native::element::resolve_ax_session(
        state.active_frame_id.as_deref(),
        &session_id,
        &state.iframe_sessions,
    );

    let ax_tree: GetFullAXTreeResult = mgr
        .client
        .send_command_typed_raw(
            "Accessibility.getFullAXTree",
            &ax_params,
            Some(effective_session_id),
        )
        .await?;

    let (backend_node_id, actual_name) = find_ax_node_by_role(&ax_tree.nodes, role, name, exact)?;

    let ref_num = state.ref_map.next_ref_num();
    let temp_ref = format!("e{}", ref_num);
    state.ref_map.add_with_frame(
        temp_ref.clone(),
        Some(backend_node_id),
        role,
        &actual_name,
        None,
        state.active_frame_id.as_deref(),
    );
    state.ref_map.set_next_ref_num(ref_num + 1);

    let result = execute_subaction(cmd, state, &format!("@{}", temp_ref)).await;
    state.ref_map.remove(&temp_ref);
    result
}

/// Map a Chrome AX tree role to its ARIA name. Divergences: `image` -> `img`
/// and `RootWebArea` -> `document` (Chrome's names for what ARIA/Playwright call
/// `img` and `document`); queries pass through the same table so both spellings
/// match. `directory` is not mapped here (Chrome collapses it into `list`); it is
/// matched on the DOM attribute instead, see handle_getbyrole.
pub(super) fn normalize_ax_role(role: &str) -> String {
    let lower = role.to_ascii_lowercase();
    match lower.as_str() {
        "image" => "img".to_string(),
        "rootwebarea" => "document".to_string(),
        _ => lower,
    }
}

/// Collapse internal whitespace runs to single spaces and trim, mirroring
/// Playwright's accessible-name normalization: `"Save   changes"` and
/// `"Save changes"` are the same accessible name.
pub(super) fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Match a role and (optional) accessible name against the AX tree, mirroring
/// Playwright's getByRole semantics: role values are compared case-insensitively
/// after AX-to-ARIA normalization; accessible names are whitespace-normalized on
/// both sides, then non-exact matching is a case-insensitive substring check and
/// exact matching is a case-sensitive whole-name comparison. Continues past AX
/// nodes without a backendDOMNodeId (virtual nodes) so a later actionable match
/// still wins.
pub(super) fn find_ax_node_by_role(
    nodes: &[crate::native::cdp::types::AXNode],
    role: &str,
    name: Option<&str>,
    exact: bool,
) -> Result<(i64, String), String> {
    let mut matched_without_backend_id = false;
    // `names_seen` keeps first-seen order for the error message; `names_set` makes
    // the dedup check O(1) so a failed name query is linear, not quadratic, in the
    // number of role matches.
    let mut names_seen: Vec<String> = Vec::new();
    let mut names_set: HashSet<String> = HashSet::new();
    let mut role_match_count: usize = 0;
    let target_role = normalize_ax_role(role);

    for node in nodes {
        if node.ignored.unwrap_or(false) {
            continue;
        }

        let node_role = crate::native::element::extract_ax_string(&node.role);
        if normalize_ax_role(&node_role) != target_role {
            continue;
        }

        let node_name = crate::native::element::extract_ax_string(&node.name);
        let matches = match name {
            Some(target_name) if exact => normalize_ws(&node_name) == normalize_ws(target_name),
            Some(target_name) => normalize_ws(&node_name)
                .to_lowercase()
                .contains(&normalize_ws(target_name).to_lowercase()),
            None => true,
        };

        if !matches {
            if name.is_some() {
                role_match_count += 1;
                if names_set.insert(node_name.clone()) {
                    names_seen.push(node_name);
                }
            }
            continue;
        }

        if let Some(id) = node.backend_d_o_m_node_id {
            return Ok((id, node_name));
        }

        matched_without_backend_id = true;
    }

    if matched_without_backend_id {
        return Err(match name {
            Some(target_name) => format!(
                "Found role \"{}\" matching name \"{}\" in the accessibility tree, but it has no live DOM element to act on.",
                role, target_name
            ),
            None => format!(
                "Found role \"{}\" in the accessibility tree, but it has no live DOM element to act on.",
                role
            ),
        });
    }

    // A role match with no name match is more actionable than a blanket
    // "not found": show what the query actually saw, so an agent can fix a
    // typo'd name without a blind retry.
    if let Some(target_name) = name {
        if !names_seen.is_empty() {
            let shown: Vec<String> = names_seen
                .iter()
                .take(5)
                .map(|n| format!("\"{}\"", n))
                .collect();
            let more = if names_seen.len() > 5 { ", ..." } else { "" };
            let (plural, verb) = if role_match_count == 1 {
                ("", "has")
            } else {
                ("s", "have")
            };
            return Err(format!(
                "{count} element{plural} {verb} role \"{role}\", but none match name \"{target_name}\". Names seen: {shown}{more}",
                count = role_match_count,
                plural = plural,
                verb = verb,
                role = role,
                target_name = target_name,
                shown = shown.join(", "),
                more = more,
            ));
        }
    }

    let desc = build_role_selector(role, name, exact);
    Err(format!("No element found: {}", desc))
}

pub(super) async fn handle_semantic_locator(
    cmd: &Value,
    state: &mut DaemonState,
    strategy: &str,
    param_name: &str,
) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let value = cmd
        .get(param_name)
        .and_then(|v| v.as_str())
        .ok_or(format!("Missing '{}' parameter", param_name))?;
    let exact = cmd.get("exact").and_then(|v| v.as_bool()).unwrap_or(false);

    let match_fn = if exact {
        format!(
            "el.textContent.trim() === {}",
            serde_json::to_string(value).unwrap_or_default()
        )
    } else {
        format!(
            "el.textContent.includes({})",
            serde_json::to_string(value).unwrap_or_default()
        )
    };

    let query = match strategy {
        // Like Playwright's getByLabel: match <label> associations AND
        // aria-label / aria-labelledby. Icon buttons and custom controls
        // are usually labelled via aria-label only.
        "label" => {
            let value_json = serde_json::to_string(value).unwrap_or_default();
            let matches_fn = if exact {
                format!("(s) => !!s && s.trim() === {value_json}")
            } else {
                format!("(s) => !!s && s.includes({value_json})")
            };
            format!(
                r#"(() => {{
                const matches = {matches_fn};
                const label = Array.from(document.querySelectorAll('label')).find(el => matches(el.textContent));
                if (label) {{
                    const forId = label.getAttribute('for');
                    const target = forId ? document.getElementById(forId) : label.querySelector('input,select,textarea');
                    if (target) {{ target.setAttribute('data-frob-located', 'true'); return true; }}
                }}
                const aria = Array.from(document.querySelectorAll('[aria-label]')).find(el => matches(el.getAttribute('aria-label')));
                if (aria) {{ aria.setAttribute('data-frob-located', 'true'); return true; }}
                const referenced = Array.from(document.querySelectorAll('[aria-labelledby]')).find(el => {{
                    const text = el.getAttribute('aria-labelledby').split(/\s+/)
                        .map(id => {{ const r = document.getElementById(id); return r ? r.textContent : ''; }})
                        .join(' ');
                    return matches(text);
                }});
                if (referenced) {{ referenced.setAttribute('data-frob-located', 'true'); return true; }}
                return false;
            }})()"#,
            )
        }
        "placeholder" => format!(
            r#"(() => {{
                const el = document.querySelector('input[placeholder={val}], textarea[placeholder={val}]');
                if (el) {{ el.setAttribute('data-frob-located', 'true'); return true; }}
                return false;
            }})()"#,
            val = serde_json::to_string(value).unwrap_or_default(),
        ),
        "alttext" => format!(
            r#"(() => {{
                const el = document.querySelector('img[alt={val}], [alt={val}]');
                if (el) {{ el.setAttribute('data-frob-located', 'true'); return true; }}
                return false;
            }})()"#,
            val = serde_json::to_string(value).unwrap_or_default(),
        ),
        "title" => format!(
            r#"(() => {{
                const el = document.querySelector('[title={val}]');
                if (el) {{ el.setAttribute('data-frob-located', 'true'); return true; }}
                return false;
            }})()"#,
            val = serde_json::to_string(value).unwrap_or_default(),
        ),
        "testid" => format!(
            r#"(() => {{
                const el = document.querySelector('[data-testid={val}]');
                if (el) {{ el.setAttribute('data-frob-located', 'true'); return true; }}
                return false;
            }})()"#,
            val = serde_json::to_string(value).unwrap_or_default(),
        ),
        _ => {
            // "text" strategy
            format!(
                r#"(() => {{
                    const all = document.querySelectorAll('*');
                    for (const el of all) {{
                        if (el.children.length === 0 && {match_fn}) {{
                            el.setAttribute('data-frob-located', 'true');
                            return true;
                        }}
                    }}
                    return false;
                }})()"#,
                match_fn = match_fn,
            )
        }
    };

    let result: crate::native::cdp::types::EvaluateResult = mgr
        .client
        .send_command_typed(
            "Runtime.evaluate",
            &crate::native::cdp::types::EvaluateParams {
                expression: query,
                return_by_value: Some(true),
                await_promise: Some(false),
            },
            Some(&session_id),
        )
        .await?;

    if !result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(format!("No element found by {} '{}'", strategy, value));
    }

    let selector = "[data-frob-located='true']";
    let action_result = execute_subaction(cmd, state, selector).await;

    if let Some(ref browser) = state.browser {
        let _ = browser
            .evaluate(
                "document.querySelector('[data-frob-located]')?.removeAttribute('data-frob-located')",
                None,
            )
            .await;
    }

    action_result
}

pub(super) async fn handle_getbytext(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    handle_semantic_locator(cmd, state, "text", "text").await
}

pub(super) async fn handle_getbylabel(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    handle_semantic_locator(cmd, state, "label", "label").await
}

pub(super) async fn handle_getbyplaceholder(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    handle_semantic_locator(cmd, state, "placeholder", "placeholder").await
}

pub(super) async fn handle_getbyalttext(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    handle_semantic_locator(cmd, state, "alttext", "text").await
}

pub(super) async fn handle_getbytitle(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    handle_semantic_locator(cmd, state, "title", "text").await
}

pub(super) async fn handle_getbytestid(
    cmd: &Value,
    state: &mut DaemonState,
) -> Result<Value, String> {
    handle_semantic_locator(cmd, state, "testid", "testId").await
}

pub(super) async fn handle_nth(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;
    let index = cmd
        .get("index")
        .and_then(|v| v.as_i64())
        .ok_or("Missing 'index' parameter")?;

    let js = format!(
        r#"(() => {{
            const els = document.querySelectorAll({sel});
            const idx = {idx} < 0 ? els.length + {idx} : {idx};
            if (idx < 0 || idx >= els.length) return false;
            els[idx].setAttribute('data-frob-located', 'true');
            return true;
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default(),
        idx = index,
    );

    let result: crate::native::cdp::types::EvaluateResult = mgr
        .client
        .send_command_typed(
            "Runtime.evaluate",
            &crate::native::cdp::types::EvaluateParams {
                expression: js,
                return_by_value: Some(true),
                await_promise: Some(false),
            },
            Some(&session_id),
        )
        .await?;

    if !result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(format!(
            "No element at index {} for selector '{}'",
            index, selector
        ));
    }

    let located = "[data-frob-located='true']";
    let action_result = execute_subaction(cmd, state, located).await;

    if let Some(ref browser) = state.browser {
        let _ = browser
            .evaluate(
                "document.querySelector('[data-frob-located]')?.removeAttribute('data-frob-located')",
                None,
            )
            .await;
    }

    action_result
}

pub(super) async fn handle_find(cmd: &Value, state: &DaemonState) -> Result<Value, String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let selector = cmd
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'selector' parameter")?;

    let js = format!(
        r#"(() => {{
            const els = document.querySelectorAll({});
            return Array.from(els).map((el, i) => ({{
                index: i,
                tagName: el.tagName.toLowerCase(),
                text: el.textContent?.trim().substring(0, 100) || '',
                visible: el.offsetWidth > 0 && el.offsetHeight > 0,
            }}));
        }})()"#,
        serde_json::to_string(selector).unwrap_or_default()
    );

    let result = mgr.evaluate(&js, None).await?;
    Ok(json!({ "elements": result, "selector": selector }))
}
