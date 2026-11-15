use super::*;

/// Wait strategy used by `auth_login` when navigating to the login page.
///
/// We intentionally use `Load` (instead of `NetworkIdle`) because many modern
/// apps keep background requests active indefinitely (polling, analytics,
/// websockets), which can prevent network-idle from ever resolving.
///
/// After navigation completes, `auth_login` explicitly waits for form selectors
/// to appear before filling/clicking.
pub const AUTH_LOGIN_WAIT_UNTIL: WaitUntil = WaitUntil::Load;

/// Poll interval used while waiting for auth form selectors to appear.
pub(super) const AUTH_LOGIN_SELECTOR_POLL_INTERVAL_MS: u64 = 100;

/// Time spent trying targeted username selectors before broad text-input
/// fallback selectors are allowed.
pub(super) const AUTH_LOGIN_PREFERRED_SELECTOR_WINDOW_MS: u64 = 5_000;

/// Local snapshot ref used to keep the auth element selected during detection
/// bound to the exact DOM node used by the subsequent interaction.
pub(super) const AUTH_LOGIN_ELEMENT_REF: &str = "e1";

pub(super) const AUTH_LOGIN_NO_NAVIGATE_PAGE_ERROR: &str = "auth login --no-navigate requires an existing active HTTP(S) browser page; open the login page first";

pub struct PendingConfirmation {
    pub action: String,
    pub cmd: Value,
    pub(super) approved_actions: Vec<String>,
    /// The flow a `flow_run` was checked against, so the confirmed run
    /// executes the same file rather than one read again from disk.
    pub(super) flow: Option<Box<crate::native::flow::Flow>>,
}

#[derive(Debug, Clone)]
pub(super) struct ActiveProviderSession {
    pub(super) session: providers::ProviderSession,
    pub(super) plugins: Vec<crate::plugins::PluginConfig>,
}

/// Captured request/response metadata used to export HAR 1.2 files.
#[derive(Clone)]
pub struct HarEntry {
    pub request_id: String,
    /// Seconds since Unix epoch (CDP `wallTime`), with sub-second precision.
    pub wall_time: f64,
    // Request fields
    pub method: String,
    pub url: String,
    pub request_headers: Vec<(String, String)>,
    pub post_data: Option<String>,
    pub request_body_size: i64,
    pub resource_type: String,
    // Response fields — populated by `Network.responseReceived`
    pub status: Option<i64>,
    pub status_text: String,
    /// Normalised from CDP `response.protocol` (e.g. `"h2"` → `"HTTP/2.0"`).
    pub http_version: String,
    pub response_headers: Vec<(String, String)>,
    pub mime_type: String,
    pub redirect_url: String,
    /// Updated by `Network.loadingFinished` for final accuracy.
    pub response_body_size: i64,
    /// Raw CDP `ResourceTiming` object from `Network.responseReceived`.
    pub cdp_timing: Option<Value>,
    /// Monotonic timestamp (seconds) from `Network.loadingFinished`; used to
    /// compute the `receive` timing phase.
    pub loading_finished_timestamp: Option<f64>,
    /// Response body fetched via `Network.getResponseBody` when the entry
    /// finished loading, subject to the active [`HarContentMode`] and size caps.
    pub response_body: Option<String>,
    /// Whether `response_body` is base64-encoded (binary content).
    pub response_body_base64: bool,
    /// The address and port the browser connected to (CDP
    /// `remoteIPAddress` and `remotePort`).
    pub remote_addr: Option<std::net::SocketAddr>,
}

/// Which response bodies to embed in HAR output as `content.text`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HarContentMode {
    /// Sizes and MIME types only (pre-0.33 behavior).
    None,
    /// Text-like bodies only: JSON, XML, HTML, JS, form data, SVG.
    #[default]
    Text,
    /// Every body; binary content is embedded base64-encoded.
    All,
}

impl HarContentMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "none" => Ok(Self::None),
            "text" => Ok(Self::Text),
            "all" => Ok(Self::All),
            other => Err(format!(
                "Invalid HAR content mode '{}'. Valid options: all, text, none",
                other
            )),
        }
    }
}

/// Bodies larger than this are not embedded in the HAR (the entry keeps its
/// size/MIME metadata either way).
pub(super) const HAR_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Total budget for embedded bodies across one recording session.
pub(super) const HAR_MAX_TOTAL_BODY_BYTES: usize = 64 * 1024 * 1024;

pub struct RouteEntry {
    pub url_pattern: String,
    pub response: Option<RouteResponse>,
    pub abort: bool,
    /// When non-empty, only requests whose `resourceType` (as reported by
    /// CDP Fetch.requestPaused) is in this list are matched. Values are
    /// compared case-insensitively. Empty means "match any resource type".
    pub resource_types: Vec<String>,
}

pub struct RouteResponse {
    pub status: Option<u16>,
    pub body: Option<String>,
    pub content_type: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    /// Origin a page must have to read the mock cross-origin (`--allow-origin`).
    pub allow_origin: Option<String>,
}

/// Most requests the `network requests` log keeps; older ones are dropped.
pub(super) const TRACKED_REQUESTS_MAX: usize = 10_000;
/// How many of the oldest requests are dropped once the log is over the cap.
pub(super) const TRACKED_REQUESTS_TRIM: usize = 1_000;

#[derive(Clone, serde::Serialize)]
pub struct TrackedRequest {
    pub url: String,
    pub method: String,
    pub headers: Value,
    pub timestamp: u64,
    #[serde(rename = "resourceType")]
    pub resource_type: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "postData", skip_serializing_if = "Option::is_none")]
    pub post_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(rename = "responseHeaders", skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Value>,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

pub struct FetchPausedRequest {
    pub request_id: String,
    pub url: String,
    pub resource_type: String,
    pub frame_id: String,
    pub session_id: String,
    /// Original request headers from the Fetch.requestPaused event, needed
    /// because Fetch.continueRequest replaces (not merges) headers.
    pub request_headers: Option<serde_json::Map<String, Value>>,
}

pub enum BackendType {
    Cdp,
    WebDriver,
}

#[derive(Debug, Clone, Default)]
pub struct PendingDialog {
    pub dialog_type: String,
    pub message: String,
    pub url: String,
    pub default_prompt: Option<String>,
    /// Flat CDP session the dialog opened on. A dialog on a background tab
    /// must not block commands targeting the active tab.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MouseState {
    pub x: f64,
    pub y: f64,
    pub buttons: i32,
}

#[derive(Debug, Clone)]
pub(super) struct SnapshotRevision {
    pub(super) revision: u64,
    pub(super) url: String,
    pub(super) options: String,
    pub(super) tree: String,
    pub(super) refs: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone)]
pub(super) struct ScreenshotObservation {
    pub(super) revision: u64,
    pub(super) signature: String,
    pub(super) decoded_hash: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

#[derive(Default)]
pub(super) struct DrainedEvents {
    pub(super) pending_acks: Vec<i64>,
    pub(super) new_targets: Vec<TargetCreatedEvent>,
    pub(super) changed_targets: Vec<TargetInfoChangedEvent>,
    pub(super) destroyed_targets: Vec<String>,
    /// Top-level page/webview targets attached by browser-level auto-attach.
    pub(super) attached_page_sessions: Vec<(TargetInfo, String)>,
    /// Cross-origin iframe (frame_id, session_id) pairs from Target.attachedToTarget.
    pub(super) attached_iframe_sessions: Vec<(String, String)>,
    /// Worker-like targets that can initiate network traffic but do not support
    /// page-domain setup.
    pub(super) attached_worker_sessions: Vec<(TargetInfo, String)>,
    /// Attached non-page targets that still need to be resumed when auto-attach
    /// pauses them, even though frob does not track them as tabs.
    pub(super) attached_other_sessions: Vec<String>,
    /// Session IDs from Target.detachedFromTarget.
    pub(super) detached_iframe_sessions: Vec<String>,
    /// (request_id, event session_id) pairs from `Network.loadingFinished`
    /// while HAR recording; bodies are fetched for these in
    /// `apply_drained_events` before Chrome evicts them (e.g. on navigation).
    pub(super) har_finished_requests: Vec<(String, Option<String>)>,
}

pub(super) fn is_active_iframe_network_event(
    method: &str,
    session_id: Option<&str>,
    tracking_enabled: bool,
    active_iframe_sessions: &HashSet<String>,
) -> bool {
    tracking_enabled
        && method.starts_with("Network.")
        && session_id.is_some_and(|sid| active_iframe_sessions.contains(sid))
}

pub(super) fn active_frame_scope_may_have_changed(drained: &DrainedEvents) -> bool {
    !drained.attached_iframe_sessions.is_empty()
        || !drained.detached_iframe_sessions.is_empty()
        || !drained.attached_page_sessions.is_empty()
        || !drained.new_targets.is_empty()
        || !drained.destroyed_targets.is_empty()
}

/// Launch fields that only describe a browser frob starts itself. The CLI never
/// sends them with an external target, so a launch that inherits one drops them.
pub(super) const LOCAL_ONLY_LAUNCH_FIELDS: &[&str] = &[
    "allowFileAccess",
    "args",
    "engine",
    "executablePath",
    "extensions",
    "headless",
    "hideScrollbars",
    "noXvfb",
    "profile",
    "proxy",
    "proxyBypass",
    "sharedBrowser",
    "storageState",
    "userAgent",
    "webgpu",
];

/// External browser a session attached to. A later launch that names no connection
/// target resolves to it, so the session stays on that browser until `close`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AttachedTarget {
    CdpUrl(String),
    CdpPort(u64),
    AutoConnect,
    Provider(String),
}

impl AttachedTarget {
    /// Reads the connection target a launch command names, if any.
    pub(super) fn named_by(cmd: &Value) -> Option<Self> {
        // Blank, zero and false name no browser, here and in the settings.
        if let Some(url) = cmd.get("cdpUrl").and_then(Value::as_str).and_then(named) {
            return Some(Self::CdpUrl(url.to_string()));
        }
        if let Some(port) = cmd
            .get("cdpPort")
            .and_then(Value::as_u64)
            .filter(|port| *port > 0)
        {
            return Some(Self::CdpPort(port));
        }
        if cmd.get("autoConnect").and_then(Value::as_bool) == Some(true) {
            return Some(Self::AutoConnect);
        }
        let provider = cmd
            .get("provider")
            .and_then(Value::as_str)
            .and_then(named)?;
        Some(Self::Provider(provider.to_ascii_lowercase()))
    }

    /// The target a successful launch leaves the session attached to. The iOS and
    /// Safari providers are WebDriver-backed and have no CDP attach to resume.
    pub(super) fn from_launch_command(cmd: &Value) -> Option<Self> {
        Self::named_by(cmd)
            .filter(|target| !matches!(target, Self::Provider(p) if p == "ios" || p == "safari"))
    }

    fn insert_into(&self, cmd: &mut serde_json::Map<String, Value>) {
        let (key, value) = match self {
            Self::CdpUrl(url) => ("cdpUrl", json!(url)),
            Self::CdpPort(port) => ("cdpPort", json!(port)),
            Self::AutoConnect => ("autoConnect", json!(true)),
            Self::Provider(provider) => ("provider", json!(provider)),
        };
        cmd.insert(key.to_string(), value);
    }
}

/// Points a launch that names no connection target at the browser the session is
/// attached to, without the fields only a local launch honors, so it runs exactly
/// like the original attach. Returns `None` when the launch needs no rewrite.
pub(super) fn resolve_launch_target(
    cmd: &Value,
    attached: Option<&AttachedTarget>,
) -> Option<Value> {
    let attached = attached?;
    if AttachedTarget::named_by(cmd).is_some() {
        return None;
    }
    let mut resolved = cmd.as_object()?.clone();
    for field in LOCAL_ONLY_LAUNCH_FIELDS {
        resolved.remove(*field);
    }
    attached.insert_into(&mut resolved);
    Some(Value::Object(resolved))
}

pub(super) fn validate_ca_cert_launch_mode(
    options: &LaunchOptions,
    engine: Option<&str>,
    external_launch: bool,
) -> Result<(), String> {
    if options.ca_cert.is_none() {
        return Ok(());
    }
    if external_launch {
        return Err(
            "CA trust is active for this session and requires a locally launched Chromium browser on Linux. Pass --no-ca-cert to clear it before using CDP, auto-connect, or a provider."
                .to_string(),
        );
    }
    if engine.is_some_and(|value| !value.eq_ignore_ascii_case("chrome")) {
        return Err("--ca-cert is supported only with the Chrome engine on Linux".to_string());
    }
    if options.ignore_https_errors {
        return Err("--ca-cert cannot be combined with --ignore-https-errors".to_string());
    }
    crate::native::browser::validate_launch_options(
        options.extensions.as_deref(),
        false,
        options.profile.as_deref(),
        options.storage_state.as_deref(),
        options.allow_file_access,
        options.executable_path.as_deref(),
        options.ca_cert.as_deref(),
    )
}

#[derive(Clone, Debug)]
pub(super) struct EffectiveCaCert {
    pub(super) path: String,
    pub(super) bundle: crate::ca_bundle::CaBundle,
}

pub(super) fn resolve_effective_ca_cert(
    cmd: &Value,
    state: &DaemonState,
) -> Result<Option<EffectiveCaCert>, String> {
    let requested_path = cmd.get("caCert").and_then(Value::as_str);
    let clear = cmd
        .get("clearCaCert")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if requested_path.is_some() && clear {
        return Err("Cannot use --ca-cert with --no-ca-cert".to_string());
    }
    if clear {
        return Ok(None);
    }
    let Some(path) = requested_path else {
        return Ok(state.effective_ca_cert.clone());
    };
    let bundle = crate::ca_bundle::load(path)?;
    if state
        .effective_ca_cert
        .as_ref()
        .is_some_and(|current| current.bundle.digest() == bundle.digest())
    {
        return Ok(state.effective_ca_cert.clone());
    }
    Ok(Some(EffectiveCaCert {
        path: path.to_string(),
        bundle,
    }))
}

pub(super) fn apply_effective_ca_cert(
    options: &mut LaunchOptions,
    effective_ca_cert: &Option<EffectiveCaCert>,
) {
    options.ca_cert = effective_ca_cert.as_ref().map(|ca| ca.path.clone());
    options.ca_bundle = effective_ca_cert.as_ref().map(|ca| ca.bundle.clone());
    options.ca_cert_digest = effective_ca_cert.as_ref().map(|ca| *ca.bundle.digest());
}

/// Arguments of the last `Emulation.setEmulatedMedia` call, kept so the same
/// emulation can be replayed onto a new page session.
#[derive(Debug, Clone)]
pub struct EmulatedMedia {
    pub media: Option<String>,
    /// `(name, value)` media features, e.g. `("prefers-color-scheme", "dark")`.
    pub features: Vec<(String, String)>,
}

/// An init script tracked across target sessions.
///
/// Chrome assigns script identifiers independently in each target session, so
/// two tabs can both return the same identifier for different scripts. The
/// daemon-owned `identifier` is the user-facing handle, while
/// `session_identifiers` records the target-local identifier to send when
/// removing it from a particular target.
#[derive(Debug, Clone)]
pub struct InitScriptSetup {
    pub identifier: String,
    pub source: String,
    pub session_identifiers: HashMap<String, String>,
}

/// Page-level setup the daemon has applied to the active page session.
///
/// CDP scopes all of these to a single target session, so a tab the daemon
/// creates itself starts without them. This record lets
/// [`apply_session_setup`] replay them onto the new tab before its first
/// navigation. Reset whenever a browser is (re)launched. Permissions are not
/// tracked: `Browser.grantPermissions` is context-scoped and already covers
/// new tabs in the default context.
#[derive(Debug, Default, Clone)]
pub struct SessionSetup {
    /// `--user-agent`, `useragent`, or the UA half of `device`.
    pub user_agent: Option<String>,
    /// Last `Emulation.setEmulatedMedia` call: `--color-scheme` at launch or
    /// `set_media` / `emulatemedia`.
    pub emulated_media: Option<EmulatedMedia>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    /// `(latitude, longitude, accuracy)`.
    pub geolocation: Option<(f64, f64, Option<f64>)>,
    /// Global headers from the `headers` and `credentials` commands
    /// (`Network.setExtraHTTPHeaders`). Origin-scoped `--headers` live in
    /// `DaemonState::origin_headers`.
    pub extra_headers: Option<HashMap<String, String>>,
    pub offline: Option<bool>,
    /// Init scripts registered with `Page.addScriptToEvaluateOnNewDocument`:
    /// `--enable`, `--init-script`, plugin init scripts, and `addinitscript`.
    pub init_scripts: Vec<InitScriptSetup>,
    /// Monotonic source for daemon-owned init-script handles. CDP identifiers
    /// cannot be used here because each target session allocates them
    /// independently, commonly starting at `1`.
    pub(super) next_init_script_handle: u64,
}

impl SessionSetup {
    pub(super) fn from_launch_options(options: &LaunchOptions) -> Self {
        Self {
            user_agent: options.user_agent.clone(),
            emulated_media: options.color_scheme.as_ref().map(|scheme| EmulatedMedia {
                media: None,
                features: vec![("prefers-color-scheme".to_string(), scheme.clone())],
            }),
            ..Self::default()
        }
    }

    /// True when nothing has been configured, so there is nothing to replay.
    pub(super) fn is_empty(&self) -> bool {
        self.user_agent.is_none()
            && self.emulated_media.is_none()
            && self.timezone.is_none()
            && self.locale.is_none()
            && self.geolocation.is_none()
            && self.extra_headers.as_ref().is_none_or(HashMap::is_empty)
            && !self.offline.unwrap_or(false)
            && self.init_scripts.is_empty()
    }

    pub(super) fn register_init_script(
        &mut self,
        source: String,
        session_id: String,
        session_identifier: String,
    ) -> String {
        self.next_init_script_handle += 1;
        let identifier = format!("init-script-{}", self.next_init_script_handle);
        let mut session_identifiers = HashMap::new();
        session_identifiers.insert(session_id, session_identifier);
        self.init_scripts.push(InitScriptSetup {
            identifier: identifier.clone(),
            source,
            session_identifiers,
        });
        identifier
    }
}
