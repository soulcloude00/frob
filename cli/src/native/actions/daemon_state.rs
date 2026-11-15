use super::*;

pub struct DaemonState {
    pub browser: Option<BrowserManager>,
    pub appium: Option<AppiumManager>,
    pub safari_driver: Option<safari::SafariDriverProcess>,
    pub webdriver_backend: Option<crate::native::webdriver::backend::WebDriverBackend>,
    pub backend_type: BackendType,
    pub ref_map: RefMap,
    /// Last delta snapshot per page session. Bounded to the currently tracked tabs.
    pub(super) snapshot_revisions: HashMap<String, SnapshotRevision>,
    pub(super) screenshot_observations: HashMap<(String, String), ScreenshotObservation>,
    pub domain_filter: Arc<RwLock<Option<DomainFilter>>>,
    pub event_tracker: EventTracker,
    pub session_name: Option<String>,
    pub restore_save: String,
    pub restore_check_url: Option<String>,
    pub restore_check_text: Option<String>,
    pub restore_check_fn: Option<String>,
    pub restore_status: String,
    pub restore_status_detail: Option<String>,
    /// Origins whose storage the last launch-time restore skipped because the page left that origin.
    pub restore_skipped_origins: Vec<String>,
    pub restore_loaded_path: Option<String>,
    pub restore_load_failed: bool,
    pub restore_validation_pending: bool,
    pub restore_save_status: String,
    pub restore_saved_path: Option<String>,
    /// When the most recent browser-touching command finished. Periodic
    /// autosaves wait for a quiet period after this so a multi-second save
    /// never lands in the middle of an active command burst.
    pub last_command_finished: Option<std::time::Instant>,
    /// When session state was last saved or a periodic autosave last failed,
    /// used to enforce the minimum interval between periodic saves.
    pub last_autosave_attempt: Option<std::time::Instant>,
    pub session_id: String,
    /// The settings this session runs with.
    pub settings: Running,
    /// Where the security settings are kept across daemon restarts.
    pub(super) store: Arc<dyn Store>,
    pub tracing_state: TracingState,
    pub recording_state: RecordingState,
    pub(super) event_rx: Option<broadcast::Receiver<CdpEvent>>,
    /// Downloads seen on the attached browser, for `wait --download`.
    pub(super) downloads: DownloadLedger,
    pub webmcp: webmcp::RuntimeState,
    /// Whether the active launch opted into Chrome's experimental WebMCP features.
    pub webmcp_enabled: bool,
    pub screencasting: bool,
    pub policy: Option<ActionPolicy>,
    pub pending_confirmation: Option<PendingConfirmation>,
    /// A flow handed from a confirmation to the re-executed `flow_run`.
    pub(super) preloaded_flow: Option<Box<crate::native::flow::Flow>>,
    pub har_recording: bool,
    pub har_entries: Vec<HarEntry>,
    /// Commands and request tags captured by `flow start`.
    pub flow: crate::native::flow::FlowRecorder,
    /// Registrable domain whose saved flows were last announced.
    pub flow_hinted: Option<String>,
    /// Why the last `flow run` stopped, for the error response.
    pub flow_stop: Option<Value>,
    pub har_content_mode: HarContentMode,
    /// Bytes of response bodies embedded so far this recording; enforces
    /// [`HAR_MAX_TOTAL_BODY_BYTES`].
    pub har_body_total_bytes: usize,
    /// When each recorded request last received its headers or body bytes.
    pub har_progress: HashMap<String, std::time::Instant>,
    pub confirm_actions: Option<ConfirmActions>,
    pub inspect_server: Option<InspectServer>,
    pub routes: Arc<RwLock<Vec<RouteEntry>>>,
    pub tracked_requests: Vec<TrackedRequest>,
    pub request_tracking: bool,
    pub active_frame_id: Option<String>,
    /// Cross-origin iframe frame_id → dedicated CDP session_id.
    /// Populated by Target.attachedToTarget events from Target.setAutoAttach.
    /// Entries are retained across tab changes because Chrome does not emit a
    /// second attachment event when returning to an already-attached tab.
    /// Target.detachedFromTarget events remove stale sessions.
    pub iframe_sessions: HashMap<String, String>,
    /// Dedicated iframe sessions reachable from the currently active page.
    /// Network tracking uses this subset so background-tab iframe traffic is
    /// not mixed into the active tab's request list or HAR capture.
    pub active_iframe_sessions: HashSet<String>,
    /// Origin-scoped extra HTTP headers set via `--headers` on navigate.
    /// Key is the origin (scheme + host + port), value is the headers map.
    /// Wrapped in Arc<RwLock<>> so the background Fetch handler can read it.
    pub origin_headers: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    /// Last document URL the domain filter blocked, keyed by frame ID, so
    /// navigation can report a redirect that landed on the blocked page.
    pub blocked_documents: Arc<std::sync::Mutex<HashMap<String, String>>>,
    /// Proxy authentication credentials (username, password) for handling
    /// Fetch.authRequired events from authenticated proxies.
    pub proxy_credentials: Arc<RwLock<Option<(String, String)>>>,
    /// Background task that processes Fetch.requestPaused events in real-time,
    /// handling domain filtering, route interception, and origin-scoped headers
    /// without deadlocking navigation/evaluate.
    pub(super) fetch_handler_task: Option<tokio::task::JoinHandle<()>>,
    /// Background task that auto-accepts `alert` and `beforeunload` dialogs
    /// so they never block the agent.
    dialog_handler_task: Option<tokio::task::JoinHandle<()>>,
    pub mouse_state: MouseState,
    /// Session-wide pointer behavior, configured by `--input-mode`.
    pub input_mode: String,
    /// Tracks the currently open JavaScript dialog (alert/confirm/prompt), if any.
    pub pending_dialog: Option<PendingDialog>,
    /// A `page_wedged` report whose response may not have reached the client,
    /// keyed by the request id. The client's read timeout can expire before
    /// the daemon finishes replacing the tab, so a retry of the same request
    /// gets the report instead of running on the blank replacement tab.
    pub undelivered_wedge: Option<(String, WedgeReport)>,
    /// A mouse button left logically down because a dialog opened between
    /// mousePressed and mouseReleased; released when the dialog is resolved.
    pub pending_pointer_release: Option<crate::native::interaction::PendingRelease>,
    /// When true, automatically dismiss `beforeunload` dialogs and accept `alert`
    /// dialogs so they never block the agent.  Enabled by default.
    pub auto_dialog: bool,
    /// Shared slot for stream server to receive CDP client when browser launches.
    pub stream_client: Option<Arc<RwLock<Option<Arc<CdpClient>>>>>,
    /// Stream server instance kept alive so the broadcast channel remains open.
    pub stream_server: Option<Arc<StreamServer>>,
    /// Daemon-owned activity clock shared by commands and stream servers.
    pub idle_activity: Arc<IdleActivity>,
    /// Hash of launch options used for the current browser, for relaunch detection.
    pub(super) launch_identity: Option<u64>,
    /// CDP endpoint and headers from the last explicit CDP launch, reused when
    /// the daemon reconnects to that same endpoint on its own.
    pub(super) cdp_headers: Option<(String, Vec<(String, String)>)>,
    /// External browser this session attached to; target-less launches resolve to it.
    pub(super) attached_target: Option<AttachedTarget>,
    pub(super) effective_ca_cert: Option<EffectiveCaCert>,
    /// Whether browser-level auto-attach has been enabled for the current
    /// browser so top-level popups pause before their first request.
    pub(super) network_auto_attach_installed: bool,
    /// Browser engine name (e.g. "chrome", "lightpanda") for observability.
    pub engine: String,
    /// Default timeout for wait operations, from FROB_DEFAULT_TIMEOUT env var.
    pub default_timeout_ms: u64,
    /// Last viewport settings (width, height, deviceScaleFactor, mobile),
    /// re-applied to new contexts (e.g., recording).
    pub viewport: Option<(i32, i32, f64, bool)>,
    /// Session-scoped setup applied to the active page, re-applied to tabs the
    /// daemon creates. See [`SessionSetup`].
    pub session_setup: SessionSetup,
    /// Init script sources returned by launch mutator plugins for this launch.
    pub plugin_init_scripts: Vec<String>,
    /// Provider cleanup metadata for the active external browser session.
    pub(super) active_provider_session: Option<ActiveProviderSession>,
    /// True when the connected CDP browser is owned by a cloud provider.
    ///
    /// This is tracked separately from cleanup metadata because provider
    /// plugins are allowed to omit a close-session payload.
    pub(super) active_provider_connection: bool,
    /// Browser product already checked for an outdated build, so each product is checked once even when the fetch fails.
    pub(super) browser_security_checked: Option<String>,
    /// The outdated-browser check runs in the background, since it fetches a release feed over
    /// the network; its warning is attached to the first response after it finishes.
    pub(super) browser_security_pending: Option<tokio::task::JoinHandle<Option<String>>>,
    /// Actions already approved while replaying a confirmed command.
    pub(super) confirmed_policy_actions: HashSet<String>,
    /// Strict session-to-tab binding (`--pin-tab` / FROB_PIN_TAB).
    /// Sticky per session: persisted in the binding file so later commands
    /// and daemon restarts keep the strict semantics.
    pub pin_tab: bool,
    /// Last binding written to disk, so persistence is write-on-change.
    pub(super) last_persisted_binding: Option<tab_binding::TabBinding>,
}

pub(super) fn default_idle_shutdown_is_blocked(
    is_webdriver: bool,
    provider_owned: bool,
    browser_blocks_shutdown: bool,
) -> bool {
    is_webdriver || (!provider_owned && browser_blocks_shutdown)
}

impl DaemonState {
    /// The client received the `page_wedged` response to request `id`, so it
    /// is not replayed. A newer report from another request stays pending.
    pub fn wedge_report_delivered(&mut self, id: &str) {
        if self
            .undelivered_wedge
            .as_ref()
            .is_some_and(|(wedged_id, _)| wedged_id == id)
        {
            self.undelivered_wedge = None;
        }
    }

    /// A session seeded from the environment whose security settings are kept
    /// in memory only.
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_store(
            Running::from_env(),
            Arc::new(crate::session_settings::MemStore::default()),
        )
    }

    /// A session that starts with `settings` and keeps its security settings
    /// in `store`.
    pub fn with_store(settings: Running, store: Arc<dyn Store>) -> Self {
        let session_id = env::var("FROB_SESSION").unwrap_or_else(|_| "default".to_string());
        let security = settings.security();
        let restore = settings.restore();
        // A corrupt binding file is surfaced later, on attach; it does not
        // enable pinning here because its pinned state is unknowable.
        let pin_tab = settings.pin_tab()
            || tab_binding::load(&session_id)
                .ok()
                .flatten()
                .is_some_and(|b| b.pinned);
        Self {
            browser: None,
            appium: None,
            safari_driver: None,
            webdriver_backend: None,
            backend_type: BackendType::Cdp,
            ref_map: RefMap::new(),
            snapshot_revisions: HashMap::new(),
            screenshot_observations: HashMap::new(),
            domain_filter: Arc::new(RwLock::new(
                // An empty list is no filter, as when it is cleared live.
                security
                    .allowed_domains
                    .as_ref()
                    .filter(|domains| !domains.is_empty())
                    .map(|domains| DomainFilter::new(&domains.join(","))),
            )),
            event_tracker: EventTracker::new(),
            session_name: restore.key,
            restore_save: restore.save,
            restore_check_url: restore.check_url,
            restore_check_text: restore.check_text,
            restore_check_fn: restore.check_fn,
            restore_status: "not_configured".to_string(),
            restore_status_detail: None,
            restore_skipped_origins: Vec::new(),
            restore_loaded_path: None,
            restore_load_failed: false,
            restore_validation_pending: false,
            restore_save_status: "not_attempted".to_string(),
            restore_saved_path: None,
            last_command_finished: None,
            last_autosave_attempt: None,
            session_id,
            engine: settings.engine().unwrap_or_else(|| "chrome".to_string()),
            // Intentionally below the CLI's 30s IPC read timeout (see
            // `DEFAULT_TIMEOUT_MS`).
            default_timeout_ms: settings.default_timeout_ms(),
            auto_dialog: !settings.no_auto_dialog(),
            tracing_state: TracingState::new(),
            recording_state: RecordingState::new(),
            event_rx: None,
            downloads: DownloadLedger::default(),
            webmcp: webmcp::RuntimeState::default(),
            webmcp_enabled: false,
            screencasting: false,
            policy: security
                .action_policy_path
                .as_deref()
                .map(ActionPolicy::from_path),
            pending_confirmation: None,
            preloaded_flow: None,
            har_recording: false,
            har_entries: Vec::new(),
            flow: Default::default(),
            flow_hinted: None,
            flow_stop: None,
            har_content_mode: HarContentMode::default(),
            har_body_total_bytes: 0,
            har_progress: HashMap::new(),
            confirm_actions: security
                .confirm_actions
                .as_deref()
                .and_then(ConfirmActions::parse),
            inspect_server: None,
            routes: Arc::new(RwLock::new(Vec::new())),
            tracked_requests: Vec::new(),
            request_tracking: false,
            active_frame_id: None,
            iframe_sessions: HashMap::new(),
            active_iframe_sessions: HashSet::new(),
            origin_headers: Arc::new(RwLock::new(HashMap::new())),
            blocked_documents: Arc::new(std::sync::Mutex::new(HashMap::new())),
            proxy_credentials: Arc::new(RwLock::new(None)),
            fetch_handler_task: None,
            dialog_handler_task: None,
            mouse_state: MouseState::default(),
            input_mode: "instant".to_string(),
            pending_dialog: None,
            undelivered_wedge: None,
            pending_pointer_release: None,
            stream_client: None,
            stream_server: None,
            idle_activity: Arc::new(IdleActivity::new()),
            launch_identity: None,
            cdp_headers: None,
            attached_target: None,
            effective_ca_cert: None,
            network_auto_attach_installed: false,
            viewport: None,
            session_setup: SessionSetup::default(),
            plugin_init_scripts: Vec::new(),
            active_provider_session: None,
            active_provider_connection: false,
            browser_security_checked: None,
            browser_security_pending: None,
            confirmed_policy_actions: HashSet::new(),
            pin_tab,
            last_persisted_binding: None,
            settings,
            store,
        }
    }

    /// True when the default idle timeout must not shut down this session.
    /// WebDriver-backed Safari and iOS sessions are always headed, while CDP
    /// sessions delegate the decision to BrowserManager. Provider-owned CDP
    /// connections remain eligible because the daemon owns their lifecycle.
    pub(crate) fn blocks_default_idle_shutdown(&self) -> bool {
        default_idle_shutdown_is_blocked(
            matches!(self.backend_type, BackendType::WebDriver),
            self.active_provider_connection,
            self.browser
                .as_ref()
                .is_some_and(BrowserManager::blocks_default_idle_shutdown),
        )
    }

    /// Extract the timeout from a command JSON, falling back to the
    /// configured `default_timeout_ms` (from `FROB_DEFAULT_TIMEOUT`).
    /// All wait-family handlers should use this instead of reading the
    /// timeout field and providing their own fallback.
    pub(super) fn timeout_ms(&self, cmd: &Value) -> u64 {
        cmd.get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.default_timeout_ms)
    }

    pub(super) fn reset_input_state(&mut self) {
        self.mouse_state = MouseState::default();
    }

    /// Create state with an optional stream client slot and server instance
    /// (for daemon startup with stream server).
    pub fn new_with_stream(
        settings: Running,
        store: Arc<dyn Store>,
        stream_client: Option<Arc<RwLock<Option<Arc<CdpClient>>>>>,
        stream_server: Option<Arc<StreamServer>>,
        idle_activity: Arc<IdleActivity>,
    ) -> Self {
        let mut s = Self::with_store(settings, store);
        if stream_server.is_some() {
            s.request_tracking = true;
        }
        s.stream_client = stream_client;
        s.stream_server = stream_server;
        s.idle_activity = idle_activity;
        s
    }

    pub(super) fn subscribe_to_browser_events(&mut self) {
        self.downloads.clear();
        if let Some(ref browser) = self.browser {
            self.event_rx = Some(browser.client.subscribe());
        }
    }

    /// Start the background task that processes Fetch.requestPaused and
    /// Fetch.authRequired events in real-time (domain filtering, route
    /// interception, origin-scoped headers, proxy authentication).
    /// Must be called after the browser is set and events are subscribed.
    pub(super) fn start_fetch_handler(&mut self) {
        // Abort any existing handler.
        if let Some(task) = self.fetch_handler_task.take() {
            task.abort();
        }

        let Some(ref browser) = self.browser else {
            return;
        };

        let client = browser.client.clone();
        let mut rx = browser.client.subscribe_methods(&[
            "Fetch.authRequired",
            "Target.attachedToTarget",
            "Fetch.requestPaused",
        ]);
        let domain_filter = self.domain_filter.clone();
        let routes = self.routes.clone();
        let origin_headers = self.origin_headers.clone();
        let blocked_documents = self.blocked_documents.clone();
        let proxy_credentials = self.proxy_credentials.clone();
        let capture_session = self.recording_state.capture_session.clone();

        self.fetch_handler_task = Some(tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Some(event) if event.method == "Fetch.authRequired" => {
                        let request_id = event
                            .params
                            .get("requestId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let sid = event.session_id.clone().unwrap_or_default();
                        let creds = proxy_credentials.read().await;
                        if let Some((ref user, ref pass)) = *creds {
                            let _ = client
                                .send_command(
                                    "Fetch.continueWithAuth",
                                    Some(json!({
                                        "requestId": request_id,
                                        "authChallengeResponse": {
                                            "response": "ProvideCredentials",
                                            "username": user,
                                            "password": pass,
                                        }
                                    })),
                                    Some(&sid),
                                )
                                .await;
                        } else {
                            let _ = client
                                .send_command(
                                    "Fetch.continueWithAuth",
                                    Some(json!({
                                        "requestId": request_id,
                                        "authChallengeResponse": {
                                            "response": "CancelAuth",
                                        }
                                    })),
                                    Some(&sid),
                                )
                                .await;
                        }
                    }
                    Some(event) if event.method == "Target.attachedToTarget" => {
                        let Some(sid) = event
                            .params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .map(ToString::to_string)
                        else {
                            continue;
                        };
                        let target_info = event.params.get("targetInfo").and_then(|value| {
                            serde_json::from_value::<TargetInfo>(value.clone()).ok()
                        });
                        // The recorder's screencast session shares a target
                        // with a page session that already carries the
                        // controls.
                        let is_recorder_session = target_info.as_ref().is_some_and(|target| {
                            recording::owns_attachment(&capture_session, &target.target_id, &sid)
                        });
                        if is_recorder_session {
                            continue;
                        }
                        let waiting_for_debugger = event
                            .params
                            .get("waitingForDebugger")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        let df = domain_filter.read().await.clone();
                        let has_proxy_creds = proxy_credentials.read().await.is_some();
                        setup_auto_attached_session(
                            &client,
                            &sid,
                            target_info.as_ref(),
                            waiting_for_debugger,
                            df.as_ref(),
                            has_proxy_creds,
                        )
                        .await;
                    }
                    Some(event) if event.method == "Fetch.requestPaused" => {
                        let request_id = event
                            .params
                            .get("requestId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let request_url = event
                            .params
                            .get("request")
                            .and_then(|r| r.get("url"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let resource_type = event
                            .params
                            .get("resourceType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let request_headers = event
                            .params
                            .get("request")
                            .and_then(|r| r.get("headers"))
                            .and_then(|h| h.as_object())
                            .cloned();
                        let frame_id = event
                            .params
                            .get("frameId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let sid = event.session_id.clone().unwrap_or_default();

                        let paused = FetchPausedRequest {
                            request_id,
                            url: request_url,
                            resource_type,
                            frame_id,
                            session_id: sid,
                            request_headers,
                        };

                        let df = domain_filter.read().await;
                        let rt = routes.read().await;
                        let oh = origin_headers.read().await;

                        resolve_fetch_paused(
                            &client,
                            df.as_ref(),
                            &rt,
                            &oh,
                            &blocked_documents,
                            &paused,
                        )
                        .await;
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
        }));
    }

    /// Start the background task that auto-accepts `alert` and `beforeunload`
    /// dialogs so they never block the agent. `confirm` and `prompt` dialogs
    /// are left for the agent to handle explicitly.
    pub(super) fn start_dialog_handler(&mut self) {
        if let Some(task) = self.dialog_handler_task.take() {
            task.abort();
        }

        if !self.auto_dialog {
            return;
        }

        let Some(ref browser) = self.browser else {
            return;
        };

        let client = browser.client.clone();
        let mut rx = browser.client.subscribe();

        self.dialog_handler_task = Some(tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) if event.method == "Page.javascriptDialogOpening" => {
                        let dialog_type = event
                            .params
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if matches!(dialog_type, "beforeunload" | "alert") {
                            let message = event
                                .params
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            eprintln!("[auto-dismiss] {} dialog: {}", dialog_type, message);
                            let sid = event.session_id.clone().unwrap_or_default();
                            if let Err(e) = client
                                .send_command(
                                    "Page.handleJavaScriptDialog",
                                    Some(json!({ "accept": true })),
                                    Some(&sid),
                                )
                                .await
                            {
                                eprintln!(
                                    "[auto-dismiss] failed to dismiss {} dialog: {}",
                                    dialog_type, e
                                );
                            }
                        }
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        }));
    }

    /// Update the stream server's CDP client slot when browser is set or cleared.
    pub async fn update_stream_client(&self) {
        if let Some(ref slot) = self.stream_client {
            let mut guard = slot.write().await;
            *guard = self.browser.as_ref().map(|m| Arc::clone(&m.client));
        }
        if let Some(ref server) = self.stream_server {
            // Update the CDP page session ID so screencast commands target the right page
            let session_id = self
                .browser
                .as_ref()
                .and_then(|m| m.active_session_id().ok().map(|s| s.to_string()));
            let connected = self.browser.is_some();
            let sc = server.is_screencasting().await;
            let (vw, vh) = server.viewport().await;
            if let Some(ref mgr) = self.browser {
                server
                    .bind_cdp_session_and_broadcast_tabs(session_id, &mgr.tab_list())
                    .await;
            } else {
                server
                    .bind_cdp_session_and_broadcast_tabs(session_id, &[])
                    .await;
            }
            server
                .broadcast_status(connected, sc, vw, vh, &self.engine)
                .await;
        }
    }

    /// Attach the recorder's own CDP session to the page behind `session_id`
    /// and spawn the task that screencasts into ffmpeg. The caller verifies
    /// ffmpeg before any navigation or restart teardown. Failures here roll the
    /// recording state back so the next `record start` is not blocked.
    pub(super) async fn start_recording_task(
        &mut self,
        client: Arc<CdpClient>,
        session_id: String,
    ) -> Result<(), String> {
        let initial_frame = match recording::capture_initial_image(&client, &session_id).await {
            Ok(frame) => frame,
            Err(error) => {
                self.rollback_failed_recording_start().await;
                return Err(error);
            }
        };
        let capture_session = match recording::attach_capture_session(
            &client,
            &session_id,
            &self.recording_state.capture_session,
        )
        .await
        {
            Ok(capture_session) => capture_session,
            Err(e) => {
                self.rollback_failed_recording_start().await;
                return Err(e);
            }
        };
        let shared_count = Arc::new(AtomicU64::new(0));
        let shared_captured = Arc::new(AtomicU64::new(0));
        let shared_contact_sheet_count = Arc::new(AtomicU64::new(0));
        let shared_screenshot_capture = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.recording_state.device_scale = initial_frame.device_scale;
        let (cancel_tx, cancel_rx) = oneshot::channel();
        if self.recording_state.cursor {
            self.refresh_active_iframe_sessions().await;
            let mut sessions: Vec<_> = self.active_iframe_sessions.iter().cloned().collect();
            sessions.push(capture_session.clone());
            if let Err(error) = recording::ensure_cursor_overlays(
                &client,
                &self.recording_state.cursor_overlays,
                &sessions,
            )
            .await
            {
                recording::remove_cursor_overlays(&client, &self.recording_state.cursor_overlays)
                    .await;
                recording::detach_capture_session(&client, &capture_session).await;
                self.rollback_failed_recording_start().await;
                return Err(format!("Failed to install recording cursor: {error}"));
            }
        }
        let handle = recording::spawn_recording_task(
            client,
            capture_session,
            session_id,
            initial_frame,
            self.recording_state.output_path.clone(),
            self.recording_state.fps,
            shared_count.clone(),
            shared_captured.clone(),
            self.recording_state.cursor,
            self.recording_state.shared_cursor.clone(),
            self.recording_state.cursor_overlays.clone(),
            self.active_iframe_sessions.iter().cloned().collect(),
            self.recording_state.contact_sheet_path.clone(),
            self.recording_state.contact_sheet_threshold,
            shared_contact_sheet_count.clone(),
            shared_screenshot_capture.clone(),
            cancel_rx,
        );
        self.recording_state.capture_task = Some(handle);
        self.recording_state.shared_frame_count = Some(shared_count);
        self.recording_state.shared_captured_count = Some(shared_captured);
        self.recording_state.shared_contact_sheet_count = Some(shared_contact_sheet_count);
        self.recording_state.shared_screenshot_capture = Some(shared_screenshot_capture);
        self.recording_state.cancel_tx = Some(cancel_tx);
        Ok(())
    }

    /// Reconcile every externally visible recording flag after startup fails.
    async fn rollback_failed_recording_start(&mut self) {
        self.recording_state.active = false;
        if let Some(ref server) = self.stream_server {
            server.set_recording(false, &self.engine).await;
        }
    }

    pub(super) async fn stop_recording_task(&mut self) -> Result<(), String> {
        recording::stop_recording_task(&mut self.recording_state).await
    }

    pub async fn drain_cdp_events_background(&mut self) -> Result<(), String> {
        let drained = self.drain_cdp_events();
        self.apply_drained_events(drained).await
    }

    pub(super) async fn refresh_active_iframe_sessions(&mut self) {
        let Some(ref browser) = self.browser else {
            self.active_iframe_sessions.clear();
            return;
        };
        let Ok(top_session_id) = browser.active_session_id().map(ToString::to_string) else {
            self.active_iframe_sessions.clear();
            return;
        };
        if self.iframe_sessions.is_empty() {
            self.active_iframe_sessions.clear();
            return;
        }

        let client = browser.client.clone();
        let iframe_sessions = self.iframe_sessions.clone();
        self.active_iframe_sessions =
            a11y::active_iframe_session_ids(&client, &top_session_id, &iframe_sessions)
                .await
                .unwrap_or_default();
    }

    pub(super) async fn apply_drained_events(
        &mut self,
        drained: DrainedEvents,
    ) -> Result<(), String> {
        // Popups and externally closed pages can change the active top-level
        // target without changing iframe topology. Refresh after either kind
        // of event so network capture stays scoped to the active page.
        let active_frame_scope_changed = active_frame_scope_may_have_changed(&drained);
        // ACK screencast frames
        if !drained.pending_acks.is_empty() {
            if let Some(ref browser) = self.browser {
                if let Ok(session_id) = browser.active_session_id() {
                    for ack_sid in drained.pending_acks {
                        let _ = stream::ack_screencast_frame(&browser.client, session_id, ack_sid)
                            .await;
                    }
                }
            }
        }

        // Remove destroyed targets
        for target_id in &drained.destroyed_targets {
            if let Some(ref mut mgr) = self.browser {
                mgr.remove_page_by_target_id(target_id);
            }
        }

        // Track cross-origin iframe sessions
        for (frame_id, iframe_sid) in &drained.attached_iframe_sessions {
            self.iframe_sessions
                .insert(frame_id.clone(), iframe_sid.clone());
            let filter = self.domain_filter.read().await.clone();
            let has_proxy_creds = self.proxy_credentials.read().await.is_some();
            let controls_active = filter.is_some() || has_proxy_creds;
            let setup_result = if let Some(ref mgr) = self.browser {
                bounded_auto_attached_setup("session", iframe_sid, async {
                    mgr.prepare_domains_pub(iframe_sid).await?;
                    let _ = mgr
                        .client
                        .send_command_no_params("DOM.enable", Some(iframe_sid.as_str()))
                        .await;
                    let _ = mgr
                        .client
                        .send_command_no_params("Accessibility.enable", Some(iframe_sid.as_str()))
                        .await;
                    if controls_active {
                        install_network_controls_for_session(
                            &mgr.client,
                            iframe_sid,
                            filter.as_ref(),
                            has_proxy_creds,
                            false,
                        )
                        .await?;
                    }
                    mgr.resume_if_waiting_pub(iframe_sid).await
                })
                .await
            } else {
                Ok(())
            };
            match setup_result {
                Ok(()) => {}
                // Only this session is dropped; other frames keep theirs.
                Err(error) if is_session_detached_error(&error) => {
                    log_detached_during_setup("iframe", &error);
                    self.iframe_sessions.retain(|_, sid| sid != iframe_sid);
                    self.active_iframe_sessions.remove(iframe_sid);
                }
                Err(error) if controls_active => {
                    return close_after_network_control_failure(self, error).await;
                }
                Err(error) => {
                    eprintln!(
                        "Warning: failed to prepare iframe session controls: {}",
                        error
                    );
                    // No controls depend on the pause, so do not leave the
                    // frame stopped before its first script.
                    if let Some(ref mgr) = self.browser {
                        let _ = mgr.resume_if_waiting_pub(iframe_sid).await;
                    }
                }
            }
        }

        // Register top-level pages that browser-level auto-attach paused before
        // their first request. Controls must be installed before resuming.
        for (target_info, page_sid) in &drained.attached_page_sessions {
            let filter = self.domain_filter.read().await.clone();
            let has_proxy_creds = self.proxy_credentials.read().await.is_some();
            let controls_active = filter.is_some() || has_proxy_creds;
            let setup_result = if let Some(ref mut mgr) = self.browser {
                bounded_auto_attached_setup("session", page_sid, async {
                    mgr.prepare_domains_pub(page_sid).await?;
                    if controls_active {
                        install_network_controls_for_session(
                            &mgr.client,
                            page_sid,
                            filter.as_ref(),
                            has_proxy_creds,
                            false,
                        )
                        .await?;
                    }

                    let mut page_url = target_info.url.clone();
                    if let Some(ref filter) = filter {
                        if should_blank_existing_url(&page_url, filter) {
                            let _ = mgr
                                .client
                                .send_command(
                                    "Page.navigate",
                                    Some(json!({ "url": "about:blank" })),
                                    Some(page_sid),
                                )
                                .await;
                            page_url = "about:blank".to_string();
                        }
                    }

                    // This handler drains `Target.attachedToTarget` for a
                    // page that browser-level auto-attach discovered before
                    // its own `Target.targetCreated` was drained (e.g. a
                    // human-opened tab, or a JS-opened popup in the shared
                    // Chrome). Explicit agent commands (`tab new`, `window
                    // new`, `click --new-tab`) already register their own
                    // page via `add_page` on their own path before this
                    // event is ever drained, so this branch never runs for
                    // agent-initiated tabs. `register_discovered_page` is the
                    // single decision point shared with the
                    // `Target.targetCreated` handler below: a pinned session
                    // never activates a discovered target (that would steal
                    // the active tab and overwrite its binding); a legacy
                    // session follows it.
                    mgr.register_discovered_page(
                        &target_info.target_id,
                        page_sid,
                        page_url,
                        target_info.title.clone(),
                        target_info.target_type.clone(),
                    );

                    mgr.resume_if_waiting_pub(page_sid).await
                })
                .await
            } else {
                Ok(())
            };
            match setup_result {
                Ok(()) => {}
                // A popup that closed right away; its target is gone.
                Err(error) if is_session_detached_error(&error) => {
                    log_detached_during_setup("page", &error);
                }
                Err(error) if controls_active => {
                    return close_after_network_control_failure(self, error).await;
                }
                Err(error) => {
                    eprintln!(
                        "Warning: failed to prepare attached page session: {}",
                        error
                    );
                }
            }
        }

        for (target_info, worker_sid) in &drained.attached_worker_sessions {
            let filter = self.domain_filter.read().await.clone();
            let has_proxy_creds = self.proxy_credentials.read().await.is_some();
            let controls_active = filter.is_some() || has_proxy_creds;
            let setup_result = if let Some(ref mgr) = self.browser {
                bounded_auto_attached_setup("session", worker_sid, async {
                    prepare_network_control_target_session(&mgr.client, worker_sid, target_info)
                        .await?;
                    if controls_active {
                        install_worker_network_controls_for_session(
                            &mgr.client,
                            worker_sid,
                            filter.as_ref(),
                            has_proxy_creds,
                            target_info,
                        )
                        .await?;
                    }
                    let _ = mgr
                        .client
                        .send_command_no_wait(
                            "Runtime.runIfWaitingForDebugger",
                            None,
                            Some(worker_sid),
                        )
                        .await;
                    Ok(())
                })
                .await
            } else {
                Ok(())
            };
            match setup_result {
                Ok(()) => {}
                Err(error) if is_session_detached_error(&error) => {
                    log_detached_during_setup("worker", &error);
                }
                Err(error) if controls_active => {
                    return close_after_network_control_failure(self, error).await;
                }
                Err(error) => {
                    eprintln!(
                        "Warning: failed to prepare worker session controls: {}",
                        error
                    );
                }
            }
        }

        for sid in &drained.attached_other_sessions {
            if let Some(ref mgr) = self.browser {
                let _ = mgr.resume_if_waiting_pub(sid).await;
            }
        }

        for sid in &drained.detached_iframe_sessions {
            self.iframe_sessions.retain(|_, v| v != sid);
            self.active_iframe_sessions.remove(sid);
        }

        // Attach and register new targets
        for te in &drained.new_targets {
            let filter = self.domain_filter.read().await.clone();
            let has_proxy_creds = self.proxy_credentials.read().await.is_some();
            let controls_active = filter.is_some() || has_proxy_creds;
            let setup_result = if let Some(ref mut mgr) = self.browser {
                let target_id = te.target_info.target_id.as_str();
                bounded_auto_attached_setup("target", target_id, async {
                    let attach: AttachToTargetResult = mgr
                        .client
                        .send_command_typed(
                            "Target.attachToTarget",
                            &AttachToTargetParams {
                                target_id: te.target_info.target_id.clone(),
                                flatten: true,
                            },
                            None,
                        )
                        .await?;
                    mgr.prepare_domains_pub(&attach.session_id).await?;
                    if controls_active {
                        install_network_controls_for_session(
                            &mgr.client,
                            &attach.session_id,
                            filter.as_ref(),
                            has_proxy_creds,
                            false,
                        )
                        .await?;
                    }

                    let mut page_url = te.target_info.url.clone();
                    if let Some(ref filter) = filter {
                        if should_blank_existing_url(&page_url, filter) {
                            let _ = mgr
                                .client
                                .send_command(
                                    "Page.navigate",
                                    Some(json!({ "url": "about:blank" })),
                                    Some(&attach.session_id),
                                )
                                .await;
                            page_url = "about:blank".to_string();
                        }
                    }

                    // Event-discovered target (e.g. a tab the human opened in the
                    // shared Chrome, or a JS-opened popup): register it via the
                    // same `register_discovered_page` decision point used by the
                    // `Target.attachedToTarget` handler above, which activates it
                    // only for legacy sessions; a pinned session never adopts a
                    // discovered tab (that steal would also overwrite its
                    // binding). Explicit commands (`tab new`, `window new`,
                    // `click --new-tab`) activate via their own paths.
                    mgr.register_discovered_page(
                        &te.target_info.target_id,
                        &attach.session_id,
                        page_url,
                        te.target_info.title.clone(),
                        te.target_info.target_type.clone(),
                    );
                    mgr.resume_if_waiting_pub(&attach.session_id).await
                })
                .await
            } else {
                Ok(())
            };
            match setup_result {
                Ok(()) => {}
                Err(error) if is_session_detached_error(&error) => {
                    log_detached_during_setup("page", &error);
                }
                Err(error) if controls_active => {
                    return close_after_network_control_failure(self, error).await;
                }
                Err(error) => {
                    eprintln!("Warning: failed to prepare new page session: {}", error);
                }
            }
        }

        // Update changed targets
        for te in &drained.changed_targets {
            if let Some(ref mut mgr) = self.browser {
                mgr.update_page_target_info(&te.target_info);
            }
        }

        // Fetch response bodies for HAR entries that just finished loading,
        // while Chrome still has them buffered — bodies are evicted on
        // navigation, so this cannot wait until `har stop`.
        if self.har_recording && !drained.har_finished_requests.is_empty() {
            let mode = self.har_content_mode;
            let mut to_fetch: Vec<(String, Option<String>)> = Vec::new();
            for (request_id, session_id) in &drained.har_finished_requests {
                let Some(entry) = self
                    .har_entries
                    .iter()
                    .rev()
                    .find(|e| &e.request_id == request_id)
                else {
                    continue;
                };
                if entry.response_body.is_some() {
                    continue;
                }
                let wanted = match mode {
                    HarContentMode::None => false,
                    HarContentMode::Text => har_mime_is_text(&entry.mime_type),
                    HarContentMode::All => true,
                };
                if wanted {
                    to_fetch.push((request_id.clone(), session_id.clone()));
                }
            }

            let mut fetched: Vec<(String, String, bool)> = Vec::new();
            if let Some(ref mgr) = self.browser {
                let active_sid = mgr.active_session_id().ok().map(String::from);
                for (request_id, event_sid) in to_fetch {
                    let Some(sid) = event_sid.or_else(|| active_sid.clone()) else {
                        continue;
                    };
                    let Ok(result) = mgr
                        .client
                        .send_command(
                            "Network.getResponseBody",
                            Some(json!({ "requestId": &request_id })),
                            Some(&sid),
                        )
                        .await
                    else {
                        continue;
                    };
                    let base64_encoded = result
                        .get("base64Encoded")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if let Some(body) = result.get("body").and_then(|v| v.as_str()) {
                        if !body.is_empty()
                            && body.len() <= HAR_MAX_BODY_BYTES
                            && self.har_body_total_bytes + body.len() <= HAR_MAX_TOTAL_BODY_BYTES
                        {
                            self.har_body_total_bytes += body.len();
                            fetched.push((request_id, body.to_string(), base64_encoded));
                        }
                    }
                }
            }

            for (request_id, body, base64_encoded) in fetched {
                if let Some(entry) = self
                    .har_entries
                    .iter_mut()
                    .rev()
                    .find(|e| e.request_id == request_id)
                {
                    entry.response_body = Some(body);
                    entry.response_body_base64 = base64_encoded;
                }
            }
        }

        if active_frame_scope_changed {
            self.refresh_active_iframe_sessions().await;
            if self.recording_state.active && self.recording_state.cursor {
                let capture = self
                    .recording_state
                    .capture_session
                    .lock()
                    .ok()
                    .and_then(|capture| capture.as_ref().and_then(|c| c.session_id.clone()));
                if let (Some(browser), Some(capture)) = (self.browser.as_ref(), capture) {
                    if let Ok(sessions) = a11y::active_iframe_session_ids(
                        &browser.client,
                        &capture,
                        &self.iframe_sessions,
                    )
                    .await
                    {
                        let _ = recording::ensure_cursor_overlays(
                            &browser.client,
                            &self.recording_state.cursor_overlays,
                            &sessions.into_iter().collect::<Vec<_>>(),
                        )
                        .await;
                    }
                }
            }
        }

        Ok(())
    }

    pub(super) fn drain_cdp_events(&mut self) -> DrainedEvents {
        let rx = match self.event_rx.as_mut() {
            Some(rx) => rx,
            None => return DrainedEvents::default(),
        };

        let mut pending_acks: Vec<i64> = Vec::new();
        let mut new_targets: Vec<TargetCreatedEvent> = Vec::new();
        let mut new_target_ids: HashSet<String> = HashSet::new();
        let mut changed_targets: Vec<TargetInfoChangedEvent> = Vec::new();
        let mut destroyed_targets: Vec<String> = Vec::new();
        let mut attached_page_sessions: Vec<(TargetInfo, String)> = Vec::new();
        let mut attached_iframe_sessions: Vec<(String, String)> = Vec::new();
        let mut attached_worker_sessions: Vec<(TargetInfo, String)> = Vec::new();
        let mut attached_other_sessions: Vec<String> = Vec::new();
        let mut detached_iframe_sessions: Vec<String> = Vec::new();
        let mut har_finished_requests: Vec<(String, Option<String>)> = Vec::new();

        loop {
            match rx.try_recv() {
                Ok(event) => {
                    if self.downloads.observe(&event.method, &event.params) {
                        continue;
                    }
                    // Target events are not session-scoped; handle them first
                    match event.method.as_str() {
                        "Target.targetCreated" => {
                            if let Ok(te) =
                                serde_json::from_value::<TargetCreatedEvent>(event.params.clone())
                            {
                                if should_track_target(&te.target_info) {
                                    let already_tracked = self
                                        .browser
                                        .as_ref()
                                        .is_none_or(|b| b.has_target(&te.target_info.target_id));
                                    if !already_tracked {
                                        new_target_ids.insert(te.target_info.target_id.clone());
                                        new_targets.push(te);
                                    }
                                }
                            }
                            continue;
                        }
                        "Target.targetInfoChanged" => {
                            if let Ok(te) = serde_json::from_value::<TargetInfoChangedEvent>(
                                event.params.clone(),
                            ) {
                                if should_track_target(&te.target_info) {
                                    // If this target is not yet tracked (e.g. it was
                                    // initially filtered because its URL was
                                    // chrome://newtab/), promote it to a new target
                                    // so it gets attached and added to `pages`.
                                    let already_tracked = self
                                        .browser
                                        .as_ref()
                                        .is_some_and(|b| b.has_target(&te.target_info.target_id));
                                    if already_tracked
                                        || new_target_ids.contains(&te.target_info.target_id)
                                    {
                                        changed_targets.push(te);
                                    } else {
                                        new_target_ids.insert(te.target_info.target_id.clone());
                                        new_targets.push(TargetCreatedEvent {
                                            target_info: te.target_info,
                                        });
                                    }
                                }
                            }
                            continue;
                        }
                        "Target.targetDestroyed" => {
                            if let Ok(te) =
                                serde_json::from_value::<TargetDestroyedEvent>(event.params.clone())
                            {
                                let destroyed_active_target = self
                                    .browser
                                    .as_ref()
                                    .and_then(|browser| browser.active_target_id().ok())
                                    == Some(te.target_id.as_str());
                                if let Some(session_id) = self
                                    .browser
                                    .as_ref()
                                    .and_then(|browser| {
                                        browser.session_id_for_target(&te.target_id)
                                    })
                                    .map(ToString::to_string)
                                {
                                    if destroyed_active_target {
                                        self.webmcp.clear_page_scope(&session_id);
                                    } else {
                                        self.webmcp.clear_page_tools(&session_id);
                                    }
                                }
                                if let Some(session) = self.browser.as_ref().and_then(|browser| {
                                    browser.session_id_for_target(&te.target_id)
                                }) {
                                    self.webmcp.observations.remove(session);
                                }
                                new_targets
                                    .retain(|entry| entry.target_info.target_id != te.target_id);
                                new_target_ids.remove(&te.target_id);
                                attached_page_sessions.retain(|(target_info, _)| {
                                    target_info.target_id != te.target_id
                                });
                                attached_worker_sessions.retain(|(target_info, _)| {
                                    target_info.target_id != te.target_id
                                });
                                attached_iframe_sessions
                                    .retain(|(target_id, _)| target_id != &te.target_id);
                                destroyed_targets.push(te.target_id);
                            }
                            continue;
                        }
                        "Target.attachedToTarget" => {
                            if let (Some(sid), Some(target_info_value)) = (
                                event.params.get("sessionId").and_then(|v| v.as_str()),
                                event.params.get("targetInfo"),
                            ) {
                                match serde_json::from_value::<TargetInfo>(
                                    target_info_value.clone(),
                                ) {
                                    // The recorder's screencast session is
                                    // not a tab and must not have page
                                    // domains enabled on it.
                                    Ok(target_info)
                                        if recording::owns_attachment(
                                            &self.recording_state.capture_session,
                                            &target_info.target_id,
                                            sid,
                                        ) => {}
                                    Ok(target_info) if target_info.target_type == "iframe" => {
                                        // For OOPIF targets, Chrome uses the frameId as
                                        // the targetId, so we can key iframe_sessions by it.
                                        attached_iframe_sessions
                                            .push((target_info.target_id, sid.to_string()));
                                    }
                                    Ok(target_info) if should_track_target(&target_info) => {
                                        attached_page_sessions.push((target_info, sid.to_string()));
                                    }
                                    Ok(target_info) if target_is_worker_like(&target_info) => {
                                        attached_worker_sessions
                                            .push((target_info, sid.to_string()));
                                    }
                                    _ => {
                                        attached_other_sessions.push(sid.to_string());
                                    }
                                }
                            }
                            continue;
                        }
                        "Target.detachedFromTarget" => {
                            if let Some(sid) =
                                event.params.get("sessionId").and_then(|v| v.as_str())
                            {
                                if let Some(frame_id) =
                                    self.iframe_sessions
                                        .iter()
                                        .find_map(|(frame_id, known_sid)| {
                                            (known_sid == sid).then(|| frame_id.clone())
                                        })
                                {
                                    if let Some(ref browser) = self.browser {
                                        if let Ok(session_id) = browser.active_session_id() {
                                            self.webmcp.clear_frame_scope(session_id, &frame_id);
                                        }
                                    }
                                }
                                attached_page_sessions.retain(|(_, known_sid)| known_sid != sid);
                                attached_worker_sessions.retain(|(_, known_sid)| known_sid != sid);
                                attached_iframe_sessions.retain(|(_, known_sid)| known_sid != sid);
                                attached_other_sessions.retain(|known_sid| known_sid != sid);
                                detached_iframe_sessions.push(sid.to_string());
                            }
                            continue;
                        }
                        _ => {}
                    }

                    let session_matches = if let Some(ref browser) = self.browser {
                        event.session_id.as_deref() == browser.active_session_id().ok()
                    } else {
                        false
                    };

                    // Allow Network events from cross-origin iframe sessions
                    // when HAR recording or request tracking is active.
                    let iframe_network_event = !session_matches
                        && is_active_iframe_network_event(
                            &event.method,
                            event.session_id.as_deref(),
                            self.har_recording || self.request_tracking,
                            &self.active_iframe_sessions,
                        );

                    let webmcp_event = event.session_id.as_deref().is_some_and(|sid| {
                        (matches!(
                            event.method.as_str(),
                            "WebMCP.toolsAdded"
                                | "WebMCP.toolsRemoved"
                                | "Page.frameNavigated"
                                | "Page.frameDetached"
                        ) && self
                            .browser
                            .as_ref()
                            .is_some_and(|browser| browser.has_page_session(sid)))
                            || (event.method == "WebMCP.toolResponded"
                                && (session_matches
                                    || self.active_iframe_sessions.contains(sid)
                                    || self.iframe_sessions.values().any(|known| known == sid)))
                    });

                    if !session_matches && !iframe_network_event && !webmcp_event {
                        continue;
                    }

                    match event.method.as_str() {
                        "WebMCP.toolsAdded" => {
                            if let Some(session_id) = event.session_id.as_deref() {
                                self.webmcp
                                    .apply_tools_added(session_id, &event.params, "null");
                            }
                        }
                        "WebMCP.toolsRemoved" => {
                            if let Some(session_id) = event.session_id.as_deref() {
                                self.webmcp.apply_tools_removed(session_id, &event.params);
                            }
                        }
                        "WebMCP.toolResponded" => {
                            self.webmcp.apply_response(event.params.clone());
                        }
                        "Page.frameNavigated" => {
                            if let Some(frame) = event.params.get("frame") {
                                let session_id = event.session_id.as_deref().unwrap_or_default();
                                if frame.get("parentId").is_none() {
                                    if session_matches {
                                        self.webmcp.clear_page_scope(session_id);
                                    } else {
                                        self.webmcp.clear_page_tools(session_id);
                                    }
                                }
                                if let (Some(frame_id), Some(origin)) = (
                                    frame.get("id").and_then(Value::as_str),
                                    webmcp::frame_origin(frame),
                                ) {
                                    self.webmcp
                                        .update_frame_origin(session_id, frame_id, &origin);
                                }
                            }
                        }
                        "Page.frameDetached" => {
                            if let Some(frame_id) =
                                event.params.get("frameId").and_then(Value::as_str)
                            {
                                let session_id = event.session_id.as_deref().unwrap_or_default();
                                self.webmcp.clear_frame_scope(session_id, frame_id);
                            }
                        }
                        "Runtime.consoleAPICalled" => {
                            let level = event
                                .params
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("log");
                            let raw_args: Vec<Value> = event
                                .params
                                .get("args")
                                .and_then(|v| v.as_array())
                                .cloned()
                                .unwrap_or_default();
                            let text = network::format_console_args(&raw_args);
                            if let Some(ref server) = self.stream_server {
                                server.broadcast_console(level, &text, &raw_args);
                            }
                            self.event_tracker.add_console(level, &text, raw_args);
                        }
                        "Runtime.exceptionThrown" => {
                            if let Ok(ex_event) =
                                serde_json::from_value::<ExceptionThrownEvent>(event.params.clone())
                            {
                                let details = &ex_event.exception_details;
                                let text = details
                                    .exception
                                    .as_ref()
                                    .and_then(|e| e.description.as_deref())
                                    .unwrap_or(&details.text);
                                self.event_tracker.add_error(
                                    text,
                                    None,
                                    details.line_number,
                                    details.column_number,
                                );
                                if let Some(ref server) = self.stream_server {
                                    server.broadcast_page_error(
                                        text,
                                        details.line_number,
                                        details.column_number,
                                    );
                                }
                            }
                        }
                        "Network.requestWillBeSent"
                            if self.har_recording || self.request_tracking =>
                        {
                            if let Some(request) = event.params.get("request") {
                                let method = request
                                    .get("method")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("GET")
                                    .to_string();
                                let url = request
                                    .get("url")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let request_id = event
                                    .params
                                    .get("requestId")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if self.har_recording {
                                    let wall_time = event
                                        .params
                                        .get("wallTime")
                                        .and_then(|v| v.as_f64())
                                        .unwrap_or(0.0);
                                    let request_headers =
                                        har_extract_headers(request.get("headers"));
                                    let post_data = request
                                        .get("postData")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);
                                    let request_body_size =
                                        post_data.as_ref().map(|s| s.len() as i64).unwrap_or(0);
                                    let resource_type = event
                                        .params
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Other")
                                        .to_string();
                                    // A redirect re-sends this request id; the hop it
                                    // answers ends here, with the redirect response.
                                    if let Some(redirect) = event.params.get("redirectResponse") {
                                        if let Some(prev) = self
                                            .har_entries
                                            .iter_mut()
                                            .rev()
                                            .find(|e| e.request_id == request_id)
                                        {
                                            har_fill_response(prev, redirect);
                                            prev.loading_finished_timestamp = event
                                                .params
                                                .get("timestamp")
                                                .and_then(|v| v.as_f64())
                                                .or(Some(0.0));
                                        }
                                    }
                                    self.har_entries.push(HarEntry {
                                        request_id: request_id.clone(),
                                        wall_time,
                                        method: method.clone(),
                                        url: url.clone(),
                                        request_headers,
                                        post_data,
                                        request_body_size,
                                        resource_type,
                                        status: None,
                                        status_text: String::new(),
                                        http_version: "HTTP/1.1".to_string(),
                                        response_headers: Vec::new(),
                                        mime_type: String::new(),
                                        redirect_url: String::new(),
                                        response_body_size: -1,
                                        cdp_timing: None,
                                        loading_finished_timestamp: None,
                                        response_body: None,
                                        response_body_base64: false,
                                        remote_addr: None,
                                    });
                                    self.flow.tag(&request_id, wall_time);
                                }
                                if self.request_tracking {
                                    if let Some(redirect) = event.params.get("redirectResponse") {
                                        if let Some(prev) = self
                                            .tracked_requests
                                            .iter_mut()
                                            .rev()
                                            .find(|e| e.request_id == request_id)
                                        {
                                            tracked_fill_response(prev, redirect);
                                        }
                                    }
                                    let headers =
                                        request.get("headers").cloned().unwrap_or(json!({}));
                                    let resource_type = event
                                        .params
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Other")
                                        .to_string();
                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as u64)
                                        .unwrap_or(0);
                                    self.tracked_requests.push(TrackedRequest {
                                        url,
                                        method,
                                        headers,
                                        timestamp,
                                        resource_type,
                                        request_id,
                                        post_data: request
                                            .get("postData")
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                        status: None,
                                        response_headers: None,
                                        mime_type: None,
                                    });
                                }
                            }
                        }
                        "Network.responseReceived"
                            if self.har_recording || self.request_tracking =>
                        {
                            if let Some(response) = event.params.get("response") {
                                let request_id = event
                                    .params
                                    .get("requestId")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if self.har_recording {
                                    self.har_progress
                                        .insert(request_id.to_string(), std::time::Instant::now());
                                    if let Some(entry) = self
                                        .har_entries
                                        .iter_mut()
                                        .rev()
                                        .find(|e| e.request_id == request_id)
                                    {
                                        har_fill_response(entry, response);
                                    }
                                }
                                if self.request_tracking {
                                    if let Some(entry) = self
                                        .tracked_requests
                                        .iter_mut()
                                        .rev()
                                        .find(|e| e.request_id == request_id)
                                    {
                                        tracked_fill_response(entry, response);
                                    }
                                }
                            }
                        }
                        "Network.dataReceived" if self.har_recording => {
                            if let Some(id) = event.params.get("requestId").and_then(|v| v.as_str())
                            {
                                self.har_progress
                                    .insert(id.to_string(), std::time::Instant::now());
                            }
                        }
                        "Network.loadingFinished" if self.har_recording => {
                            let request_id = event
                                .params
                                .get("requestId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let timestamp = event.params.get("timestamp").and_then(|v| v.as_f64());
                            let encoded_data_length = event
                                .params
                                .get("encodedDataLength")
                                .and_then(|v| v.as_i64());
                            if let Some(entry) = self
                                .har_entries
                                .iter_mut()
                                .rev()
                                .find(|e| e.request_id == request_id)
                            {
                                if let Some(ts) = timestamp {
                                    entry.loading_finished_timestamp = Some(ts);
                                }
                                if let Some(len) = encoded_data_length {
                                    entry.response_body_size = len;
                                }
                                if self.har_content_mode != HarContentMode::None {
                                    har_finished_requests
                                        .push((request_id.to_string(), event.session_id.clone()));
                                }
                            }
                        }
                        "Network.loadingFailed" if self.har_recording => {
                            let request_id = event
                                .params
                                .get("requestId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let timestamp = event.params.get("timestamp").and_then(|v| v.as_f64());
                            let error_text = event
                                .params
                                .get("errorText")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Failed");
                            if let Some(entry) = self
                                .har_entries
                                .iter_mut()
                                .rev()
                                .find(|e| e.request_id == request_id)
                            {
                                if entry.status.is_none() {
                                    entry.status = Some(0);
                                    entry.status_text = error_text.to_string();
                                }
                                if let Some(ts) = timestamp {
                                    entry.loading_finished_timestamp = Some(ts);
                                }
                            }
                        }
                        // Frame broadcasting and acks are handled in real-time by the
                        // stream server's background CDP event loop. Here we just
                        // collect acks as a fallback for non-streaming mode.
                        "Page.screencastFrame" if self.stream_server.is_none() => {
                            if let Some(sid) =
                                event.params.get("sessionId").and_then(|v| v.as_i64())
                            {
                                pending_acks.push(sid);
                            }
                        }
                        "Page.javascriptDialogOpening" => {
                            if let Ok(dialog_event) =
                                serde_json::from_value::<JavascriptDialogOpeningEvent>(
                                    event.params.clone(),
                                )
                            {
                                // When auto_dialog is enabled, alert and beforeunload
                                // dialogs are handled by the background dialog_handler_task.
                                // Skip tracking them to avoid a stale warning.
                                let auto_handled = self.auto_dialog
                                    && matches!(
                                        dialog_event.dialog_type.as_str(),
                                        "beforeunload" | "alert"
                                    );
                                if !auto_handled {
                                    self.pending_dialog = Some(PendingDialog {
                                        dialog_type: dialog_event.dialog_type,
                                        message: dialog_event.message,
                                        url: dialog_event.url,
                                        default_prompt: dialog_event.default_prompt,
                                        session_id: event.session_id.clone(),
                                    });
                                }
                            }
                        }
                        "Page.javascriptDialogClosed" => {
                            self.pending_dialog = None;
                        }
                        // Fetch.requestPaused is handled by the background
                        // fetch_handler_task — no need to collect here.
                        _ => {}
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    eprintln!("[frob] Warning: CDP event buffer overflowed, {} events dropped. Network requests may be missing from HAR output.", n);
                    continue;
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.event_rx = None;
                    break;
                }
            }
        }

        let attached_page_target_ids: HashSet<&str> = attached_page_sessions
            .iter()
            .map(|(t, _)| t.target_id.as_str())
            .collect();
        if !attached_page_target_ids.is_empty() {
            new_targets
                .retain(|te| !attached_page_target_ids.contains(te.target_info.target_id.as_str()));
        }

        DrainedEvents {
            pending_acks,
            new_targets,
            changed_targets,
            destroyed_targets,
            attached_page_sessions,
            attached_iframe_sessions,
            attached_worker_sessions,
            attached_other_sessions,
            detached_iframe_sessions,
            har_finished_requests,
        }
    }
}

impl Drop for DaemonState {
    fn drop(&mut self) {
        // The background fetch handler sits in rx.recv().await indefinitely.
        // Without aborting it, the tokio runtime won't shut down (tests hang).
        if let Some(task) = self.fetch_handler_task.take() {
            task.abort();
        }
        if let Some(task) = self.dialog_handler_task.take() {
            task.abort();
        }
    }
}
