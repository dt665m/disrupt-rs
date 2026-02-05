use crate::wait_strategies::WakeupNotifier;
use crossbeam_utils::CachePadded;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};

/// A sequence that can be advanced by external actors (e.g. fsync flush markers).
///
/// Defaults to `-1`, matching the initial cursor used internally so a gated stage
/// is blocked until explicitly advanced. Cache-line padded to reduce false sharing.
///
/// This type is cheaply cloneable - clones share the same underlying atomic sequence.
#[derive(Clone, Debug)]
pub(crate) struct DependentSequence(Arc<CachePadded<AtomicI64>>);

impl DependentSequence {
    /// Create a new sequence initialized to `-1`.
    pub(crate) fn new() -> Self {
        Self::with_value(-1)
    }

    /// Create a sequence with the given initial value (power user escape hatch).
    pub(crate) fn with_value(v: i64) -> Self {
        Self(Arc::new(CachePadded::new(AtomicI64::new(v))))
    }

    /// Load the current value with `Acquire` ordering.
    pub(crate) fn get(&self) -> i64 {
        self.0.load(Ordering::Acquire)
    }

    /// Store a new value with `Release` ordering.
    pub(crate) fn set(&self, v: i64) {
        self.0.store(v, Ordering::Release);
    }

    /// Compare and set the value with `AcqRel`/`Acquire` ordering.
    pub(crate) fn compare_and_set(&self, current: i64, new: i64) -> Result<i64, i64> {
        self.0
            .compare_exchange(current, new, Ordering::AcqRel, Ordering::Acquire)
    }
}

impl Default for DependentSequence {
    fn default() -> Self {
        Self::new()
    }
}

/// A gated sequence that automatically wakes blocked consumers when advanced.
///
/// This wraps a [`DependentSequence`] with a notifier, ensuring that any consumers
/// blocked on this gate (when using blocking wait strategies) are woken up when
/// the sequence is advanced via [`set()`](Self::set).
///
/// Returned by [`and_then_with_dependents`](crate::builder::single::SPBuilder::and_then_with_dependents)
/// to ensure correct behavior with all wait strategies.
#[derive(Clone)]
pub struct GatedSequence<N> {
    inner: DependentSequence,
    notifier: N,
}

impl<N> GatedSequence<N> {
    /// Create a new gated sequence from a `DependentSequence` and notifier.
    pub(crate) fn new(inner: DependentSequence, notifier: N) -> Self {
        Self { inner, notifier }
    }

    /// Load the current value with `Acquire` ordering.
    pub fn get(&self) -> i64 {
        self.inner.get()
    }

    /// Returns a reference to the underlying `DependentSequence`.
    pub(crate) fn dependent(&self) -> DependentSequence {
        self.inner.clone()
    }
}

impl<N: WakeupNotifier> GatedSequence<N> {
    /// Store a new value with `Release` ordering and wake any blocked consumers.
    ///
    /// This is the primary method for advancing the gate. It ensures that consumers
    /// using blocking wait strategies are properly notified.
    pub fn set(&self, v: i64) {
        self.inner.set(v);
        self.notifier.wake();
    }

    /// Compare and set the value, waking consumers only on success.
    pub fn compare_and_set(&self, current: i64, new: i64) -> Result<i64, i64> {
        let result = self.inner.compare_and_set(current, new);
        if result.is_ok() {
            self.notifier.wake();
        }
        result
    }
}
