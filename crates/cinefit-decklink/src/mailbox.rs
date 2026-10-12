//! A latest-wins handoff from the capture callback to the thread that renders.
//!
//! Not a channel: a bounded channel that is full drops the new frame and
//! keeps the old, the opposite of what a display wants, and an unbounded one
//! holds every frame the card can lend, which starves its pool.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// Holds at most one item: the newest put and not yet taken.
pub struct Mailbox<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

struct State<T> {
    item: Option<T>,
    displaced: u64,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Mailbox<T> {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                item: None,
                displaced: 0,
            }),
            ready: Condvar::new(),
        }
    }

    // A panic while the lock is held cannot leave the state half-changed:
    // every change is one assignment. So a poisoned lock is still good, and
    // the capture callback, where a panic aborts, never panics on it.
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Puts `item`, replacing one not yet taken, which counts as displaced.
    ///
    /// The displaced item is dropped after the lock is released: dropping a
    /// captured frame calls into the SDK, and the consumer must not wait on
    /// that.
    pub fn put(&self, item: T) {
        let displaced = {
            let mut s = self.lock();
            let old = s.item.replace(item);
            if old.is_some() {
                s.displaced += 1;
            }
            old
        };
        self.ready.notify_one();
        drop(displaced);
    }

    /// Takes the item if there is one, without waiting.
    pub fn take(&self) -> Option<T> {
        self.lock().item.take()
    }

    /// Takes the item, waiting up to `timeout` for one to be put.
    ///
    /// Waiting matters: a consumer that only samples at vblank, when the
    /// input's clock and the display's drift, sees a repeated frame and then
    /// a dropped one, alternately, where waiting a little would have caught
    /// each frame once.
    pub fn take_wait(&self, timeout: Duration) -> Option<T> {
        let guard = self.lock();
        let (mut s, _) = self
            .ready
            .wait_timeout_while(guard, timeout, |s| s.item.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        s.item.take()
    }

    /// Items replaced before anyone took them, since the mailbox was made.
    pub fn displaced(&self) -> u64 {
        self.lock().displaced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn takes_nothing_from_an_empty_mailbox() {
        let m = Mailbox::<u32>::new();
        assert_eq!(m.take(), None);
        assert_eq!(m.displaced(), 0);
    }

    #[test]
    fn a_put_is_taken_once() {
        let m = Mailbox::new();
        m.put(1);
        assert_eq!(m.take(), Some(1));
        assert_eq!(m.take(), None);
        assert_eq!(m.displaced(), 0);
    }

    #[test]
    fn the_newest_wins_and_the_older_are_counted_and_dropped() {
        struct Counted(u32, Arc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let m = Mailbox::new();
        for i in 0..3 {
            m.put(Counted(i, drops.clone()));
        }
        assert_eq!(m.displaced(), 2);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            2,
            "displaced items dropped at once"
        );
        assert_eq!(m.take().map(|c| c.0), Some(2));
    }

    #[test]
    fn a_displaced_item_is_dropped_outside_the_lock() {
        // An item whose drop locks the mailbox: dropped while the lock is
        // held, the put would deadlock (std's mutex is not reentrant).
        struct Relocks(Arc<Mailbox<Relocks>>);
        impl Drop for Relocks {
            fn drop(&mut self) {
                let _ = self.0.displaced();
            }
        }
        let m = Arc::new(Mailbox::new());
        let (done, finished) = mpsc::channel();
        let m2 = m.clone();
        thread::spawn(move || {
            m2.put(Relocks(m2.clone()));
            m2.put(Relocks(m2.clone()));
            done.send(()).unwrap();
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(5)).is_ok(),
            "put deadlocked dropping the displaced item"
        );
        // Break the cycle the remaining item makes with the mailbox.
        drop(m.take());
    }

    #[test]
    fn take_wait_returns_at_once_when_an_item_is_there() {
        let m = Mailbox::new();
        m.put(7);
        let started = Instant::now();
        assert_eq!(m.take_wait(Duration::from_secs(5)), Some(7));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn take_wait_times_out_empty() {
        let m = Mailbox::<u32>::new();
        let started = Instant::now();
        assert_eq!(m.take_wait(Duration::from_millis(50)), None);
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn take_wait_wakes_when_another_thread_puts() {
        let m = Arc::new(Mailbox::new());
        let m2 = m.clone();
        let started = Instant::now();
        let producer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            m2.put(9);
        });
        assert_eq!(m.take_wait(Duration::from_secs(5)), Some(9));
        // Woken by the put, not by the timeout.
        assert!(started.elapsed() < Duration::from_secs(4));
        producer.join().unwrap();
    }
}
