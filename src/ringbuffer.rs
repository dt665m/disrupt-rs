use std::{cell::UnsafeCell, ptr::NonNull};

use crate::Sequence;

unsafe impl<E: Sync> Sync for RingBuffer<E> {}

#[doc(hidden)]
pub struct RingBuffer<E> {
    slots: Box<[UnsafeCell<E>]>,
    capacity: Sequence,
    slot_mask: Sequence,
}

impl<E> RingBuffer<E> {
    pub(crate) fn new<F>(size: usize, mut event_factory: F) -> Self
    where
        F: FnMut() -> E,
    {
        if !size.is_power_of_two() {
            panic!("Size must be power of 2.")
        }

        let slots: Box<[UnsafeCell<E>]> = (0..size)
            .map(|_i| UnsafeCell::new(event_factory()))
            .collect();
        let capacity = size as Sequence;
        let slot_mask = (size - 1) as Sequence;

        RingBuffer {
            slots,
            capacity,
            slot_mask,
        }
    }

    #[inline]
    fn wrap_point(&self, sequence: Sequence) -> Sequence {
        sequence - self.capacity()
    }

    #[inline(always)]
    fn slot_index(&self, sequence: Sequence) -> usize {
        debug_assert!(
            sequence >= 0,
            "sequence must be non-negative when indexing into the ring buffer"
        );
        (sequence & self.slot_mask) as usize
    }

    #[inline]
    pub(crate) fn available_capacity(
        &self,
        producer: Sequence,
        highest_read_by_consumers: Sequence,
    ) -> i64 {
        let wrap_point = self.wrap_point(producer);
        highest_read_by_consumers - wrap_point
    }

    /// Callers must ensure that only a single mutable reference or multiple immutable references
    /// exist at any point in time.
    #[inline]
    pub(crate) unsafe fn get(&self, sequence: Sequence) -> NonNull<E> {
        let index = self.slot_index(sequence);
        // SAFETY: Index is within bounds - guaranteed by invariant and slot mask.
        let slot = unsafe { self.slots.get_unchecked(index) };
        // SAFETY: `UnsafeCell::get()` returns a non-null pointer to the slot storage.
        unsafe { NonNull::new_unchecked(slot.get()) }
    }

    #[inline]
    pub(crate) fn capacity(&self) -> Sequence {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_capacity() {
        let ring_buffer = RingBuffer::new(8, || 0);

        // Round 1:
        // Publisher has just published 7 and consumer has read 0.
        assert_eq!(1, ring_buffer.available_capacity(7, 0));
        // Consumer had read 0 and the publisher has just published 8.
        assert_eq!(0, ring_buffer.available_capacity(8, 0));
        // Producer has published 0 and comsumer read 0.
        assert_eq!(8, ring_buffer.available_capacity(0, 0));
        // Publisher has just published 3 and consumer is (still) reading 0.
        assert_eq!(4, ring_buffer.available_capacity(3, -1));
        // Publisher has just published 7 and consumer is (still) reading 0.
        assert_eq!(0, ring_buffer.available_capacity(7, -1));
        // Publisher has just published 7 and consumer has read 0.
        assert_eq!(1, ring_buffer.available_capacity(7, 0));
        // Publisher has just released 5 and consumer has read 2.
        assert_eq!(5, ring_buffer.available_capacity(5, 2));
        // Publisher has just released 5 and consumer has read 3.
        assert_eq!(6, ring_buffer.available_capacity(5, 3));
        // Publisher has just released 5 and consumer has read 4.
        assert_eq!(7, ring_buffer.available_capacity(5, 4));
        // Publisher has just released 5 and consumer has read 5.
        assert_eq!(8, ring_buffer.available_capacity(5, 5));

        // Round 2:
        // Publisher has just published 11 and consumer has read 9.
        assert_eq!(6, ring_buffer.available_capacity(11, 9));
        // Publisher has just published 12 and consumer has read 12.
        assert_eq!(8, ring_buffer.available_capacity(12, 12));
        // Consumer has read 7 and the publisher has just published 15.
        assert_eq!(0, ring_buffer.available_capacity(15, 7));
    }
}
