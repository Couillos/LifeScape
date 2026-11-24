#![forbid(unsafe_code)]
//! Admission of the navigation service in one object (lot N6): who may
//! queue a plan, the tickets and their results, the bounded queue, the
//! waiters sharing a ticket, the completion signal the waiter pool sleeps
//! on.
//!
//! These were nine locks and counters of `Service`, spread over `op_submit`,
//! `poll_slot`, `op_cancel`, `serve_plans` and four helpers. Each rule is
//! now one method with the lock order it always had (sequential, never
//! two held), and each is exercised without a socket, a snapshot or a
//! pool thread (tests below, a `Job` is just data).
//!
//! Rules kept as they were:
//! - a cache hit never reaches this object (the service answers before);
//! - an identical pending request joins its ticket (`join`), never a
//!   second Dijkstra;
//! - `ACCOUNT_CAP` tickets per account, a full queue refuses at once;
//! - the result slot exists BEFORE the job is queued (the pool must never
//!   dequeue a job whose Pending slot is not visible yet);
//! - only the last waiter truly cancels, and releases the account then;
//! - admission is released exactly once per ticket (a double release stole
//!   a live ticket's cap unit);
//! - `results` stays bounded: consumed terminals leave first, oldest id
//!   first, never a pending one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use lifescape_json::Value;

use crate::plan::{NavOptions, Tile};
use crate::service::{PlanKey, ACCOUNT_CAP, QUEUE_CAP, RESULTS_CAP};

/// One queued computation.
pub(crate) struct Job {
    pub(crate) id: u64,
    pub(crate) account: String,
    pub(crate) key: PlanKey,
    pub(crate) from: Tile,
    pub(crate) dest: Tile,
    pub(crate) options: NavOptions,
    pub(crate) cancel: Arc<AtomicBool>,
}

/// A ticket's state in `results`.
pub(crate) enum Slot {
    Pending { cancel: Arc<AtomicBool> },
    Ready { plan: Value, polled: bool },
    Failed { error: Value, polled: bool },
}

/// What a poll finds. `Ready` and `Failed` clone out without consuming
/// the slot (N waiters poll one id); eviction reaps them later.
pub(crate) enum Polled {
    Pending,
    Ready(Value),
    Failed(Value),
    Unknown,
}

/// Why a submit was not queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    AccountCap,
    QueueFull,
}

/// Outcome of `submit`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Submitted {
    /// An identical request was pending: its ticket, one more waiter.
    Joined(u64),
    /// A new ticket, queued.
    Queued(u64),
    Refused(Refusal),
}

pub(crate) struct Admission {
    /// Tickets held per account (the cap).
    pub(crate) accounts: Mutex<HashMap<String, usize>>,
    /// Pending computation per full key (the join table).
    pub(crate) inflight: Mutex<HashMap<PlanKey, u64>>,
    /// Ticket -> (key, owner) while it holds an account unit.
    pub(crate) tickets: Mutex<HashMap<u64, (PlanKey, String)>>,
    /// Waiters per ticket.
    pub(crate) waiters: Mutex<HashMap<u64, usize>>,
    pub(crate) results: Mutex<HashMap<u64, Slot>>,
    queue_tx: SyncSender<Job>,
    queue_rx: Mutex<Receiver<Job>>,
    queued: AtomicUsize,
    next_id: AtomicU64,
    /// Terminal outcomes stored so far + the condvar `plan` waiters sleep
    /// on: a finished search wakes them at once instead of the next 10 ms
    /// poll slice (reactivity, fewer idle wake-ups).
    completions: Mutex<u64>,
    completed: Condvar,
}

impl Admission {
    pub(crate) fn new() -> Admission {
        Admission::with_queue(QUEUE_CAP)
    }

    /// A queue of `capacity` jobs (tests use tiny ones).
    pub(crate) fn with_queue(capacity: usize) -> Admission {
        let (queue_tx, queue_rx) = mpsc::sync_channel(capacity);
        Admission {
            accounts: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            tickets: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            queue_tx,
            queue_rx: Mutex::new(queue_rx),
            queued: AtomicUsize::new(0),
            next_id: AtomicU64::new(1),
            completions: Mutex::new(0),
            completed: Condvar::new(),
        }
    }

    /// Queue depth now: sent-but-undequeued jobs (a cancelled ticket still
    /// queued counts until the pool drops it: admission already released,
    /// so the cap never wedges on it).
    pub(crate) fn queue_depth(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    /// Tickets held across every account.
    pub(crate) fn tickets_held(&self) -> usize {
        self.accounts.lock().unwrap().values().sum()
    }

    /// Tickets one account holds, `None` when it holds none.
    #[cfg(test)]
    pub(crate) fn held_by(&self, account: &str) -> Option<usize> {
        self.accounts.lock().unwrap().get(account).copied()
    }

    /// A request for `key`: join the pending identical one, else take an
    /// account unit and queue a new ticket. Sequential locks only (lock
    /// order: inflight, results, waiters, accounts, never nested).
    pub(crate) fn submit(&self, account: &str, key: PlanKey, from: Tile, dest: Tile, options: NavOptions) -> Submitted {
        if let Some(id) = self.join(&key) {
            return Submitted::Joined(id);
        }
        {
            let mut accounts = self.accounts.lock().unwrap();
            let in_flight = accounts.get(account).copied().unwrap_or(0);
            if in_flight >= ACCOUNT_CAP {
                return Submitted::Refused(Refusal::AccountCap);
            }
            accounts.insert(account.to_string(), in_flight + 1);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        // Slot BEFORE the queue: the pool must never dequeue a job whose
        // Pending slot is not yet visible (it would drop a live ticket).
        // A refused queue removes the slot again — never a dangling id.
        self.results.lock().unwrap().insert(id, Slot::Pending { cancel: Arc::clone(&cancel) });
        self.inflight.lock().unwrap().insert(key.clone(), id);
        self.tickets.lock().unwrap().insert(id, (key.clone(), account.to_string()));
        self.waiters.lock().unwrap().insert(id, 1);
        let job = Job { id, account: account.to_string(), key, from, dest, options, cancel };
        if let Err(error) = self.queue_tx.try_send(job) {
            // The job back: its key recovers the exact in-flight entry
            // (the tickets map is already cleared below, so it cannot).
            let job = match error {
                mpsc::TrySendError::Full(back) | mpsc::TrySendError::Disconnected(back) => back,
            };
            self.results.lock().unwrap().remove(&job.id);
            self.tickets.lock().unwrap().remove(&job.id);
            self.waiters.lock().unwrap().remove(&job.id);
            let mut inflight = self.inflight.lock().unwrap();
            if inflight.get(&job.key) == Some(&job.id) {
                inflight.remove(&job.key);
            }
            let mut accounts = self.accounts.lock().unwrap();
            let in_flight = accounts.get(&job.account).copied().unwrap_or(1);
            accounts.insert(job.account, in_flight.saturating_sub(1));
            return Submitted::Refused(Refusal::QueueFull);
        }
        self.queued.fetch_add(1, Ordering::Relaxed);
        Submitted::Queued(id)
    }

    /// The pending ticket of an identical request, one more waiter on it.
    fn join(&self, key: &PlanKey) -> Option<u64> {
        let id = self.inflight.lock().unwrap().get(key).copied()?;
        let pending = matches!(self.results.lock().unwrap().get(&id), Some(Slot::Pending { .. }));
        if !pending {
            return None;
        }
        *self.waiters.lock().unwrap().entry(id).or_insert(1) += 1;
        Some(id)
    }

    /// A cache hit answers as a fresh terminal ticket (a poll reads it,
    /// same shape as a computed plan, no queue involved).
    pub(crate) fn instant(&self, is_plan: bool, body: Value) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.waiters.lock().unwrap().insert(id, 1);
        let mut results = self.results.lock().unwrap();
        if is_plan {
            results.insert(id, Slot::Ready { plan: body, polled: false });
        } else {
            results.insert(id, Slot::Failed { error: body, polled: false });
        }
        drop(results);
        self.trim();
        id
    }

    /// Keep `results` within `RESULTS_CAP`. Stops when nothing is
    /// consumable (pendings only) instead of spinning.
    pub(crate) fn trim(&self) {
        while self.results.lock().unwrap().len() > RESULTS_CAP {
            if !self.evict_one() {
                break;
            }
        }
    }

    /// Reap one consumable terminal: a polled (already consumed) one
    /// first, oldest id first so the choice is FIFO-deterministic; an
    /// unpolled one only when no polled terminal exists (the bound holds,
    /// never a pending). The waiter orphan goes down with its slot:
    /// without this, every evicted id leaked one `waiters` entry forever
    /// (`unknown-ticket` storm artefact). One lock at a time, never
    /// nested. False when nothing is consumable.
    pub(crate) fn evict_one(&self) -> bool {
        let victim = self
            .results
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, slot)| matches!(slot, Slot::Ready { .. } | Slot::Failed { .. }))
            .min_by_key(|(id, slot)| {
                let consumed = matches!(slot, Slot::Ready { polled: true, .. } | Slot::Failed { polled: true, .. });
                (!consumed, **id)
            })
            .map(|(id, _)| *id);
        if let Some(id) = victim {
            self.results.lock().unwrap().remove(&id);
            self.waiters.lock().unwrap().remove(&id);
            true
        } else {
            false
        }
    }

    /// Read a ticket. Ready/Failed answers clone out WITHOUT consuming
    /// the slot (N waiters poll the same id); a first poll marks the
    /// terminal consumed so eviction prefers it over a never-polled one.
    /// Pending stays until computed or cancelled.
    pub(crate) fn poll(&self, id: u64) -> Polled {
        let mut results = self.results.lock().unwrap();
        match results.get_mut(&id) {
            Some(Slot::Pending { .. }) => Polled::Pending,
            Some(Slot::Ready { plan, polled }) => {
                *polled = true;
                Polled::Ready(plan.clone())
            }
            Some(Slot::Failed { error, polled }) => {
                *polled = true;
                Polled::Failed(error.clone())
            }
            None => Polled::Unknown,
        }
    }

    /// A waiter leaves: only the last one truly cancels (the others keep
    /// polling the same id). The last cancel also releases the account
    /// NOW: a cancelled ticket must not wedge the cap until the pool
    /// dequeues and drops it. Double release is harmless (`release` on a
    /// missing key is a no-op) and the over-admit window is one ticket for
    /// milliseconds.
    pub(crate) fn cancel(&self, id: u64) {
        let last = {
            let mut waiters = self.waiters.lock().unwrap();
            match waiters.get(&id).copied() {
                Some(count) if count > 1 => {
                    waiters.insert(id, count - 1);
                    false
                }
                _ => {
                    waiters.remove(&id);
                    true
                }
            }
        };
        if last {
            if let Some(Slot::Pending { cancel }) = self.results.lock().unwrap().remove(&id) {
                cancel.store(true, Ordering::Relaxed);
            }
            if let Some((key, owner)) = self.tickets.lock().unwrap().remove(&id) {
                let mut inflight = self.inflight.lock().unwrap();
                if inflight.get(&key) == Some(&id) {
                    inflight.remove(&key);
                }
                drop(inflight);
                self.release(&owner);
            }
        }
    }

    /// A waiter of a terminal ticket leaves: only the waiter count goes
    /// (completion already cleaned tickets, inflight and the account; the
    /// Ready/Failed slot stays consumable for joins).
    pub(crate) fn release_waiter(&self, id: u64) {
        let mut waiters = self.waiters.lock().unwrap();
        match waiters.get(&id).copied() {
            Some(count) if count > 1 => {
                waiters.insert(id, count - 1);
            }
            _ => {
                waiters.remove(&id);
            }
        }
    }

    /// Give one account unit back (a missing account is a no-op).
    pub(crate) fn release(&self, account: &str) {
        let mut accounts = self.accounts.lock().unwrap();
        let in_flight = accounts.get(account).copied().unwrap_or(1);
        if in_flight <= 1 {
            accounts.remove(account);
        } else {
            accounts.insert(account.to_string(), in_flight - 1);
        }
    }

    /// Pool side: the next job to compute, blocking; `None` when the
    /// queue is closed. A job cancelled while queued is dropped here: the
    /// canceller already dropped the maps and released admission, so only
    /// release when this thread still owns them, never twice for one
    /// ticket (a double release stole a live ticket's cap unit and let
    /// the effective concurrency drift above `ACCOUNT_CAP`).
    pub(crate) fn next_job(&self) -> Option<Job> {
        loop {
            let job = {
                let queue = self.queue_rx.lock().unwrap();
                queue.recv().ok()?
            };
            self.queued.fetch_sub(1, Ordering::Relaxed);
            if self.results.lock().unwrap().contains_key(&job.id) {
                return Some(job);
            }
            if self.tickets.lock().unwrap().remove(&job.id).is_some() {
                let mut inflight = self.inflight.lock().unwrap();
                if inflight.get(&job.key) == Some(&job.id) {
                    inflight.remove(&job.key);
                }
                drop(inflight);
                self.release(&job.account);
            }
        }
    }

    /// Pool side: a computed outcome lands. One critical section turns
    /// Pending into Ready/Failed with no poll-visible hole (a poll never
    /// sees a missing live ticket); a missing slot means every waiter
    /// cancelled and the outcome is dropped. Then the join entry goes,
    /// and the account unit comes back exactly once per ticket: a cancel
    /// already released it and dropped the maps, so only the owner still
    /// holding the ticket entry releases here.
    pub(crate) fn finish(&self, job: &Job, is_plan: bool, body: Value) {
        let mut results = self.results.lock().unwrap();
        if matches!(results.get(&job.id), Some(Slot::Pending { .. })) {
            if is_plan {
                results.insert(job.id, Slot::Ready { plan: body, polled: false });
            } else {
                results.insert(job.id, Slot::Failed { error: body, polled: false });
            }
            // Bound stored results: evict a consumable outcome (never a
            // pending); its owner already gave up and fell back sync.
            drop(results);
            self.trim();
            *self.completions.lock().unwrap() += 1;
            self.completed.notify_all();
        }
        {
            let mut inflight = self.inflight.lock().unwrap();
            if inflight.get(&job.key) == Some(&job.id) {
                inflight.remove(&job.key);
            }
        }
        if self.tickets.lock().unwrap().remove(&job.id).is_some() {
            self.release(&job.account);
        }
    }

    /// Completions so far (read BEFORE a poll, then `wait_completion`).
    pub(crate) fn completion_mark(&self) -> u64 {
        *self.completions.lock().unwrap()
    }

    /// Sleeps until a search completes after `seen`, at most `slice`.
    pub(crate) fn wait_completion(&self, seen: u64, slice: Duration) {
        let guard = self.completions.lock().unwrap();
        let _ = self.completed.wait_timeout_while(guard, slice, |count| *count == seen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_json::{object, str_value};

    fn tile(x: i32) -> Tile {
        Tile { x, z: 3200, level: 0 }
    }

    fn key(dest_x: i32) -> PlanKey {
        PlanKey::of(tile(3200), tile(dest_x), &NavOptions::default(), "v")
    }

    fn submit(admission: &Admission, account: &str, dest_x: i32) -> Submitted {
        admission.submit(account, key(dest_x), tile(3200), tile(dest_x), NavOptions::default())
    }

    fn body(text: &str) -> Value {
        object(vec![("text", str_value(text))])
    }

    #[test]
    fn a_ticket_queues_computes_and_releases_its_account() {
        let admission = Admission::with_queue(4);
        let Submitted::Queued(id) = submit(&admission, "bot", 3210) else { panic!("queued") };
        assert_eq!(admission.queue_depth(), 1);
        assert_eq!(admission.held_by("bot"), Some(1));
        assert!(matches!(admission.poll(id), Polled::Pending));
        let job = admission.next_job().expect("the job comes out");
        assert_eq!((job.id, job.account.as_str()), (id, "bot"));
        assert_eq!(admission.queue_depth(), 0);
        let before = admission.completion_mark();
        admission.finish(&job, true, body("plan"));
        assert_eq!(admission.completion_mark(), before + 1);
        assert!(matches!(admission.poll(id), Polled::Ready(_)), "terminal, readable");
        assert!(matches!(admission.poll(id), Polled::Ready(_)), "reading does not consume it");
        assert_eq!(admission.held_by("bot"), None, "the account unit is back");
        assert!(admission.inflight.lock().unwrap().is_empty());
        assert!(admission.tickets.lock().unwrap().is_empty());
    }

    #[test]
    fn an_identical_pending_request_joins_the_ticket() {
        let admission = Admission::with_queue(4);
        let Submitted::Queued(id) = submit(&admission, "a", 3210) else { panic!("queued") };
        assert_eq!(submit(&admission, "b", 3210), Submitted::Joined(id));
        assert_eq!(admission.waiters.lock().unwrap().get(&id).copied(), Some(2));
        assert_eq!(admission.held_by("b"), None, "a join takes no account unit");
        // One waiter leaves: the other still holds the ticket.
        admission.cancel(id);
        assert!(matches!(admission.poll(id), Polled::Pending));
        assert_eq!(admission.held_by("a"), Some(1));
        // The last one cancels for real and gives the unit back.
        admission.cancel(id);
        assert!(matches!(admission.poll(id), Polled::Unknown));
        assert_eq!(admission.held_by("a"), None);
        assert!(admission.inflight.lock().unwrap().is_empty());
    }

    #[test]
    fn the_account_cap_and_a_full_queue_refuse_at_once() {
        let admission = Admission::with_queue(2);
        assert!(matches!(submit(&admission, "bot", 3201), Submitted::Queued(_)));
        assert!(matches!(submit(&admission, "bot", 3202), Submitted::Queued(_)));
        assert_eq!(submit(&admission, "bot", 3203), Submitted::Refused(Refusal::AccountCap), "ACCOUNT_CAP is {ACCOUNT_CAP}");
        // Another account, full queue: refused, and nothing dangles.
        assert_eq!(submit(&admission, "other", 3204), Submitted::Refused(Refusal::QueueFull));
        assert!(admission.held_by("other").unwrap_or(0) == 0, "the unit taken for the refused job came back");
        assert!(admission.results.lock().unwrap().len() == 2, "no slot left for the refused job");
        assert!(!admission.inflight.lock().unwrap().contains_key(&key(3204)));
    }

    #[test]
    fn a_job_cancelled_while_queued_is_dropped_without_a_double_release() {
        let admission = Admission::with_queue(4);
        let Submitted::Queued(first) = submit(&admission, "bot", 3210) else { panic!("queued") };
        admission.cancel(first);
        assert_eq!(admission.held_by("bot"), None, "the cancel released it");
        // A second ticket of the same account is in flight when the pool
        // meets the cancelled job: it must keep its unit.
        let Submitted::Queued(second) = submit(&admission, "bot", 3220) else { panic!("queued") };
        let job = admission.next_job().expect("the live job, the cancelled one is skipped");
        assert_eq!(job.id, second);
        assert_eq!(admission.held_by("bot"), Some(1), "no second release");
        admission.finish(&job, false, body("no route"));
        assert!(matches!(admission.poll(second), Polled::Failed(_)));
        assert_eq!(admission.held_by("bot"), None);
    }

    #[test]
    fn a_cancelled_outcome_is_dropped_and_results_stay_bounded() {
        let admission = Admission::with_queue(4);
        let Submitted::Queued(id) = submit(&admission, "bot", 3210) else { panic!("queued") };
        let job = admission.next_job().expect("job");
        admission.cancel(id);
        admission.finish(&job, true, body("late"));
        assert!(matches!(admission.poll(id), Polled::Unknown), "nobody waits for it any more");
        for index in 0..RESULTS_CAP + 20 {
            admission.instant(true, body(&format!("hit {index}")));
        }
        assert!(admission.results.lock().unwrap().len() <= RESULTS_CAP, "cache hits cannot grow the table");
    }

    #[test]
    fn eviction_takes_consumed_terminals_first_and_never_a_pending() {
        let admission = Admission::with_queue(4);
        let Submitted::Queued(pending) = submit(&admission, "bot", 3210) else { panic!("queued") };
        let old = admission.instant(true, body("old"));
        let newer = admission.instant(true, body("newer"));
        assert!(matches!(admission.poll(newer), Polled::Ready(_)), "newer is consumed");
        assert!(admission.evict_one());
        assert!(matches!(admission.poll(newer), Polled::Unknown), "the consumed one goes first, though newer");
        assert!(matches!(admission.poll(old), Polled::Ready(_)));
        assert!(admission.evict_one());
        assert!(!admission.evict_one(), "only the pending one is left: nothing consumable");
        assert!(matches!(admission.poll(pending), Polled::Pending));
        assert!(!admission.waiters.lock().unwrap().contains_key(&newer), "no waiter orphan");
    }
}
