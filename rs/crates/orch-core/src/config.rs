#![forbid(unsafe_code)]
//! The daemon's configuration, typed (lot 5.10 of the architecture plan):
//! one struct read ONCE in `main` (`Config::from_env`), printed at startup
//! (`lines`) and handed down to what it configures, instead of each module
//! reading its own environment variable at its own time. Same variables,
//! same defaults as before; a value that does not parse falls back to its
//! default exactly as the scattered reads did.
//!
//! Not in here, on purpose: the paths (`LIFESCAPE_ROOT`, `LIFESCAPE_ENGINE_DIR`,
//! `LIFESCAPE_SAVE_DIR`: `paths`, read by tools that run without a daemon),
//! the packet trace (`LIFESCAPE_PKTLOG`, a worker/SDK switch read once per
//! process) and the debug switches of the build tools.

use std::path::PathBuf;

/// `LIFESCAPE_REDRAW_GATE`: how the goal-redraw gate (lot 3.3) runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedrawGate {
    Off,
    Shadow,
    On,
}

impl RedrawGate {
    /// `on` / `off`, anything else (and unset) is `shadow`.
    pub fn parse(text: &str) -> RedrawGate {
        match text.trim().to_lowercase().as_str() {
            "on" | "1" | "true" => RedrawGate::On,
            "off" | "0" | "false" => RedrawGate::Off,
            _ => RedrawGate::Shadow,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RedrawGate::Off => "off",
            RedrawGate::Shadow => "shadow",
            RedrawGate::On => "on",
        }
    }
}

/// Default game server address (the relay of the local container).
pub const DEFAULT_SERVER_HOST: &str = "127.0.0.1";
pub const DEFAULT_SERVER_PORT: u16 = 43594;
/// Default port of the dashboard.
pub const DEFAULT_WEB_PORT: u16 = 8765;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `LIFESCAPE_SERVER_HOST`: where the workers log in.
    pub server_host: String,
    /// `LIFESCAPE_SERVER_PORT`.
    pub server_port: u16,
    /// `LIFESCAPE_WEB_PORT`: the dashboard.
    pub web_port: u16,
    /// `LIFESCAPE_REDRAW_GATE`.
    pub redraw_gate: RedrawGate,
    /// `LIFESCAPE_RATES_PATH`: the rates snapshot file (default under `run/`).
    pub rates_path: Option<PathBuf>,
    /// The variables that were set (the rest are defaults), for `lines`.
    set: Vec<&'static str>,
}

impl Default for Config {
    fn default() -> Self {
        Config::from_lookup(&|_| None)
    }
}

impl Config {
    /// Reads the process environment (once, in `main`).
    pub fn from_env() -> Config {
        Config::from_lookup(&|name| std::env::var(name).ok())
    }

    /// `from_env` over any source of variables (tests).
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Config {
        let mut set = Vec::new();
        let mut get = |name: &'static str| {
            let value = lookup(name);
            if value.is_some() {
                set.push(name);
            }
            value
        };
        let server_host = get("LIFESCAPE_SERVER_HOST").unwrap_or_else(|| DEFAULT_SERVER_HOST.to_string());
        let server_port = get("LIFESCAPE_SERVER_PORT").and_then(|raw| raw.parse().ok()).unwrap_or(DEFAULT_SERVER_PORT);
        let web_port = get("LIFESCAPE_WEB_PORT").and_then(|raw| raw.parse().ok()).unwrap_or(DEFAULT_WEB_PORT);
        let redraw_gate = RedrawGate::parse(&get("LIFESCAPE_REDRAW_GATE").unwrap_or_default());
        let rates_path = get("LIFESCAPE_RATES_PATH").filter(|path| !path.is_empty()).map(PathBuf::from);
        Config { server_host, server_port, web_port, redraw_gate, rates_path, set }
    }

    fn source(&self, name: &str) -> &'static str {
        if self.set.contains(&name) { "env" } else { "default" }
    }

    /// The startup block: one line per setting, its value and where it came from.
    pub fn lines(&self) -> Vec<String> {
        let rates = self.rates_path.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "run/rates-snapshot.json".to_string());
        vec![
            format!("config LIFESCAPE_SERVER_HOST={} ({})", self.server_host, self.source("LIFESCAPE_SERVER_HOST")),
            format!("config LIFESCAPE_SERVER_PORT={} ({})", self.server_port, self.source("LIFESCAPE_SERVER_PORT")),
            format!("config LIFESCAPE_WEB_PORT={} ({})", self.web_port, self.source("LIFESCAPE_WEB_PORT")),
            format!("config LIFESCAPE_REDRAW_GATE={} ({})", self.redraw_gate.label(), self.source("LIFESCAPE_REDRAW_GATE")),
            format!("config LIFESCAPE_RATES_PATH={} ({})", rates, self.source("LIFESCAPE_RATES_PATH")),
        ]
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
