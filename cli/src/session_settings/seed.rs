//! Temporary shim: the one place the module reads `FROB_*`.
//!
//! The CLI still hands settings to the daemon as environment variables, so
//! the daemon seeds `Running` from them once at startup. When the patch rides
//! on each command this file goes away.

use std::env;

use serde_json::Value;

use super::running::Running;
use super::spec::Key;

fn var(name: &str) -> Option<String> {
    env::var(name).ok()
}

fn is_one_or_true(raw: &str) -> bool {
    raw == "1" || raw == "true"
}

fn is_one_true_or_yes(raw: &str) -> bool {
    matches!(raw, "1" | "true" | "yes")
}

fn split_lines_and_commas(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

impl Running {
    pub fn from_env() -> Running {
        Running::from_lookup(var)
    }

    /// Seeds from any name-to-value lookup; `from_env` passes the process
    /// environment.
    pub(super) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Running {
        let mut r = Running::defaults();

        // Cdp, CdpHeaders, AutoConnect and Provider are not seeded: the CLI
        // resends them on every command, so an attach commits them on success
        // and a failed attach rolls back instead of becoming the session's target.

        let strings = [
            (Key::ExecutablePath, "FROB_EXECUTABLE_PATH"),
            (Key::Proxy, "FROB_PROXY"),
            (Key::ProxyBypass, "FROB_PROXY_BYPASS"),
            (Key::ProxyUsername, "FROB_PROXY_USERNAME"),
            (Key::ProxyPassword, "FROB_PROXY_PASSWORD"),
            (Key::Profile, "FROB_PROFILE"),
            (Key::StorageState, "FROB_STATE"),
            (Key::UserAgent, "FROB_USER_AGENT"),
            (Key::ColorScheme, "FROB_COLOR_SCHEME"),
            (Key::DownloadPath, "FROB_DOWNLOAD_PATH"),
            (Key::Engine, "FROB_ENGINE"),
            (Key::IdleTimeout, "FROB_IDLE_TIMEOUT_MS"),
            (Key::RestoreKey, "FROB_SESSION_NAME"),
            (Key::RestoreSave, "FROB_RESTORE_SAVE"),
            (Key::RestoreCheckUrl, "FROB_RESTORE_CHECK_URL"),
            (Key::RestoreCheckText, "FROB_RESTORE_CHECK_TEXT"),
            (Key::RestoreCheckFn, "FROB_RESTORE_CHECK_FN"),
            (Key::ConfirmActions, "FROB_CONFIRM_ACTIONS"),
            (Key::Device, "FROB_IOS_DEVICE"),
        ];
        for (key, name) in strings {
            if let Some(v) = lookup(name) {
                r.set(key, Value::from(v));
            }
        }

        // The legacy name applies only when the current one is unset.
        if let Some(path) = lookup("FROB_ACTION_POLICY").or_else(|| lookup("FROB_POLICY")) {
            // Stored with the session, so it must not depend on the directory
            // a later daemon starts in.
            let path = match env::current_dir() {
                Ok(cwd) => crate::flags::absolute_policy_path(&path, &cwd).unwrap_or(path),
                Err(_) => path,
            };
            r.set(Key::ActionPolicy, Value::from(path));
        }

        if let Some(v) = lookup("FROB_HEADED") {
            r.provider_headed =
                !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "");
        }

        // `1` or `true`.
        let flags = [
            (Key::Headed, "FROB_HEADED"),
            (Key::AllowFileAccess, "FROB_ALLOW_FILE_ACCESS"),
            (Key::IgnoreHttpsErrors, "FROB_IGNORE_HTTPS_ERRORS"),
            (Key::Webgpu, "FROB_WEBGPU"),
            (Key::NoXvfb, "FROB_NO_XVFB"),
            (Key::Debug, "FROB_DEBUG"),
        ];
        for (key, name) in flags {
            if let Some(v) = lookup(name) {
                r.set(key, Value::Bool(is_one_or_true(&v)));
            }
        }

        // `1`, `true` or `yes`.
        if let Some(v) = lookup("FROB_PIN_TAB") {
            r.set(Key::PinTab, Value::Bool(is_one_true_or_yes(&v)));
        }
        if let Some(v) = lookup("FROB_NO_AUTO_DIALOG") {
            r.set(Key::NoAutoDialog, Value::Bool(is_one_true_or_yes(&v)));
        }
        if let Some(v) = lookup("FROB_NO_WEBMCP") {
            r.set(Key::Webmcp, Value::Bool(!is_one_true_or_yes(&v)));
        }

        // Anything but `0`, `false`, `no` or empty hides; unset hides.
        if let Some(v) = lookup("FROB_HIDE_SCROLLBARS") {
            let off = matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "");
            r.set(Key::HideScrollbars, Value::Bool(!off));
        }

        if let Some(v) = lookup("FROB_DEFAULT_TIMEOUT").and_then(|s| s.parse::<u64>().ok()) {
            r.set(Key::DefaultTimeout, Value::from(v));
        }

        // The plugin registry the CLI handed the daemon; an empty one sets nothing.
        if let Some(plugins) = lookup("FROB_PLUGINS")
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        {
            r.set(Key::Plugins, plugins);
        }

        if let Some(v) = lookup("FROB_ARGS") {
            r.set(Key::Args, Value::from(crate::flags::split_launch_args(&v)));
        }
        if let Some(v) = lookup("FROB_EXTENSIONS") {
            r.set(Key::Extensions, Value::from(split_lines_and_commas(&v)));
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                r.extensions_file = Some(trimmed.to_string());
            }
        }
        if let Some(v) = lookup("FROB_ENABLE") {
            r.set(Key::Enable, Value::from(split_lines_and_commas(&v)));
        }
        if let Some(v) = lookup("FROB_INIT_SCRIPTS") {
            r.set(Key::InitScripts, Value::from(split_lines_and_commas(&v)));
        }

        // A blank list sets nothing. A list with only separators seeds an empty
        // list; the domain filter built from it is empty and allows every host.
        if let Some(v) = lookup("FROB_ALLOWED_DOMAINS").filter(|s| !s.trim().is_empty()) {
            let domains: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            r.set(Key::AllowedDomains, Value::from(domains));
        }

        r
    }
}
