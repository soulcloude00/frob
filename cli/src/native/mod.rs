pub mod a11y;
pub mod actions;
pub mod auth;
pub mod browser;
pub mod cdp;
pub mod cookies;
pub mod daemon;
pub mod diff;
pub mod downloads;
pub mod element;
pub mod flow;
pub mod inspect_server;
pub mod interaction;
pub mod network;

pub mod observe;
pub mod policy;
pub mod providers;
pub mod react;
pub mod recording;
pub mod screenshot;
pub mod shared_browser;
pub mod snapshot;
pub mod state;
pub mod storage;
pub mod stream;
pub mod tab_binding;
pub mod tracing;
pub mod webdriver;
pub mod webmcp;

#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod parity_tests;
