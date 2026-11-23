//! Test helper of the real-lives harnesses: the lives are split across
//! worker threads (`LIFESCAPE_HARNESS_THREADS`, default 6), each worker
//! keeps its own tally, the tallies are merged in a fixed order. Every
//! scenario stays deterministic (its generator is seeded from the life and
//! the variant), so the merged report equals the sequential one.

use lifescape_planner::Life;

/// Runs `work` on every life over `workers` threads (life `i` goes to
/// worker `i % workers`), one `T` per worker, returned in worker order.
pub(crate) fn par_lives<T: Default + Send>(lives: &[Life], work: impl Fn(&Life, &mut T) + Sync) -> Vec<T> {
    let workers = std::env::var("LIFESCAPE_HARNESS_THREADS").ok().and_then(|raw| raw.parse::<usize>().ok()).unwrap_or(6).clamp(1, 64);
    std::thread::scope(|scope| {
        let work = &work;
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                scope.spawn(move || {
                    let mut tally = T::default();
                    for life in lives.iter().skip(worker).step_by(workers) {
                        work(life, &mut tally);
                    }
                    tally
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
    })
}
