use std::collections::HashMap;

use super::{Config, RedrawGate, DEFAULT_SERVER_HOST, DEFAULT_SERVER_PORT, DEFAULT_WEB_PORT};

fn from(pairs: &[(&str, &str)]) -> Config {
    let table: HashMap<String, String> = pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
    Config::from_lookup(&|name| table.get(name).cloned())
}

/// The defaults are the constants the scattered reads carried before.
#[test]
fn the_defaults_are_todays_constants() {
    let config = Config::default();
    assert_eq!((config.server_host.as_str(), config.server_port, config.web_port), ("127.0.0.1", 43594, 8765));
    assert_eq!((DEFAULT_SERVER_HOST, DEFAULT_SERVER_PORT, DEFAULT_WEB_PORT), ("127.0.0.1", 43594, 8765));
    assert_eq!(config.redraw_gate, RedrawGate::Shadow);
    assert_eq!(config.rates_path, None);
    assert_eq!(
        config.lines(),
        vec![
            "config LIFESCAPE_SERVER_HOST=127.0.0.1 (default)",
            "config LIFESCAPE_SERVER_PORT=43594 (default)",
            "config LIFESCAPE_WEB_PORT=8765 (default)",
            "config LIFESCAPE_REDRAW_GATE=shadow (default)",
            "config LIFESCAPE_RATES_PATH=run/rates-snapshot.json (default)",
        ]
    );
}

#[test]
fn a_set_variable_wins_and_says_so() {
    let config = from(&[("LIFESCAPE_SERVER_HOST", "172.18.0.3"), ("LIFESCAPE_SERVER_PORT", "43600"), ("LIFESCAPE_WEB_PORT", "9000"), ("LIFESCAPE_REDRAW_GATE", "on"), ("LIFESCAPE_RATES_PATH", "/tmp/copy.json")]);
    assert_eq!((config.server_host.as_str(), config.server_port, config.web_port, config.redraw_gate), ("172.18.0.3", 43600, 9000, RedrawGate::On));
    assert!(config.lines().iter().all(|line| line.ends_with("(env)")), "{:?}", config.lines());
    assert_eq!(config.rates_path, Some(std::path::PathBuf::from("/tmp/copy.json")));
}

#[test]
fn a_value_that_does_not_parse_falls_back_like_the_old_reads() {
    let config = from(&[("LIFESCAPE_SERVER_PORT", "not-a-port"), ("LIFESCAPE_WEB_PORT", "99999"), ("LIFESCAPE_REDRAW_GATE", "banana"), ("LIFESCAPE_RATES_PATH", "")]);
    assert_eq!((config.server_port, config.web_port, config.redraw_gate, config.rates_path), (43594, 8765, RedrawGate::Shadow, None));
}

#[test]
fn the_redraw_gate_words() {
    for (text, gate) in [("on", RedrawGate::On), ("1", RedrawGate::On), ("TRUE", RedrawGate::On), (" off ", RedrawGate::Off), ("0", RedrawGate::Off), ("false", RedrawGate::Off), ("shadow", RedrawGate::Shadow), ("", RedrawGate::Shadow)] {
        assert_eq!(RedrawGate::parse(text), gate, "{text:?}");
    }
}
