#![forbid(unsafe_code)]
//! Events that ask the brain for a decision now (lot 4.4).
//!
//! Layers below the brain (the push reader seeing a phase turn idle, the
//! market settling a fill) post `(account, reason)` here; the brain clock
//! thread blocks on `wait` and turns each post into a mailbox message. A
//! post is a set insert: ten fills of one account within one wait collapse
//! into one wake. Nothing here knows the brain.

use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Why an account needs a decision now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Event {
    /// The session phase turned idle (heartbeat frame).
    Idle,
    /// An order of the account was matched.
    Fill,
}

/// Posts kept at most (a flood of unknown accounts never grows it).
const CAPACITY: usize = 4096;

struct Hub {
    posts: Mutex<BTreeMap<String, Event>>,
    ready: Condvar,
}

fn hub() -> &'static Hub {
    static HUB: std::sync::OnceLock<Hub> = std::sync::OnceLock::new();
    HUB.get_or_init(|| Hub { posts: Mutex::new(BTreeMap::new()), ready: Condvar::new() })
}

/// Post an event for `account` (the first reason stays when several
/// arrive before the brain reads: both ask for the same decision).
pub fn post(account: &str, event: Event) {
    let hub = hub();
    let mut posts = hub.posts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if posts.len() >= CAPACITY && !posts.contains_key(account) {
        return;
    }
    posts.entry(account.to_string()).or_insert(event);
    hub.ready.notify_one();
}

/// Block up to `timeout` for posts, then take every one of them.
pub fn wait(timeout: Duration) -> Vec<(String, Event)> {
    let hub = hub();
    let mut posts = hub.posts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if posts.is_empty() {
        posts = hub.ready.wait_timeout(posts, timeout).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
    }
    std::mem::take(&mut *posts).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Posts of one account collapse, `wait` wakes at once on a post and
    /// returns empty on timeout. (One test: the hub is process-global.)
    #[test]
    fn posts_collapse_and_wake_the_waiter() {
        assert!(wait(Duration::from_millis(5)).is_empty());
        let poster = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            post("wh-a", Event::Fill);
            post("wh-a", Event::Idle);
            post("wh-b", Event::Idle);
        });
        let started = std::time::Instant::now();
        let mut got = wait(Duration::from_secs(5));
        poster.join().unwrap();
        got.extend(wait(Duration::from_millis(1)));
        assert!(started.elapsed() < Duration::from_secs(2), "woken by the post, not by the timeout");
        got.sort();
        assert_eq!(got, vec![("wh-a".to_string(), Event::Fill), ("wh-b".to_string(), Event::Idle)]);
    }
}
