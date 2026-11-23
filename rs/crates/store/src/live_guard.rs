//! Tripwire against tests touching the live data directory.
//!
//! Active only with the `test-isolation` feature (dev-dependencies, never
//! the shipped binary): any path inside the real `<repo>/run/` panics.
//! The repo is found from this crate's manifest dir (the first ancestor
//! holding an `orchestration` dir), so a test that points `LIFESCAPE_ROOT`
//! at a copy is fine and one that points it at the repo is caught.

use std::path::{Component, Path, PathBuf};

/// The real `<repo>/run/` of the checkout this crate was compiled in.
pub fn live_run_dir() -> Option<PathBuf> {
    let mut dir = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    loop {
        if dir.join("orchestration").is_dir() {
            return Some(dir.join("run"));
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn normalized(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    // Resolve symlinks through the deepest existing ancestor.
    let mut tail = Vec::new();
    let mut probe = out.clone();
    loop {
        if let Ok(real) = probe.canonicalize() {
            let mut full = real;
            for part in tail.iter().rev() {
                full.push(part);
            }
            return full;
        }
        match probe.file_name().map(|name| name.to_os_string()) {
            Some(name) => {
                tail.push(name);
                probe.pop();
            }
            None => return out,
        }
    }
}

/// Panics when `path` lies inside the real `run/` (test-isolation builds
/// only; a no-op otherwise).
pub fn assert_not_live(path: &Path) {
    if !cfg!(any(test, feature = "test-isolation")) {
        return;
    }
    let Some(live) = live_run_dir() else { return };
    let live = normalized(&live);
    if normalized(path).starts_with(&live) {
        panic!("test isolation: {} is inside the LIVE data directory {}; tests must use a temp root (LIFESCAPE_ROOT=<copy>)", path.display(), live.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "LIVE data directory")]
    fn the_live_run_dir_trips() {
        assert_not_live(&live_run_dir().expect("repo").join("lifescape.db"));
    }

    #[test]
    fn a_temp_path_passes() {
        assert_not_live(&std::env::temp_dir().join("x.db"));
    }
}
