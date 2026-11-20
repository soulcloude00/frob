//! Observe after an action: what the action did to the page, returned with the action itself.
//!
//! Without this an agent clicks, gets `{"clicked": "@e3"}`, and has to snapshot again to learn
//! whether anything happened. With `--observe` (or `FROB_OBSERVE=1`, or `"observe": true` on the
//! command) every page-changing action waits for the page to settle and reports:
//!
//! - `changed` and `changes`: the interactive snapshot lines that appeared (`+`) or went (`-`),
//!   with their fresh refs, so the agent can act on new elements without another snapshot;
//! - `navigated`: the old and new URL when the action left the page, plus the new page's
//!   interactive snapshot in `snapshot`;
//! - `console` / `pageErrors`: console warnings and errors and uncaught exceptions the action
//!   caused.
//!
//! The refs map is replaced by the settled snapshot's, so refs in the observation are usable.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use similar::{capture_diff_slices, Algorithm, DiffOp};

use super::actions::DaemonState;
use super::element::RefMap;
use super::{diff, snapshot};

/// Actions that can change the page.
const OBSERVED: &[&str] = &[
    "navigate",
    "back",
    "forward",
    "reload",
    "click",
    "dblclick",
    "tap",
    "fill",
    "type",
    "inserttext",
    "press",
    "keyboard",
    "keydown",
    "keyup",
    "hover",
    "focus",
    "clear",
    "selectall",
    "scroll",
    "scrollintoview",
    "wheel",
    "select",
    "multiselect",
    "check",
    "uncheck",
    "setvalue",
    "upload",
    "drag",
    "mouse",
    "mousedown",
    "mouseup",
    "swipe",
    "dispatch",
    "find",
    "nth",
];
/// Actions after which the previous page is gone, so a diff against it means nothing.
const NAVIGATING: &[&str] = &["navigate", "back", "forward", "reload"];

/// Longest we wait for the page to stop changing after the action.
const SETTLE_CAP: Duration = Duration::from_millis(2000);
/// The page counts as settled once nothing changed in it for this long, with no request or short
/// timer the action started still pending.
const QUIET_MS: f64 = 100.0;
const POLL: Duration = Duration::from_millis(20);

/// Installed in the page before the action. It records when the DOM last changed, and counts the
/// fetch/XHR requests and short timers (up to 1.5 s, two generations deep) started while it is
/// armed, so a click whose result arrives from the network or after a delay is waited for, and a
/// click that does nothing returns after one quiet period. Unarmed, the wrappers pass straight
/// through. Idempotent per document.
const ARM_JS: &str = r#"(() => {
  const w = window;
  if (!w.__frobSettle) {
    const t = { last: performance.now(), inflight: 0, timers: 0, armed: false, depth: 0 };
    const touch = () => { t.last = performance.now(); };
    new MutationObserver(touch).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    const ofetch = w.fetch;
    if (ofetch) {
      w.fetch = function (...args) {
        if (!t.armed) return ofetch.apply(this, args);
        t.inflight++;
        let p;
        try { p = ofetch.apply(this, args); } catch (e) { t.inflight--; throw e; }
        return p.finally(() => { t.inflight--; touch(); });
      };
    }
    const XHR = w.XMLHttpRequest && w.XMLHttpRequest.prototype;
    if (XHR) {
      const osend = XHR.send;
      XHR.send = function (...args) {
        if (t.armed) {
          t.inflight++;
          this.addEventListener('loadend', () => { t.inflight--; touch(); }, { once: true });
        }
        return osend.apply(this, args);
      };
    }
    const oset = w.setTimeout, oclear = w.clearTimeout, pending = new Set();
    w.setTimeout = function (fn, ms, ...rest) {
      if (!t.armed || t.depth > 1 || typeof fn !== 'function' || (Number(ms) || 0) > 1500) {
        return oset.call(this, fn, ms, ...rest);
      }
      const depth = t.depth + 1;
      t.timers++;
      const id = oset.call(this, function (...a) {
        if (pending.delete(id)) t.timers--;
        const prev = t.depth; t.depth = depth;
        try { return fn.apply(this, a); } finally { t.depth = prev; touch(); }
      }, ms, ...rest);
      pending.add(id);
      return id;
    };
    w.clearTimeout = function (id) {
      if (pending.delete(id)) t.timers--;
      return oclear.call(this, id);
    };
    w.__frobSettle = t;
  }
  w.__frobSettle.armed = true;
  w.__frobSettle.last = performance.now();
  return true;
})()"#;

const STATUS_JS: &str = r#"(() => {
  const t = window.__frobSettle;
  if (!t) return { fresh: true, ready: document.readyState };
  return { quiet: performance.now() - t.last, inflight: t.inflight, timers: t.timers, ready: document.readyState };
})()"#;

const DISARM_JS: &str = "window.__frobSettle && (window.__frobSettle.armed = false)";
const MAX_CHANGE_LINES: usize = 80;
const MAX_SNAPSHOT_LINES: usize = 300;
const MAX_EVENTS: usize = 10;
const MAX_EVENT_TEXT: usize = 300;

pub struct Before {
    url: String,
    tree: Option<String>,
    console_total: u64,
    error_total: u64,
}

/// Whether this command asked to be observed. An explicit `observe` on the command wins over the
/// daemon's `FROB_OBSERVE` default.
pub fn wanted(cmd: &Value, action: &str) -> bool {
    if !OBSERVED.contains(&action) {
        return false;
    }
    match cmd.get("observe").and_then(Value::as_bool) {
        Some(on) => on,
        None => std::env::var("FROB_OBSERVE")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false),
    }
}

/// Records the page before the action runs. Never fails: anything it cannot read is left out.
pub async fn before(state: &mut DaemonState, action: &str) -> Before {
    let _ = state.drain_cdp_events_background().await;
    let url = match state.browser.as_ref() {
        Some(mgr) => mgr.get_url().await.unwrap_or_default(),
        None => String::new(),
    };
    let tree = if NAVIGATING.contains(&action) {
        None
    } else {
        snap(state).await.ok().map(|(tree, _)| tree)
    };
    if let Some(mgr) = state.browser.as_ref() {
        let _ = mgr.evaluate(ARM_JS, None).await;
    }
    Before {
        url,
        tree,
        console_total: state.event_tracker.console_total,
        error_total: state.event_tracker.error_total,
    }
}

/// Waits for the page to settle after the action and describes what changed.
pub async fn after(state: &mut DaemonState, before: Before) -> Value {
    let start = Instant::now();
    settle(state, start, SETTLE_CAP).await;
    let last = snap(state).await.ok();
    if let Some(mgr) = state.browser.as_ref() {
        let _ = mgr.evaluate(DISARM_JS, None).await;
    }
    let _ = state.drain_cdp_events_background().await;

    let mut obs = json!({ "settleMs": start.elapsed().as_millis() as u64 });
    let (url, title) = match state.browser.as_ref() {
        Some(mgr) => (
            mgr.get_url().await.unwrap_or_default(),
            mgr.get_title().await.unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    };
    let navigated = !before.url.is_empty()
        && !url.is_empty()
        && strip_fragment(&url) != strip_fragment(&before.url);
    if navigated {
        obs["navigated"] = json!({ "from": before.url, "to": url });
    }
    obs["url"] = json!(url);
    obs["title"] = json!(title);

    if let Some((tree, refs)) = last {
        state.ref_map = refs;
        match before.tree {
            Some(old) if !navigated => {
                let (changes, added, removed) = line_diff(&old, &tree);
                obs["changed"] = json!(added + removed > 0);
                obs["added"] = json!(added);
                obs["removed"] = json!(removed);
                if !changes.is_empty() {
                    obs["changes"] = json!(changes);
                }
            }
            _ => {
                obs["changed"] = json!(true);
                obs["snapshot"] = json!(cap_lines(&tree, MAX_SNAPSHOT_LINES));
            }
        }
    }

    let console: Vec<Value> = state
        .event_tracker
        .console_since(before.console_total)
        .iter()
        .filter(|e| matches!(e.level.as_str(), "error" | "warning" | "warn" | "assert"))
        .take(MAX_EVENTS)
        .map(|e| json!({ "type": e.level, "text": cut(&e.text, MAX_EVENT_TEXT) }))
        .collect();
    if !console.is_empty() {
        obs["console"] = json!(console);
    }
    let errors: Vec<Value> = state
        .event_tracker
        .errors_since(before.error_total)
        .iter()
        .take(MAX_EVENTS)
        .map(|e| json!({ "text": cut(&e.text, MAX_EVENT_TEXT), "url": e.url, "line": e.line }))
        .collect();
    if !errors.is_empty() {
        obs["pageErrors"] = json!(errors);
    }
    obs
}

/// Actions that settle by default, without `--observe`: the ones whose effect commonly arrives
/// after they return (a fetch, a debounced handler, a render), so a snapshot chained right after
/// them would otherwise show the page mid-update and cost the agent another look.
const SETTLED: &[&str] = &[
    "click",
    "dblclick",
    "tap",
    "fill",
    "type",
    "inserttext",
    "press",
    "keyboard",
    "select",
    "multiselect",
    "check",
    "uncheck",
    "scroll",
    "scrollintoview",
    "upload",
    "drag",
    "find",
    "nth",
];
/// A plain action waits at most this long: a page that never stops changing costs little.
const DEFAULT_SETTLE_CAP: Duration = Duration::from_millis(1000);

/// Whether an unobserved action waits for the page to settle before returning. On by default;
/// `FROB_SETTLE=0` or `"settle": false` on the command turns it off.
pub fn settle_wanted(cmd: &Value, action: &str) -> bool {
    if !SETTLED.contains(&action) {
        return false;
    }
    match cmd.get("settle").and_then(Value::as_bool) {
        Some(on) => on,
        None => !std::env::var("FROB_SETTLE")
            .is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "no" | "off")),
    }
}

/// Starts tracking the page before an action that will settle.
pub async fn arm(state: &DaemonState) {
    if let Some(mgr) = state.browser.as_ref() {
        let _ = mgr.evaluate(ARM_JS, None).await;
    }
}

/// Waits for the page to settle after an action armed with [`arm`], then stops tracking.
pub async fn settle_after(state: &DaemonState) {
    settle(state, Instant::now(), DEFAULT_SETTLE_CAP).await;
    if let Some(mgr) = state.browser.as_ref() {
        let _ = mgr.evaluate(DISARM_JS, None).await;
    }
}

/// Waits until the page is quiet: no DOM change for `QUIET_MS` and nothing the action started
/// still pending, or until `cap`. A new document (the action navigated) gets the tracker once it
/// is parsed, then the same rule.
async fn settle(state: &DaemonState, start: Instant, cap: Duration) {
    let Some(mgr) = state.browser.as_ref() else {
        return;
    };
    while start.elapsed() < cap {
        tokio::time::sleep(POLL).await;
        let Ok(status) = mgr.evaluate(STATUS_JS, None).await else {
            continue; // the old document is going away
        };
        let ready = status
            .get("ready")
            .and_then(Value::as_str)
            .unwrap_or("loading");
        if status.get("fresh").is_some() {
            if ready != "loading" {
                let _ = mgr.evaluate(ARM_JS, None).await;
            }
            continue;
        }
        let quiet = status.get("quiet").and_then(Value::as_f64).unwrap_or(0.0);
        let inflight = status.get("inflight").and_then(Value::as_i64).unwrap_or(0);
        let timers = status.get("timers").and_then(Value::as_i64).unwrap_or(0);
        if ready != "loading" && inflight <= 0 && timers <= 0 && quiet >= QUIET_MS {
            return;
        }
    }
}

/// An interactive snapshot of the active page, with the refs it assigned. The live refs map is
/// left alone, so refs the agent holds stay valid while we look.
async fn snap(state: &DaemonState) -> Result<(String, RefMap), String> {
    let mgr = state.browser.as_ref().ok_or("Browser not launched")?;
    let session_id = mgr.active_session_id()?.to_string();
    let options = snapshot::SnapshotOptions {
        interactive: true,
        ..Default::default()
    };
    let mut refs = state.ref_map.clone();
    refs.begin_snapshot();
    let tree = snapshot::take_snapshot(
        &mgr.client,
        &session_id,
        &options,
        &mut refs,
        state.active_frame_id.as_deref(),
        &state.iframe_sessions,
    )
    .await?;
    Ok((tree, refs))
}

/// A snapshot with refs taken out, for comparing two looks at the same page.
fn comparable(tree: &str) -> String {
    diff::snapshot_comparison_text(tree)
}

/// Lines that went (`- `) and appeared (`+ `), compared without refs but shown with them.
fn line_diff(old: &str, new: &str) -> (Vec<String>, usize, usize) {
    let old_raw: Vec<&str> = old.lines().collect();
    let new_raw: Vec<&str> = new.lines().collect();
    let old_cmp: Vec<String> = old_raw.iter().map(|l| comparable(l)).collect();
    let new_cmp: Vec<String> = new_raw.iter().map(|l| comparable(l)).collect();
    let (mut out, mut added, mut removed) = (Vec::new(), 0, 0);
    for op in capture_diff_slices(Algorithm::Myers, &old_cmp, &new_cmp) {
        let (gone, came) = match op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index, old_len, ..
            } => (old_index..old_index + old_len, 0..0),
            DiffOp::Insert {
                new_index, new_len, ..
            } => (0..0, new_index..new_index + new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            ),
        };
        removed += gone.len();
        added += came.len();
        out.extend(gone.map(|i| format!("- {}", old_raw[i].trim_start())));
        out.extend(came.map(|i| format!("+ {}", new_raw[i].trim_start())));
    }
    if out.len() > MAX_CHANGE_LINES {
        let more = out.len() - MAX_CHANGE_LINES;
        out.truncate(MAX_CHANGE_LINES);
        out.push(format!(
            "… {more} more changed lines; run `snapshot -i` for the whole page"
        ));
    }
    (out, added, removed)
}

fn cap_lines(text: &str, max: usize) -> String {
    let total = text.lines().count();
    if total <= max {
        return text.to_string();
    }
    let mut out: String = text.lines().take(max).collect::<Vec<_>>().join("\n");
    out.push_str(&format!(
        "\n… {} more lines; run `snapshot -i` for the whole page",
        total - max
    ));
    out
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn strip_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_ignores_ref_renumbering_and_shows_new_refs() {
        let old = "- button \"Save\" [ref=e1]\n- link \"Help\" [ref=e2]";
        let new =
            "- button \"Save\" [ref=e7]\n- link \"Help\" [ref=e8]\n- dialog \"Saved\" [ref=e9]";
        let (lines, added, removed) = line_diff(old, new);
        assert_eq!((added, removed), (1, 0));
        assert_eq!(lines, vec!["+ - dialog \"Saved\" [ref=e9]"]);
    }

    #[test]
    fn diff_of_identical_pages_is_empty() {
        let tree = "- button \"Save\" [ref=e1]";
        assert_eq!(line_diff(tree, tree), (vec![], 0, 0));
    }

    #[test]
    fn wanted_only_for_page_changing_actions() {
        assert!(wanted(&json!({ "observe": true }), "click"));
        assert!(!wanted(&json!({ "observe": true }), "snapshot"));
        assert!(!wanted(&json!({ "observe": false }), "click"));
    }
}
