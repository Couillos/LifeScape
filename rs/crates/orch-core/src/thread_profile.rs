#![forbid(unsafe_code)]
//! Per-minute `cpu-profile threads` line (plan d'architecture, lot 0.2):
//! the CPU of EVERY thread of the daemon, grouped by thread name (the
//! trailing number trimmed: `brain-tick-3` counts in `brain-tick`), so
//! nothing of the process stays unattributed. One `thread-profile`
//! thread, one `/proc/self/task` scan a minute.
//!
//! Attribution is by thread id: the CPU a live thread used during the
//! minute lands in its group; the rest of the process total (utime +
//! stime of `/proc/self/stat`) is what threads that ended inside the
//! minute used (`exited`: the per-call `worker-connect` / `session-conn`
//! threads, ...). So `sum(groups) + exited = total` always.
//! Unnamed threads (the main thread included) share the process name,
//! `orchestrator`.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// Summary period (same as the other `cpu-profile` lines).
pub const THREAD_WINDOW: Duration = Duration::from_secs(60);
/// Kernel USER_HZ for `/proc/self/stat` times.
const USER_HZ_NS: u64 = 10_000_000;

/// One reading: per thread id (group, CPU ns), and the process total ns.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Reading {
    pub threads: HashMap<u64, (String, u64)>,
    pub process_ns: u64,
}

/// Group of a thread name: trailing digits, `-` and `_` trimmed.
pub fn group_of(name: &str) -> String {
    let trimmed = name.trim().trim_end_matches(|ch: char| ch.is_ascii_digit() || ch == '-' || ch == '_');
    if trimmed.is_empty() { name.trim().to_string() } else { trimmed.to_string() }
}

/// utime + stime of a `/proc/<pid>/stat` text, in ns (fields counted
/// after the closing paren of the command name).
pub fn parse_process_ns(text: &str) -> Option<u64> {
    let after = &text[text.rfind(')')? + 1..];
    let mut fields = after.split_whitespace().skip(11);
    let user: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some((user + system).saturating_mul(USER_HZ_NS))
}

/// Reads every thread of this process.
pub fn read_all() -> Reading {
    let mut reading = Reading::default();
    reading.process_ns = std::fs::read_to_string("/proc/self/stat").ok().and_then(|text| parse_process_ns(&text)).unwrap_or(0);
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return reading };
    for task in tasks.flatten() {
        let Some(tid) = task.file_name().to_str().and_then(|name| name.parse::<u64>().ok()) else { continue };
        let comm = std::fs::read_to_string(task.path().join("comm")).unwrap_or_default();
        let nanos = std::fs::read_to_string(task.path().join("schedstat"))
            .ok()
            .and_then(|text| text.split_whitespace().next().and_then(|field| field.parse::<u64>().ok()))
            .unwrap_or(0);
        reading.threads.insert(tid, (group_of(&comm), nanos));
    }
    reading
}

/// One summary line from two readings (pure: tests feed it directly).
/// Groups by CPU descending (name on a tie), `xN` = live threads.
pub fn threads_line(span: Duration, before: &Reading, after: &Reading) -> String {
    let mut groups: BTreeMap<&str, (u64, usize)> = BTreeMap::new();
    let mut attributed = 0u64;
    for (tid, (group, nanos)) in &after.threads {
        let base = before.threads.get(tid).map_or(0, |(_, nanos)| *nanos);
        let used = nanos.saturating_sub(base);
        attributed += used;
        let entry = groups.entry(group.as_str()).or_insert((0, 0));
        entry.0 += used;
        entry.1 += 1;
    }
    let total = after.process_ns.saturating_sub(before.process_ns);
    let exited = total.saturating_sub(attributed);
    let mut rows: Vec<(&str, (u64, usize))> = groups.into_iter().collect();
    rows.sort_by(|left, right| right.1.0.cmp(&left.1.0).then_with(|| left.0.cmp(right.0)));
    let mut line = format!("cpu-profile threads ({:.0}s): cpu_ms", span.as_secs_f64());
    for (group, (nanos, count)) in rows {
        line.push_str(&format!(" {group}={:.0}x{count}", nanos as f64 / 1e6));
    }
    line.push_str(&format!(" exited={:.0} total={:.0} threads={}", exited as f64 / 1e6, total as f64 / 1e6, after.threads.len()));
    line
}

/// Starts the `thread-profile` thread (daemon only).
pub fn start() {
    let spawned = std::thread::Builder::new().name("thread-profile".to_string()).spawn(|| {
        let mut before = read_all();
        let mut started = Instant::now();
        loop {
            // Gauge period, not a game wait.
            std::thread::sleep(THREAD_WINDOW);
            let after = read_all();
            crate::logln!("{}", threads_line(started.elapsed(), &before, &after));
            (before, started) = (after, Instant::now());
        }
    });
    if let Err(error) = spawned {
        eprintln!("thread-profile thread spawn failed ({error}): no cpu-profile threads line");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(rows: &[(u64, &str, u64)], process_ns: u64) -> Reading {
        Reading { threads: rows.iter().map(|(tid, group, nanos)| (*tid, (group.to_string(), *nanos))).collect(), process_ns }
    }

    #[test]
    fn groups_trim_the_trailing_number() {
        assert_eq!(group_of("brain-tick-3\n"), "brain-tick");
        assert_eq!(group_of("plan-pool-12"), "plan-pool");
        assert_eq!(group_of("push-read"), "push-read");
        assert_eq!(group_of("orchestrator"), "orchestrator");
    }

    #[test]
    fn process_time_reads_after_the_command_paren() {
        let stat = "123 (orch estrator) S 1 2 3 4 5 6 7 8 9 10 250 150 0 0 20";
        assert_eq!(parse_process_ns(stat), Some(400 * USER_HZ_NS));
    }

    /// Everything is attributed: groups plus `exited` add up to the
    /// process total, a new thread counts from zero, a gone one only
    /// shows in `exited`.
    #[test]
    fn groups_and_exited_add_up_to_the_total() {
        let before = reading(&[(1, "brain-tick", 1_000_000_000), (2, "brain-tick", 0), (3, "push-read", 5_000_000), (9, "worker-connect", 10_000_000)], 2_000_000_000);
        let after = reading(&[(1, "brain-tick", 1_500_000_000), (2, "brain-tick", 300_000_000), (3, "push-read", 25_000_000), (7, "worker-connect", 40_000_000)], 3_000_000_000);
        let line = threads_line(Duration::from_secs(60), &before, &after);
        assert_eq!(line, "cpu-profile threads (60s): cpu_ms brain-tick=800x2 worker-connect=40x1 push-read=20x1 exited=140 total=1000 threads=4");
    }
}
