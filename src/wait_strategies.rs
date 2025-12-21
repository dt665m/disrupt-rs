//! Module with different strategies for waiting for an event to be published.
//!
//! The lowest latency possible is the [`BusySpin`] strategy.
//!
//! To "waste" less CPU time and power, use one of the other strategies which have higher latency.

use std::{
    hint,
    sync::atomic::{fence, AtomicBool, AtomicI64, Ordering},
    sync::Arc,
    thread,
};

use crate::{barrier::Barrier, Sequence};
use crossbeam_utils::CachePadded;
use parking_lot::{Condvar, Mutex};

macro_rules! return_if_available_or_shutdown {
    ($requested:expr, $barrier:expr, $shutdown:expr) => {{
        let requested = $requested;
        let barrier = $barrier;
        let shutdown = $shutdown;

        let available = barrier.get_after(requested);
        if available >= requested {
            fence(Ordering::Acquire);
            return WaitOutcome::Available { upper: available };
        }

        if shutdown.load(Ordering::Relaxed) == requested {
            return WaitOutcome::Shutdown;
        }
    }};
}

/// Outcome of waiting for availability.
pub enum WaitOutcome {
    /// Highest contiguous sequence available (inclusive).
    Available {
        /// Highest available sequence (inclusive).
        upper: Sequence,
    },
    /// The disruptor is shutting down.
    Shutdown,
    /// The wait timed out without progress.
    Timeout,
}

/// Handle for nudging blocked waiters after publication.
pub trait WakeupNotifier: Send + Sync {
    /// Hint waiters that new data may be available.
    fn wake(&self);
}

/// No-op wakeup notifier for spin/yield strategies.
#[derive(Default, Clone)]
pub struct NoopWakeupNotifier;

impl WakeupNotifier for NoopWakeupNotifier {
    #[inline]
    fn wake(&self) {}
}

/// Public wait strategy; cloneable so producers can hold a copy for signaling.
pub trait WaitStrategy: Send + Sync + Clone + 'static {
    /// Per-consumer waiter type that holds any mutable state.
    type Waiter: Waiter;
    /// Handle type used by producers to wake blocked waiters.
    type Notifier: WakeupNotifier + Clone + Send + Sync + 'static;

    /// Create a new waiter instance for a consumer thread.
    fn new_waiter(&self) -> Self::Waiter;

    /// Called during build to hand producers a wakeup handle.
    fn notifier(&self) -> Self::Notifier;
}

/// Per-consumer stateful waiter; owns spin/backoff/park bookkeeping.
pub trait Waiter: Send {
    /// Return highest contiguous available ≥ requested, or a terminal/alert condition.
    ///
    /// Implementations must issue an `Acquire` fence before returning `Available`.
    fn wait_for(
        &mut self,
        requested: Sequence,
        barrier: &impl Barrier,
        shutdown: &CachePadded<AtomicI64>,
    ) -> WaitOutcome;
}

/// Busy spin wait strategy. Lowest possible latency.
#[derive(Clone, Copy, Default)]
pub struct BusySpin;

impl WaitStrategy for BusySpin {
    type Waiter = BusySpinWaiter;
    type Notifier = NoopWakeupNotifier;

    #[inline]
    fn new_waiter(&self) -> Self::Waiter {
        BusySpinWaiter
    }

    #[inline]
    fn notifier(&self) -> Self::Notifier {
        NoopWakeupNotifier
    }
}

/// State holder for [`BusySpin`] wait strategy.
pub struct BusySpinWaiter;

impl Waiter for BusySpinWaiter {
    #[inline]
    fn wait_for(
        &mut self,
        requested: Sequence,
        barrier: &impl Barrier,
        shutdown: &CachePadded<AtomicI64>,
    ) -> WaitOutcome {
        loop {
            return_if_available_or_shutdown!(requested, barrier, shutdown);
        }
    }
}

/// Busy spin wait strategy with spin loop hint which enables the processor to optimize its behavior
/// by e.g. saving power os switching hyper threads. Obviously, this can induce latency.
///
/// See also [`BusySpin`].
#[derive(Clone, Copy, Default)]
pub struct BusySpinWithSpinLoopHint;

impl WaitStrategy for BusySpinWithSpinLoopHint {
    type Waiter = BusySpinWithSpinLoopHintWaiter;
    type Notifier = NoopWakeupNotifier;

    fn new_waiter(&self) -> Self::Waiter {
        BusySpinWithSpinLoopHintWaiter
    }

    #[inline]
    fn notifier(&self) -> Self::Notifier {
        NoopWakeupNotifier
    }
}

/// State holder for [`BusySpinWithSpinLoopHint`] wait strategy.
pub struct BusySpinWithSpinLoopHintWaiter;

impl Waiter for BusySpinWithSpinLoopHintWaiter {
    fn wait_for(
        &mut self,
        requested: Sequence,
        barrier: &impl Barrier,
        shutdown: &CachePadded<AtomicI64>,
    ) -> WaitOutcome {
        loop {
            return_if_available_or_shutdown!(requested, barrier, shutdown);

            hint::spin_loop();
        }
    }
}

const SPIN_TRIES: u32 = 100;

/// Yielding wait strategy: spin for a while, then yield the thread.
///
/// This mirrors the Java Disruptor's `YieldingWaitStrategy`, trading a small increase in latency
/// for reduced CPU usage under contention.
#[derive(Clone, Copy, Default)]
pub struct YieldingWaitStrategy;

impl WaitStrategy for YieldingWaitStrategy {
    type Waiter = YieldingWaiter;
    type Notifier = NoopWakeupNotifier;

    fn new_waiter(&self) -> Self::Waiter {
        YieldingWaiter {
            spins_remaining: SPIN_TRIES,
        }
    }

    #[inline]
    fn notifier(&self) -> Self::Notifier {
        NoopWakeupNotifier
    }
}

/// State holder for [`YieldingWaitStrategy`].
pub struct YieldingWaiter {
    spins_remaining: u32,
}

impl Waiter for YieldingWaiter {
    fn wait_for(
        &mut self,
        requested: Sequence,
        barrier: &impl Barrier,
        shutdown: &CachePadded<AtomicI64>,
    ) -> WaitOutcome {
        // Reset budget for this wait invocation.
        self.spins_remaining = SPIN_TRIES;
        loop {
            return_if_available_or_shutdown!(requested, barrier, shutdown);

            if self.spins_remaining > 0 {
                self.spins_remaining -= 1;
                hint::spin_loop();
            } else {
                thread::yield_now();
            }
        }
    }
}

/// Blocking wait strategy using a single `Condvar` shared among all waiters.
///
/// This is similar to the Java Disruptor's `BlockingWaitStrategy`: waiters block on a condition
/// variable and are woken by producers and (in this crate) by consumers when they advance.
///
/// Note: on its own, a `Condvar` does not provide a predicate; we use an epoch counter protected
/// by the same mutex to avoid missed wakeups.
#[derive(Clone, Default)]
pub struct BlockingWaitStrategy {
    inner: Arc<BlockingInner>,
    spin_tries: u32,
}

impl BlockingWaitStrategy {
    /// Configure a short busy-spin window before blocking on the condvar.
    ///
    /// This can reduce lock/park/unpark overhead under sustained high load, while still allowing
    /// threads to sleep when the system is mostly idle.
    pub fn with_spin_tries(mut self, spin_tries: u32) -> Self {
        self.spin_tries = spin_tries;
        self
    }
}

#[derive(Default)]
struct BlockingInner {
    mutex: Mutex<u64>,
    condvar: Condvar,
}

/// Per-consumer waiter for [`BlockingWaitStrategy`].
pub struct BlockingWaiter {
    inner: Arc<BlockingInner>,
    spin_tries: u32,
}

#[derive(Clone)]
/// Producer/consumer wake handle for [`BlockingWaitStrategy`].
pub struct BlockingNotifier {
    inner: Arc<BlockingInner>,
}

impl WakeupNotifier for BlockingNotifier {
    fn wake(&self) {
        let mut epoch = self.inner.mutex.lock();
        *epoch = epoch.wrapping_add(1);
        self.inner.condvar.notify_all();
    }
}

impl WaitStrategy for BlockingWaitStrategy {
    type Waiter = BlockingWaiter;
    type Notifier = BlockingNotifier;

    fn new_waiter(&self) -> Self::Waiter {
        BlockingWaiter {
            inner: Arc::clone(&self.inner),
            spin_tries: self.spin_tries,
        }
    }

    fn notifier(&self) -> Self::Notifier {
        BlockingNotifier {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Waiter for BlockingWaiter {
    fn wait_for(
        &mut self,
        requested: Sequence,
        barrier: &impl Barrier,
        shutdown: &CachePadded<AtomicI64>,
    ) -> WaitOutcome {
        loop {
            return_if_available_or_shutdown!(requested, barrier, shutdown);

            for _ in 0..self.spin_tries {
                return_if_available_or_shutdown!(requested, barrier, shutdown);

                hint::spin_loop();
            }

            let mut epoch_guard = self.inner.mutex.lock();
            let epoch = *epoch_guard;
            while *epoch_guard == epoch {
                return_if_available_or_shutdown!(requested, barrier, shutdown);

                self.inner.condvar.wait(&mut epoch_guard);
            }
        }
    }
}

/// "Lite" blocking wait strategy.
///
/// Compared to [`BlockingWaitStrategy`], this tracks the number of currently parked waiters and
/// avoids taking the mutex in `wake()` when there are no waiters.
#[derive(Clone, Default)]
pub struct LiteBlockingWaitStrategy {
    inner: Arc<LiteBlockingInner>,
    spin_tries: u32,
}

impl LiteBlockingWaitStrategy {
    /// Configure a short busy-spin window before blocking on the condvar.
    ///
    /// This can reduce lock/park/unpark overhead under sustained high load, while still allowing
    /// threads to sleep when the system is mostly idle.
    pub fn with_spin_tries(mut self, spin_tries: u32) -> Self {
        self.spin_tries = spin_tries;
        self
    }
}

#[derive(Default)]
struct LiteBlockingInner {
    mutex: Mutex<u64>,
    condvar: Condvar,
    signal_needed: AtomicBool,
}

/// Per-consumer waiter for [`LiteBlockingWaitStrategy`].
pub struct LiteBlockingWaiter {
    inner: Arc<LiteBlockingInner>,
    spin_tries: u32,
}

#[derive(Clone)]
/// Producer/consumer wake handle for [`LiteBlockingWaitStrategy`].
pub struct LiteBlockingNotifier {
    inner: Arc<LiteBlockingInner>,
}

impl WakeupNotifier for LiteBlockingNotifier {
    fn wake(&self) {
        // Fast-path: no waiter has indicated it may block since the last signal.
        //
        // This is intentionally *not* keyed off `waiters == 0` as that can miss a signal when a
        // waiter is about to park (the classic "check-then-sleep" race).
        if !self.inner.signal_needed.swap(false, Ordering::AcqRel) {
            return;
        }
        let mut epoch = self.inner.mutex.lock();
        *epoch = epoch.wrapping_add(1);
        self.inner.condvar.notify_all();
    }
}

impl WaitStrategy for LiteBlockingWaitStrategy {
    type Waiter = LiteBlockingWaiter;
    type Notifier = LiteBlockingNotifier;

    fn new_waiter(&self) -> Self::Waiter {
        LiteBlockingWaiter {
            inner: Arc::clone(&self.inner),
            spin_tries: self.spin_tries,
        }
    }

    fn notifier(&self) -> Self::Notifier {
        LiteBlockingNotifier {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Waiter for LiteBlockingWaiter {
    fn wait_for(
        &mut self,
        requested: Sequence,
        barrier: &impl Barrier,
        shutdown: &CachePadded<AtomicI64>,
    ) -> WaitOutcome {
        loop {
            return_if_available_or_shutdown!(requested, barrier, shutdown);

            for _ in 0..self.spin_tries {
                return_if_available_or_shutdown!(requested, barrier, shutdown);

                hint::spin_loop();
            }

            let mut epoch_guard = self.inner.mutex.lock();
            let epoch = *epoch_guard;
            while *epoch_guard == epoch {
                // Indicate to producers/advancing consumers that a signal might be needed.
                self.inner.signal_needed.store(true, Ordering::Release);

                return_if_available_or_shutdown!(requested, barrier, shutdown);

                self.inner.condvar.wait(&mut epoch_guard);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicI64, AtomicUsize},
            mpsc, Arc,
        },
        time::Duration,
    };

    struct TestBarrier {
        available: Arc<AtomicI64>,
    }

    impl TestBarrier {
        fn new(v: i64) -> Self {
            Self {
                available: Arc::new(AtomicI64::new(v)),
            }
        }
    }

    impl Barrier for TestBarrier {
        fn get_after(&self, _prev: Sequence) -> Sequence {
            self.available.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn yielding_waiter_resets_spin_budget_each_wait() {
        let mut waiter = YieldingWaiter {
            spins_remaining: 0, // force-reset to prove it is restored
        };
        let barrier = TestBarrier::new(5);
        let shutdown = CachePadded::new(AtomicI64::new(-1));

        // immediate availability: should reset and return without consuming spins
        let available = waiter.wait_for(5, &barrier, &shutdown);
        assert!(matches!(available, WaitOutcome::Available { upper: 5 }));
        assert_eq!(waiter.spins_remaining, SPIN_TRIES);

        // withhold availability briefly to consume budget, then release
        let barrier = TestBarrier::new(4); // less than requested
        let shutdown = CachePadded::new(AtomicI64::new(-1));
        let barrier_ref = barrier.available.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_micros(50));
            barrier_ref.store(6, Ordering::Relaxed);
        });
        let _ = waiter.wait_for(6, &barrier, &shutdown);
        handle.join().unwrap();

        // Next call should start with a fresh spin budget.
        let barrier = TestBarrier::new(7);
        let shutdown = CachePadded::new(AtomicI64::new(-1));
        let _ = waiter.wait_for(7, &barrier, &shutdown);
        assert_eq!(waiter.spins_remaining, SPIN_TRIES);
    }

    #[test]
    fn yielding_waiter_honors_shutdown() {
        let mut waiter = YieldingWaiter {
            spins_remaining: SPIN_TRIES,
        };
        let barrier = TestBarrier::new(-1);
        let shutdown = CachePadded::new(AtomicI64::new(10));

        let res = waiter.wait_for(10, &barrier, &shutdown);
        assert!(matches!(res, WaitOutcome::Shutdown));
    }

    #[test]
    fn busy_spin_waiter_observes_release_with_acquire_fence() {
        let mut waiter = BusySpinWaiter;
        let shutdown = Arc::new(CachePadded::new(AtomicI64::new(-1)));
        let data = Arc::new(AtomicI64::new(0));
        let barrier = TestBarrier::new(0);
        let barrier_for_thread = barrier.available.clone();
        let data_for_thread = data.clone();

        // Writer thread publishes data with Release before making sequence available.
        let handle = std::thread::spawn(move || {
            data_for_thread.store(1, Ordering::Release);
            barrier_for_thread.store(1, Ordering::Release);
        });

        // Wait for sequence 1; fence inside waiter should make the data visible.
        let available = waiter.wait_for(1, &barrier, shutdown.as_ref());
        assert!(matches!(available, WaitOutcome::Available { upper: 1 }));
        assert_eq!(data.load(Ordering::Acquire), 1);
        handle.join().unwrap();
    }

    #[test]
    fn busy_spin_waiter_exits_on_shutdown_race() {
        let mut waiter = BusySpinWaiter;
        let barrier = TestBarrier::new(-1);
        let shutdown = Arc::new(CachePadded::new(AtomicI64::new(-1)));

        // Flip shutdown while waiter would otherwise spin.
        let shutdown_ref = shutdown.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_micros(50));
            shutdown_ref.store(5, Ordering::Relaxed);
        });

        let res = waiter.wait_for(5, &barrier, shutdown.as_ref());
        assert!(matches!(res, WaitOutcome::Shutdown));
        handle.join().unwrap();
    }

    #[test]
    fn blocking_wait_strategy_wakes_waiters() {
        let barrier = TestBarrier::new(0);
        let shutdown = CachePadded::new(AtomicI64::new(-1));
        let strategy = BlockingWaitStrategy::default();
        let notifier = strategy.notifier();

        let (tx, rx) = mpsc::channel();
        let barrier_available = barrier.available.clone();
        std::thread::spawn(move || {
            let mut waiter = strategy.new_waiter();
            let res = waiter.wait_for(1, &barrier, &shutdown);
            tx.send(res).unwrap();
        });

        std::thread::sleep(Duration::from_millis(10));
        barrier_available.store(1, Ordering::Relaxed);
        notifier.wake();

        let outcome = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(outcome, WaitOutcome::Available { upper: 1 }));
    }

    #[test]
    fn blocking_wait_strategy_spin_window_can_avoid_sleep() {
        struct CountingBarrier {
            polls_before_ready: AtomicUsize,
        }

        impl CountingBarrier {
            fn new(polls_before_ready: usize) -> Self {
                Self {
                    polls_before_ready: AtomicUsize::new(polls_before_ready),
                }
            }
        }

        impl Barrier for CountingBarrier {
            fn get_after(&self, prev: Sequence) -> Sequence {
                if self.polls_before_ready.load(Ordering::Relaxed) == 0 {
                    return prev;
                }
                let _ = self.polls_before_ready.fetch_sub(1, Ordering::Relaxed);
                prev - 1
            }
        }

        let barrier = CountingBarrier::new(5);
        let shutdown = CachePadded::new(AtomicI64::new(-1));
        let strategy = BlockingWaitStrategy::default().with_spin_tries(16);
        let mut waiter = strategy.new_waiter();

        // Should make progress without any notifier wake() because it never blocks.
        let outcome = waiter.wait_for(1, &barrier, &shutdown);
        assert!(matches!(outcome, WaitOutcome::Available { upper: 1 }));
    }

    #[test]
    fn lite_blocking_wait_strategy_skips_wake_when_no_waiters() {
        let strategy = LiteBlockingWaitStrategy::default();
        let notifier = strategy.notifier();
        assert!(!strategy.inner.signal_needed.load(Ordering::Relaxed));

        // Should be a cheap no-op: nobody is waiting.
        notifier.wake();
        assert!(!strategy.inner.signal_needed.load(Ordering::Relaxed));
    }

    #[test]
    fn lite_blocking_wait_strategy_spin_window_can_avoid_indicating_signal_needed() {
        struct CountingBarrier {
            polls_before_ready: AtomicUsize,
        }

        impl CountingBarrier {
            fn new(polls_before_ready: usize) -> Self {
                Self {
                    polls_before_ready: AtomicUsize::new(polls_before_ready),
                }
            }
        }

        impl Barrier for CountingBarrier {
            fn get_after(&self, prev: Sequence) -> Sequence {
                if self.polls_before_ready.load(Ordering::Relaxed) == 0 {
                    return prev;
                }
                let _ = self.polls_before_ready.fetch_sub(1, Ordering::Relaxed);
                prev - 1
            }
        }

        let barrier = CountingBarrier::new(5);
        let shutdown = CachePadded::new(AtomicI64::new(-1));
        let strategy = LiteBlockingWaitStrategy::default().with_spin_tries(16);
        let mut waiter = strategy.new_waiter();

        // The barrier becomes available during the spin window, so the waiter should never take
        // the condvar path (and therefore never set `signal_needed`).
        let outcome = waiter.wait_for(1, &barrier, &shutdown);
        assert!(matches!(outcome, WaitOutcome::Available { upper: 1 }));
        assert!(!strategy.inner.signal_needed.load(Ordering::Relaxed));
    }

    #[test]
    fn lite_blocking_wait_strategy_wakes_waiters() {
        let barrier = TestBarrier::new(0);
        let shutdown = CachePadded::new(AtomicI64::new(-1));
        let strategy = LiteBlockingWaitStrategy::default();
        let notifier = strategy.notifier();

        let (tx, rx) = mpsc::channel();
        let barrier_available = barrier.available.clone();
        let strategy_for_thread = strategy.clone();
        std::thread::spawn(move || {
            let mut waiter = strategy_for_thread.new_waiter();
            let res = waiter.wait_for(1, &barrier, &shutdown);
            tx.send(res).unwrap();
        });

        // Give the waiter a chance to park.
        std::thread::sleep(Duration::from_millis(10));

        barrier_available.store(1, Ordering::Relaxed);
        notifier.wake();

        let outcome = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(outcome, WaitOutcome::Available { upper: 1 }));
    }
}
