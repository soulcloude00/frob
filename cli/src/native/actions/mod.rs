use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::sync::{broadcast, oneshot, RwLock};

use crate::connection::{get_socket_dir, INTERNAL_DAEMON_SHUTDOWN_ACTION};
use crate::session_files::SessionFile;
use crate::session_settings::{
    named, spec, Connection, Effect, Key, Patch, Persist, Requested, Running, Store,
};
use crate::validation::{is_valid_session_name, session_name_error};

use super::a11y;
use super::auth;
use super::browser::{should_track_target, BrowserManager, WaitUntil, WedgeReport};
use super::cdp::chrome::{prepare_nss_home, LaunchOptions};
use super::cdp::client::{is_session_detached_error, CdpClient};
use super::cdp::types::{
    AttachToTargetParams, AttachToTargetResult, CdpEvent, DispatchMouseEventParams,
    ExceptionThrownEvent, GetFullAXTreeResult, JavascriptDialogOpeningEvent, TargetCreatedEvent,
    TargetDestroyedEvent, TargetInfo, TargetInfoChangedEvent,
};
use super::cookies;
use super::diff;
use super::downloads::{DownloadLedger, DownloadState};
use super::element::RefMap;
use super::inspect_server::InspectServer;
use super::interaction;
use super::network::{self, DomainFilter, EventTracker};
use super::observe;
use super::policy::{ActionPolicy, ConfirmActions, PolicyResult};
use super::providers;
use super::react;
use super::recording::{self, RecordingState};
use super::screenshot::{self, ScreenshotOptions};
use super::snapshot::{self, SnapshotOptions};
use super::state;
use super::storage;
use super::stream::{self, IdleActivity, StreamServer};
use super::tab_binding;
use super::tracing::{self as native_tracing, TracingState};
use super::webdriver::appium::AppiumManager;
use super::webdriver::backend::{BrowserBackend, WebDriverBackend, WEBDRIVER_UNSUPPORTED_ACTIONS};
use super::webdriver::ios;
use super::webdriver::safari;
use super::webmcp;

mod auth_login;
mod capture;
mod daemon_state;
mod devtools;
mod dispatch;
mod download;
mod emulation;
mod flow_commands;
mod har;
mod input;
mod launch;
mod locators;
mod media;
mod navigation;
mod page_scripts;
mod page_state;
mod query;
mod response;
mod routes;
mod session_setup;
mod settings;
mod streaming;
mod tabs;
mod types;
mod waits;
mod webmcp_tools;

use auth_login::*;
use capture::*;
pub use daemon_state::*;
use devtools::*;
pub use dispatch::*;
use download::*;
use emulation::*;
use flow_commands::*;
use har::*;
use input::*;
pub(crate) use launch::*;
pub(crate) use locators::*;
use media::*;
use navigation::*;
use page_scripts::*;
use page_state::*;
use query::*;
use response::*;
pub use routes::*;
use session_setup::*;
use settings::*;
use streaming::*;
use tabs::*;
pub use types::*;
use waits::*;
use webmcp_tools::*;

#[cfg(test)]
mod tests;
