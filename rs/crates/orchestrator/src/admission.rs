#![forbid(unsafe_code)]
//! Session admission (E1) and opening pace (E10): the daemon refuses a new
//! bot instead of degrading the 300 already running.
//!
//! Envelope (20 Go / 8 cores, prod tick 600 ms): daemon <= 1 core, i.e.
//! <= 0.33 % per bot at active pace (PRD2 §7 allows <= 0.5 % per active
//! bot); worker PSS <= 20 Mo; continuous writes off-logs <= 1 Mo/s at
//! 300 bots (<= 3.5 Ko/s/bot); idle worker <= 2 wakeups/s.
//! `max_online_bots` is computed from the live `res-stats` rates, never
//! a magic constant: past the envelope `open` fails loud with
//! `admission:…`, the operator scales or waits.
//!
//! E6: `MALLOC_ARENA_MAX` is deliberately NOT set (no T2 gain shown).
//! E7: several bots per process is NOT adopted (needs a prod-CPU T4
//! gain >= 20 %, unmeasured at this stage).
//! E7: several bots per process is NOT adopted (needs a prod-CPU T4
//! gain >= 20 %, unmeasured at this stage).

use std::sync::{Mutex, OnceLock};

/// Hard ceiling even with zero measured load: one world, one daemon.
pub const ABSOLUTE_MAX_BOTS: usize = 300;
/// Disk guard (D34): >= 60 Go libres exigés avant tout run D'ENDURANCE,
/// jamais avant chaque ouverture en dev. L'admission ne refuse donc RIEN
/// sur le disque : `disk_warning` loggue + expose le flag res-stats, et
/// le refus dur reste réservé à l'enveloppe RAM/CPU (E1).
pub const MIN_FREE_BYTES: u64 = 60 * 1024 * 1024 * 1024;

/// Warning D34, pas refus : `Some(message)` sous le seuil, `None` sinon.
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
/// Daemon CPU budget per bot at active pace, in core-percent. E15 bench
/// (300 fake workers, 1 plan + 1 push per 700 ms per bot, 100 % L1):
/// 99 % of one core, i.e. 0.33 %/bot — inside the PRD2 §7 objective
/// (<= 0.5 % per active bot).
pub const DAEMON_CORES_PER_BOT: f64 = 0.0033;
/// Daemon budget: one core shared by nav, brain, market, store, web.
pub const DAEMON_BUDGET_CORES: f64 = 1.0;
/// Sustained off-log writes at 300 bots: 1 Mo/s, i.e. ~3.5 Ko/s/bot.
pub const WRITES_BYTES_PER_S_PER_BOT: f64 = 3500.0;
/// Daemon socket traffic one core sustains, in units/s (push frames +
/// plan ops). E15 recalibration (`bench-l2r 300 30`, active pace, 100 %
/// L1): 854 units/s at 99 % of one core, rounded down to 850. Covers
/// HIT traffic over plan-op-only exchanges (N9: zero polls on the wire);
/// Dijkstra misses stay compute-bound on the 4 plan threads and surface
/// as queue-full refusals, never as silent load.
pub const BUDGET_TRAFFIC_PER_S: f64 = 850.0;
/// Minimum spacing between two accepted `open` (E10): reconnect storms
/// re-enter one by one instead of thundering the login server.
pub const OPEN_MIN_INTERVAL_SECS: f64 = 2.0;

/// Live rates the admission reads (from `res-stats` + job count).
/// N9: no poll rate — the `plan` op answers terminal on one connexion,
/// so polls never ride the daemon socket (the waiter pool polls
/// in-process in 10 ms slices, invisible here); `plan_submits_per_s`
/// counts plan ops (one internal admission each).
#[derive(Clone, Debug, Default)]
pub struct DaemonLoad {
    pub online_bots: usize,
    pub pushes_per_s: f64,
    pub push_bytes_per_s: f64,
    pub files_per_s: f64,
    pub plan_submits_per_s: f64,
}

/// Free bytes on the filesystem holding `path` (D34 guard input).
/// `df` is the only std-only way to read it; unparseable output reads
/// as unbounded (never block opens on a measurement failure).
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

// Outcome of one admission check.
#[derive(Clone, Debug, PartialEq)]
pub enum Admission {
    Accept { remaining: usize },
    Refuse { reason: String },
}

/// `max_online_bots` from live measurements: the tightest of the daemon
/// CPU share (extrapolated per-bot from pushes+plans traffic) and the
/// write budget, capped by the absolute ceiling. Zero traffic with zero
/// bots admits the ceiling; a saturated daemon refuses nobody new.
/// Disk never refuses (D34 warning only, see `disk_warning`).
pub fn max_online_bots(load: &DaemonLoad) -> usize {
    // N9: pushes + plan ops only. Zero polls on the wire since the
    // `plan` op (E3): one connexion per plan, server-side wait.
    let traffic_per_s = load.pushes_per_s + load.plan_submits_per_s;
    let per_bot = if load.online_bots > 0 {
        traffic_per_s / load.online_bots as f64
    } else {
        0.0
    };
    let by_cpu = if per_bot > 0.0 {
        // One traffic unit (push/plan) costs the daemon a fixed slice:
        // scale the observed rate to the 1-core budget by rule of three.
        // E15 recalibration (active pace, 100 % L1): 854 units/s at 99 %
        // of one core, i.e. BUDGET_TRAFFIC_PER_S per core. The retired
        // idle-derived 300 mixed 2 ms poll round-trips the `plan` op
        // removed; the absolute ceiling still caps at 300.
        ((BUDGET_TRAFFIC_PER_S / per_bot.max(0.01)) as usize).max(1)
    } else {
        ABSOLUTE_MAX_BOTS
    };
    let by_writes = if load.online_bots > 0 && load.push_bytes_per_s > 0.0 {
        let per_bot_bytes = load.push_bytes_per_s / load.online_bots as f64;
        if per_bot_bytes > 0.0 {
            ((WRITES_BYTES_PER_S_PER_BOT * ABSOLUTE_MAX_BOTS as f64 / per_bot_bytes) as usize).max(1)
        } else {
            ABSOLUTE_MAX_BOTS
        }
    } else {
        ABSOLUTE_MAX_BOTS
    };
    by_cpu.min(by_writes).min(ABSOLUTE_MAX_BOTS).max(1)
}

/// Typed refusal or acceptance with the remaining headroom. Disk never
/// refuses: D34 is an endurance-run gate, enforced as a warning.
pub fn admission_check(load: &DaemonLoad) -> Admission {
    let max = max_online_bots(load);
    if load.online_bots >= max {
        return Admission::Refuse {
            reason: format!(
                "admission:envelope {}/{} bots (daemon<=1 core, writes<=1Mo/s); refusing instead of degrading",
                load.online_bots, max
            ),
        };
    }
    Admission::Accept { remaining: max - load.online_bots - 1 }
}

/// E10 pacing: at most one accepted `open` per interval, process-wide.
/// Returns the wait in seconds when the caller arrived too soon.
pub fn open_rate_due() -> Option<f64> {
    static LAST_OPEN: OnceLock<Mutex<f64>> = OnceLock::new();
    let slot = LAST_OPEN.get_or_init(|| Mutex::new(0.0));
    let mut last = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_secs_f64())
        .unwrap_or(0.0);
    if now - *last < OPEN_MIN_INTERVAL_SECS {
        Some(OPEN_MIN_INTERVAL_SECS - (now - *last))
    } else {
        *last = now;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle(online: usize) -> DaemonLoad {
        DaemonLoad { online_bots: online, ..Default::default() }
    }

    /// Empty daemon admits up to the ceiling, with headroom reported.
    #[test]
    fn empty_daemon_admits_to_ceiling() {
        assert_eq!(max_online_bots(&idle(0)), ABSOLUTE_MAX_BOTS);
        assert!(matches!(
            admission_check(&idle(0)),
            Admission::Accept { remaining: 299 }
        ));
    }

    /// The disk gate is a warning, never a refusal: D34 binds endurance
    /// runs, not dev openings. The envelope (CPU/writes) still refuses.
    #[test]
    fn disk_low_warns_but_admits() {
        assert!(super::disk_warning(1024).is_some(), "28 Go warns");
        assert!(super::disk_warning(100 * 1024 * 1024 * 1024).is_none(), "100 Go quiet");
        assert!(matches!(
            admission_check(&idle(0)),
            Admission::Accept { .. }
        ), "disk never refuses an open");
    }

    /// A saturated envelope refuses with the typed envelope reason,
    /// never a silent degradation.
    #[test]
    fn saturated_envelope_refuses_typing_envelope() {
        let mut load = idle(ABSOLUTE_MAX_BOTS);
        load.pushes_per_s = 600.0;
        load.plan_submits_per_s = 1350.0;
        load.push_bytes_per_s = 2_000_000.0;
        let Admission::Refuse { reason } = admission_check(&load) else {
            panic!("saturated daemon must refuse");
        };
        assert!(reason.starts_with("admission:envelope"), "typed, got {reason}");
    }

    /// E15 bench shape (1 plan + 1 push per 700 ms per bot, no polls):
    /// the gate sits at the ceiling, admitting below it and refusing
    /// past it. 290 x 2.86 units/s = 829/s fits the 850 budget (max 297);
    /// 300 x 2.86 = 858/s trips it — the bench proves 300 served at ~1
    /// core, so the gate guards exactly the measured edge.
    #[test]
    fn e15_active_pace_gate_sits_at_the_ceiling() {
        let pace = |online: usize| DaemonLoad {
            online_bots: online,
            pushes_per_s: online as f64 * 1.43,
            push_bytes_per_s: online as f64 * 200.0,
            plan_submits_per_s: online as f64 * 1.43,
            ..Default::default()
        };
        let max = max_online_bots(&pace(290));
        assert!((290..=300).contains(&max), "E15 pace fits one core, got max={max}");
        assert!(
            matches!(admission_check(&pace(290)), Admission::Accept { .. }),
            "290 active bots admit"
        );
        assert!(
            matches!(admission_check(&pace(300)), Admission::Refuse { .. }),
            "300 active bots trip the measured edge"
        );
    }

    /// Pacing lets one `open` through, holds the immediate next.
    #[test]
    fn open_rate_holds_the_immediate_next() {
        // First call may be held by an earlier test in the same process;
        // loop until one passes, then the next must wait.
        for _ in 0..50 {
            if open_rate_due().is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(open_rate_due().is_some(), "back-to-back opens pace out");
    }
}
