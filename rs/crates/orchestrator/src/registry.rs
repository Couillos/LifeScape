//! Scripts the orchestrator may launch.
//!
//! Single source: `lifescape_scripts::names()`, dispatched by
//! `lifescape_scripts::run`. Adding a script means adding it there.

/// Every launchable script name, sorted.
pub fn script_names_sorted() -> Vec<&'static str> {
    let mut names = lifescape_scripts::names();
    names.sort_unstable();
    names
}

/// True when the scripts crate dispatches this name.
pub fn script_known(name: &str) -> bool {
    lifescape_scripts::names().iter().any(|known| *known == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown() {
        assert!(script_known("fishing"));
        assert!(!script_known("nope"));
        assert!(script_names_sorted().windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
