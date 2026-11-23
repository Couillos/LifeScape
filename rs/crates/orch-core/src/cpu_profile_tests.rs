use super::*;

/// Counts, wall and CPU sum per kind; `take` sorts by CPU and resets.
#[test]
fn window_sums_sorts_and_resets() {
    let start = Instant::now();
    let mut window = Window::new(start);
    window.add("cmd:status", 2_000_000, 1_000_000);
    window.add("cmd:status", 2_000_000, 1_500_000);
    window.add("market:quote", 9_000_000, 7_000_000);
    window.add("push:event", 500_000, 0);
    assert!(!window.due(start));
    assert!(window.due(start + WINDOW));
    let (rows, span) = window.take(start + WINDOW);
    assert_eq!(span, WINDOW);
    let names: Vec<&str> = rows.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["market:quote", "cmd:status", "push:event"]);
    assert_eq!(rows[1].1, Totals { count: 2, wall_ns: 4_000_000, cpu_ns: 2_500_000 });
    let (empty, _) = window.take(start + WINDOW);
    assert!(empty.is_empty(), "the window restarts empty");
    assert!(!window.due(start + WINDOW), "the clock restarts at take");
}

/// Request-provided labels are clipped and the map stays bounded.
#[test]
fn kinds_are_bounded_and_clipped() {
    let mut window = Window::new(Instant::now());
    for index in 0..(MAX_KINDS + 10) {
        window.add(&format!("cmd:k{index}"), 1, 1);
    }
    window.add(&"x".repeat(200), 1, 1);
    let (rows, _) = window.take(Instant::now());
    assert_eq!(rows.len(), MAX_KINDS + 1, "256 kinds + other");
    let other = rows.iter().find(|(name, _)| name == "other").expect("overflow folds into other");
    assert_eq!(other.1.count, 11);
    assert!(rows.iter().all(|(name, _)| name.len() <= MAX_LABEL));
}

/// One line: header with totals, then the heaviest `TOP` kinds only.
#[test]
fn summary_lists_totals_and_top_kinds() {
    let mut window = Window::new(Instant::now());
    window.add("market:quote", 3_000_000, 2_200_000);
    window.add("cmd:status", 1_000_000, 500_000);
    let (rows, _) = window.take(Instant::now());
    let line = summary_line("pool", &rows, Duration::from_secs(60));
    assert_eq!(
        line,
        "cpu-profile pool (60s, count/cpu_ms/wall_ms, 2 kinds): total=2/2.7/4.0 market:quote=1/2.2/3.0 cmd:status=1/0.5/1.0"
    );
    let mut many = Window::new(Instant::now());
    for index in 0..20u64 {
        many.add(&format!("phase{index:02}"), 1, index * 1_000_000);
    }
    let (rows, _) = many.take(Instant::now());
    let line = summary_line("brain", &rows, Duration::from_secs(60));
    assert_eq!(line.matches('=').count(), 1 + TOP, "total + top 12");
    assert!(line.contains(" phase19=") && !line.contains(" phase07="));
}

#[test]
fn proc_lines_parse() {
    assert_eq!(parse_schedstat("638714 0 2\n"), Some(638_714));
    let stat = "1750680 (brain tick) R 1 2 3 0 -1 4194304 97 0 0 0 25 7 0 0 20 0 1 0";
    assert_eq!(parse_stat(stat), Some(32 * USER_HZ_NS));
    assert_eq!(parse_stat("garbage"), None);
}

/// The cached-fd read sees fresh values (pread at 0 regenerates).
#[test]
fn thread_cpu_advances_with_work() {
    let before = thread_cpu_ns().expect("thread cpu readable on linux");
    let started = Instant::now();
    let mut spin = 0u64;
    while started.elapsed() < Duration::from_millis(30) {
        spin = spin.wrapping_mul(31).wrapping_add(7);
    }
    std::hint::black_box(spin);
    let after = thread_cpu_ns().expect("thread cpu readable on linux");
    assert!(after > before, "cpu advanced: {before} -> {after}");
}

/// Labels are per request: taken once, `unlabeled` when the handler
/// never named itself. Phases are no-ops off the brain thread.
#[test]
fn labels_and_phases() {
    label_request("cmd:status");
    assert_eq!(take_label(), "cmd:status");
    assert_eq!(take_label(), "unlabeled");
    assert!(phase("load_life").probe.is_none(), "not the brain thread");
    mark_brain_thread();
    assert!(phase("load_life").probe.is_some());
}
