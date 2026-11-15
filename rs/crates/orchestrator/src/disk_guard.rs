#![forbid(unsafe_code)]
//! Disk guard (D34): a warning on `open`, never a refusal. The session
//! admission (E1 envelope, E10 open pace) is gone (user 2026-11-15:
//! "retire l'admission"): every `open` goes through at once.

/// >= 60 Go free asked before an ENDURANCE run, never before each dev
/// open: `disk_warning` logs and flags res-stats, nothing is refused.
pub const MIN_FREE_BYTES: u64 = 60 * 1024 * 1024 * 1024;

/// Warning D34, never a refusal: `Some(message)` under the line.
pub fn disk_warning(free_bytes: u64) -> Option<String> {
    if free_bytes < MIN_FREE_BYTES {
        Some(format!(
            "disk-low {}Go < 60Go (D34, endurance runs only, opening anyway)",
            free_bytes / 1_073_741_824
        ))
    } else {
        None
    }
}

/// Free bytes on the filesystem holding `path`: `df` is the only std-only
/// way to read it; unparseable output reads as unbounded (never block
/// opens on a measurement failure).
pub fn free_disk_bytes(path: &std::path::Path) -> u64 {
    let output = std::process::Command::new("df").arg("-B1").arg(path).output();
    let Ok(output) = output else { return u64::MAX };
    if !output.status.success() {
        return u64::MAX;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let second = text.lines().nth(1).unwrap_or("");
    second.split_whitespace().nth(3).and_then(|field| field.parse::<u64>().ok()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    /// The disk line warns, never refuses.
    #[test]
    fn disk_low_warns() {
        assert!(super::disk_warning(1024).is_some(), "1 Ko warns");
        assert!(super::disk_warning(100 * 1024 * 1024 * 1024).is_none(), "100 Go quiet");
    }
}
