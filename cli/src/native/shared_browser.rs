//! One Chrome shared by every `--shared-browser` session.
//!
//! Each session's daemon works in its own browser context, so sessions stay
//! isolated while the browser, GPU, and network processes are paid for once.
//! No daemon owns the Chrome: a registry directory records it and the daemons
//! using it, and whichever daemon leaves last stops it.
//!
//! ```text
//! <socket dir>/shared-browser/
//!   lock            flock held while the registry is read or changed
//!   browser.json    pid, process group, CDP URL, and profile dir of the Chrome
//!   members/<pid>-<n>   one empty file per session using the Chrome
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A launched Chrome that no `ChromeProcess` owns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SharedChrome {
    pub pid: u32,
    pub pgid: i32,
    pub ws_url: String,
    pub user_data_dir: PathBuf,
}

/// This daemon's claim on the shared Chrome. Dropping it gives the claim up,
/// and stops the Chrome when no other daemon holds one.
#[derive(Debug)]
pub struct Membership {
    dir: PathBuf,
    /// `<pid>-<n>`: the daemon's pid, so a dead daemon's claim can be told
    /// apart, and a counter, so one process can hold several claims.
    name: String,
}

impl Drop for Membership {
    fn drop(&mut self) {
        release(&self.dir, &self.name);
    }
}

fn next_member_name(pid: u32) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{pid}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn registry_dir() -> PathBuf {
    crate::connection::get_socket_dir().join("shared-browser")
}

/// Joins the shared Chrome, calling `launch` to start it when none is running.
pub fn acquire(
    launch: impl FnOnce() -> Result<SharedChrome, String>,
) -> Result<(String, Membership), String> {
    acquire_in(&registry_dir(), std::process::id(), launch)
}

#[cfg(unix)]
fn acquire_in(
    dir: &Path,
    pid: u32,
    launch: impl FnOnce() -> Result<SharedChrome, String>,
) -> Result<(String, Membership), String> {
    let members = dir.join("members");
    fs::create_dir_all(&members)
        .map_err(|e| format!("Failed to create shared browser registry: {e}"))?;
    let _lock = lock(dir)?;

    let chrome = match read_browser(dir) {
        Some(chrome) if accepts_connections(&chrome.ws_url) => chrome,
        stale => {
            // A Chrome that stopped answering takes its sessions with it.
            if let Some(chrome) = stale {
                stop(&chrome);
            }
            clear_members(&members);
            let chrome = launch()?;
            let json = serde_json::to_vec(&chrome).map_err(|e| e.to_string())?;
            if let Err(e) = fs::write(dir.join("browser.json"), json) {
                stop(&chrome);
                return Err(format!("Failed to record the shared browser: {e}"));
            }
            chrome
        }
    };

    let name = next_member_name(pid);
    fs::write(members.join(&name), b"")
        .map_err(|e| format!("Failed to join the shared browser: {e}"))?;
    Ok((
        chrome.ws_url,
        Membership {
            dir: dir.to_path_buf(),
            name,
        },
    ))
}

#[cfg(not(unix))]
fn acquire_in(
    _dir: &Path,
    _pid: u32,
    _launch: impl FnOnce() -> Result<SharedChrome, String>,
) -> Result<(String, Membership), String> {
    Err("--shared-browser is not supported on Windows".to_string())
}

#[cfg(unix)]
fn release(dir: &Path, name: &str) {
    let Ok(_lock) = lock(dir) else { return };
    let members = dir.join("members");
    let _ = fs::remove_file(members.join(name));
    if live_members(&members) > 0 {
        return;
    }
    if let Some(chrome) = read_browser(dir) {
        stop(&chrome);
    }
    let _ = fs::remove_file(dir.join("browser.json"));
}

#[cfg(not(unix))]
fn release(_dir: &Path, _name: &str) {}

fn read_browser(dir: &Path) -> Option<SharedChrome> {
    serde_json::from_slice(&fs::read(dir.join("browser.json")).ok()?).ok()
}

fn clear_members(members: &Path) {
    for entry in fs::read_dir(members).into_iter().flatten().flatten() {
        let _ = fs::remove_file(entry.path());
    }
}

/// Counts member files whose daemon is still running, removing the rest.
#[cfg(unix)]
fn live_members(members: &Path) -> usize {
    let mut live = 0;
    for entry in fs::read_dir(members).into_iter().flatten().flatten() {
        let pid = entry
            .file_name()
            .to_str()
            .and_then(|name| name.split('-').next()?.parse::<i32>().ok());
        match pid {
            Some(pid) if process_exists(pid) => live += 1,
            _ => {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    live
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    let signalled = unsafe { libc::kill(pid, 0) } == 0;
    // EPERM means the process exists but belongs to another user.
    signalled || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether Chrome's DevTools port still accepts connections. A pid check is
/// not enough: a Chrome killed by another daemon stays a zombie, and so
/// "exists", until the daemon that spawned it exits.
fn accepts_connections(ws_url: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let Some(address) = url::Url::parse(ws_url)
        .ok()
        .and_then(|u| Some(format!("{}:{}", u.host_str()?, u.port()?)))
    else {
        return false;
    };
    address
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .is_some_and(|addr| {
            TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
        })
}

/// Kills the Chrome process group and removes its profile directory.
#[cfg(unix)]
fn stop(chrome: &SharedChrome) {
    // The pid comes from a file that can outlive the Chrome, and pids are
    // reused, so only signal a process that is still that Chrome.
    if runs_with_profile(chrome.pid, &chrome.user_data_dir) {
        unsafe {
            libc::kill(-chrome.pgid, libc::SIGKILL);
        }
        for _ in 0..40 {
            if !runs_with_profile(chrome.pid, &chrome.user_data_dir) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    let _ = fs::remove_dir_all(&chrome.user_data_dir);
}

#[cfg(unix)]
fn runs_with_profile(pid: u32, user_data_dir: &Path) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout).contains(&*user_data_dir.to_string_lossy())
        })
}

#[cfg(unix)]
fn lock(dir: &Path) -> Result<fs::File, String> {
    use std::os::unix::io::AsRawFd;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("lock"))
        .map_err(|e| format!("Failed to open the shared browser lock: {e}"))?;
    // Released when the file is closed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!(
            "Failed to lock the shared browser registry: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(file)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A listening port stands in for Chrome's DevTools endpoint.
    fn fake_chrome(dir: &Path, listener: &TcpListener) -> SharedChrome {
        SharedChrome {
            // No process has this pid, so `stop` never signals anything.
            pid: u32::MAX,
            pgid: i32::MAX,
            ws_url: format!(
                "ws://127.0.0.1:{}/devtools/browser/x",
                listener.local_addr().unwrap().port()
            ),
            user_data_dir: dir.join("profile"),
        }
    }

    #[test]
    fn second_session_joins_the_running_chrome() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let launches = AtomicUsize::new(0);
        let launch = || {
            launches.fetch_add(1, Ordering::SeqCst);
            Ok(fake_chrome(dir.path(), &listener))
        };
        let me = std::process::id();

        let (first_url, first) = acquire_in(dir.path(), me, launch).unwrap();
        let (second_url, second) = acquire_in(dir.path(), me, launch).unwrap();

        assert_eq!(first_url, second_url);
        assert_eq!(launches.load(Ordering::SeqCst), 1);

        // Two claims from one process are separate: the Chrome stays
        // recorded until both are gone.
        drop(first);
        assert!(dir.path().join("browser.json").exists());
        drop(second);
        assert!(!dir.path().join("browser.json").exists());
    }

    #[test]
    fn chrome_is_forgotten_only_when_the_last_live_member_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let chrome = fake_chrome(dir.path(), &listener);
        fs::create_dir_all(&chrome.user_data_dir).unwrap();
        let (_, membership) = acquire_in(dir.path(), std::process::id(), || Ok(chrome)).unwrap();
        // Another live daemon (pid 1 always exists) and one that died.
        fs::write(dir.path().join("members/1-0"), b"").unwrap();
        fs::write(dir.path().join("members/2147483646-0"), b"").unwrap();

        drop(membership);
        assert!(dir.path().join("browser.json").exists());
        assert!(!dir.path().join("members/2147483646-0").exists());

        release(dir.path(), "1-0");
        assert!(!dir.path().join("browser.json").exists());
        assert!(!dir.path().join("profile").exists());
    }

    #[test]
    fn a_chrome_that_stopped_answering_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let dead = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            fake_chrome(dir.path(), &listener)
        };
        let me = std::process::id();
        let (dead_url, old) = acquire_in(dir.path(), me, || Ok(dead)).unwrap();
        std::mem::forget(old);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let (url, _membership) =
            acquire_in(dir.path(), me, || Ok(fake_chrome(dir.path(), &listener))).unwrap();

        assert_ne!(url, dead_url);
    }
}
