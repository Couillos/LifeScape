#![forbid(unsafe_code)]
//! Always-on CPU attribution (VPS 2026-11-14): which daemon request
//! kinds and which brain phases burn the CPU. Each sample is the
//! calling thread's own CPU time (`/proc/thread-self/schedstat`, ns;
//! `stat` utime+stime as the fallback) plus wall time, summed per kind.
//! Every `WINDOW` the profile logs ONE line, top `TOP` kinds by CPU,
//! then resets. Overhead: one `pread` on a cached per-thread fd per
//! sample, no per-request log line.
//!
//! Two profiles: `pool` (daemon + market socket pools, labelled by the
//! handler: `cmd:<cmd>`, `push:<type>`, `market:<op>`) and `brain`
//! (the `brain-tick` thread: `tick.*` are whole steps, the other names
//! are phases nested inside them, so their sum never adds up to a total).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Summary period.
pub const WINDOW: Duration = Duration::from_secs(60);
/// Kinds listed per summary line.
pub const TOP: usize = 12;
/// Distinct kinds per window; past it everything folds into `other`
/// (labels come from request fields, never an unbounded map).
const MAX_KINDS: usize = 256;
/// Label length cap (request-provided text).
const MAX_LABEL: usize = 48;
/// Kernel USER_HZ for `/proc/*/stat` times (fixed at 100 on Linux).
const USER_HZ_NS: u64 = 10_000_000;

/// Summed counters of one kind over a window.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Totals {
    pub count: u64,
    pub wall_ns: u64,
    pub cpu_ns: u64,
}

impl Totals {
    fn add(&mut self, wall_ns: u64, cpu_ns: u64) {
        self.count += 1;
        self.wall_ns = self.wall_ns.saturating_add(wall_ns);
        self.cpu_ns = self.cpu_ns.saturating_add(cpu_ns);
    }
}

/// One aggregation window (pure: the clock is passed in).
pub struct Window {
    started: Instant,
    by_kind: HashMap<String, Totals>,
}

impl Window {
    pub fn new(now: Instant) -> Self {
        Window { started: now, by_kind: HashMap::new() }
    }

    pub fn add(&mut self, kind: &str, wall_ns: u64, cpu_ns: u64) {
        let kind = clip_label(kind);
        if let Some(totals) = self.by_kind.get_mut(kind) {
            totals.add(wall_ns, cpu_ns);
            return;
        }
        let key = if self.by_kind.len() >= MAX_KINDS { "other" } else { kind };
        self.by_kind.entry(key.to_string()).or_default().add(wall_ns, cpu_ns);
    }

    pub fn due(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= WINDOW
    }

    /// Rows sorted by CPU (desc, then name) and the span they cover;
    /// the window restarts empty at `now`.
    pub fn take(&mut self, now: Instant) -> (Vec<(String, Totals)>, Duration) {
        let span = now.saturating_duration_since(self.started);
        let mut rows: Vec<(String, Totals)> = std::mem::take(&mut self.by_kind).into_iter().collect();
        rows.sort_by(|left, right| right.1.cpu_ns.cmp(&left.1.cpu_ns).then_with(|| left.0.cmp(&right.0)));
        self.started = now;
        (rows, span)
    }
}

fn clip_label(kind: &str) -> &str {
    if kind.len() <= MAX_LABEL {
        return kind;
    }
    let mut end = MAX_LABEL;
    while !kind.is_char_boundary(end) {
        end -= 1;
    }
    &kind[..end]
}

fn millis(nanos: u64) -> String {
    format!("{:.1}", nanos as f64 / 1_000_000.0)
}

/// `cpu-profile <name> (<span>s, count/cpu_ms/wall_ms): total=… <kind>=… …`
/// with the `TOP` heaviest kinds; `rows` arrive sorted by `Window::take`.
pub fn summary_line(name: &str, rows: &[(String, Totals)], span: Duration) -> String {
    let mut total = Totals::default();
    for (_, totals) in rows {
        total.count += totals.count;
        total.wall_ns = total.wall_ns.saturating_add(totals.wall_ns);
        total.cpu_ns = total.cpu_ns.saturating_add(totals.cpu_ns);
    }
    let mut line = format!(
        "cpu-profile {name} ({:.0}s, count/cpu_ms/wall_ms, {} kinds): total={}/{}/{}",
        span.as_secs_f64(),
        rows.len(),
        total.count,
        millis(total.cpu_ns),
        millis(total.wall_ns),
    );
    for (kind, totals) in rows.iter().take(TOP) {
        line.push_str(&format!(" {kind}={}/{}/{}", totals.count, millis(totals.cpu_ns), millis(totals.wall_ns)));
    }
    line
}

/// A named profile: shared window, logged and reset every `WINDOW`.
pub struct Profile {
    name: &'static str,
    window: Mutex<Option<Window>>,
}

impl Profile {
    const fn new(name: &'static str) -> Self {
        Profile { name, window: Mutex::new(None) }
    }

    /// Adds one sample; the first sample past the window logs the line.
    pub fn record(&self, kind: &str, wall_ns: u64, cpu_ns: u64) {
        let now = Instant::now();
        let flushed = {
            let mut slot = self.window.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let window = slot.get_or_insert_with(|| Window::new(now));
            window.add(kind, wall_ns, cpu_ns);
            if window.due(now) { Some(window.take(now)) } else { None }
        };
        if let Some((rows, span)) = flushed {
            crate::logln!("{}", summary_line(self.name, &rows, span));
        }
    }

    /// Closes a probe opened with `Probe::start` under `kind`.
    pub fn finish(&self, probe: Probe, kind: &str) {
        let (wall_ns, cpu_ns) = probe.elapsed();
        self.record(kind, wall_ns, cpu_ns);
    }
}

static POOL: Profile = Profile::new("pool");
static BRAIN: Profile = Profile::new("brain");

/// Daemon/market socket pool profile.
pub fn pool() -> &'static Profile {
    &POOL
}

/// `brain-tick` profile.
pub fn brain() -> &'static Profile {
    &BRAIN
}

/// Wall + thread CPU at a start point.
pub struct Probe {
    wall: Instant,
    cpu_ns: Option<u64>,
}

impl Probe {
    pub fn start() -> Self {
        Probe { wall: Instant::now(), cpu_ns: thread_cpu_ns() }
    }

    /// (wall ns, thread CPU ns) since `start`; CPU 0 when unreadable.
    pub fn elapsed(&self) -> (u64, u64) {
        let wall_ns = u64::try_from(self.wall.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let cpu_ns = match (self.cpu_ns, thread_cpu_ns()) {
            (Some(before), Some(after)) => after.saturating_sub(before),
            _ => 0,
        };
        (wall_ns, cpu_ns)
    }
}

thread_local! {
    /// Per-thread cached fd (`/proc/thread-self` resolves at open time
    /// to the opening thread): `None` = not tried yet.
    static SCHEDSTAT: RefCell<Option<Option<File>>> = const { RefCell::new(None) };
    /// Pool request label set by the handler, taken by the pool loop.
    static POOL_LABEL: RefCell<Option<String>> = const { RefCell::new(None) };
    /// True on the `brain-tick` thread only: phases elsewhere are no-ops
    /// (`load_life` also runs on web/CLI threads).
    static BRAIN_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Calling thread's CPU time in ns: `schedstat` (exact run time), else
/// `stat` utime+stime (10 ms resolution). `None` when neither reads.
pub fn thread_cpu_ns() -> Option<u64> {
    let exact = SCHEDSTAT.with(|cell| {
        let mut slot = cell.borrow_mut();
        let file = slot.get_or_insert_with(|| File::open("/proc/thread-self/schedstat").ok()).as_ref()?;
        let mut buf = [0u8; 128];
        let read = file.read_at(&mut buf, 0).ok()?;
        parse_schedstat(std::str::from_utf8(&buf[..read]).ok()?)
    });
    exact.or_else(|| parse_stat(&std::fs::read_to_string("/proc/thread-self/stat").ok()?))
}

/// First field of `schedstat`: ns spent on a CPU.
pub fn parse_schedstat(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// utime + stime (fields 14 and 15) of a `stat` line, in ns. The comm
/// field may hold spaces: fields are counted after its closing paren.
pub fn parse_stat(text: &str) -> Option<u64> {
    let after_comm = &text[text.rfind(')')? + 1..];
    let mut fields = after_comm.split_whitespace().skip(11);
    let user: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some((user + system).saturating_mul(USER_HZ_NS))
}

/// Names the pool request served on this thread (last call wins).
pub fn label_request(kind: &str) {
    POOL_LABEL.with(|cell| *cell.borrow_mut() = Some(clip_label(kind).to_string()));
}

/// Takes this thread's request label (`unlabeled` when none was set).
pub fn take_label() -> String {
    POOL_LABEL.with(|cell| cell.borrow_mut().take()).unwrap_or_else(|| "unlabeled".to_string())
}

/// Marks the calling thread as the brain thread (phases record).
pub fn mark_brain_thread() {
    BRAIN_THREAD.with(|flag| flag.set(true));
}

/// Scope guard for one brain phase: records on drop, no-op off the
/// brain thread.
pub struct Phase {
    name: &'static str,
    probe: Option<Probe>,
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(probe) = self.probe.take() {
            brain().finish(probe, self.name);
        }
    }
}

/// Opens a brain phase guard: `let _phase = cpu_profile::phase("x");`.
pub fn phase(name: &'static str) -> Phase {
    let probe = BRAIN_THREAD.with(|flag| flag.get()).then(Probe::start);
    Phase { name, probe }
}

/// Runs `work` inside a brain phase named `name`.
pub fn timed<R>(name: &'static str, work: impl FnOnce() -> R) -> R {
    let _phase = phase(name);
    work()
}

#[cfg(test)]
#[path = "cpu_profile_tests.rs"]
mod tests;
