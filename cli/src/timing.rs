//! Opt-in phase timing for benchmarks: with `FROB_TIMING=/path/to/log`, each `mark` appends
//! `<unix ms> <pid> <label>` to that file. The CLI and the daemon both write to it, so one log
//! shows where a command's time went across both processes. Without the variable it does nothing.

use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

fn path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| std::env::var("FROB_TIMING").ok().filter(|p| !p.is_empty()))
        .as_deref()
}

pub fn mark(label: &str) {
    let Some(path) = path() else { return };
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{ms} {} {label}", std::process::id());
    }
}
