use crate::Sequence;

/// Trait for processing events on consumer threads.
///
/// This is used by `handle_events_with` so handlers can be either closures or dedicated structs,
/// without introducing dynamic dispatch.
pub trait EventHandler<E>: Send {
    /// Called for each published event.
    fn on_event(&mut self, event: &E, sequence: Sequence, end_of_batch: bool);
}

impl<E, F> EventHandler<E> for F
where
    F: FnMut(&E, Sequence, bool) + Send,
{
    #[inline]
    fn on_event(&mut self, event: &E, sequence: Sequence, end_of_batch: bool) {
        (self)(event, sequence, end_of_batch)
    }
}

/// Trait for processing events with a separate mutable state value.
///
/// This is used by `handle_events_and_state_with` so handlers can be closures or dedicated structs,
/// without introducing dynamic dispatch.
///
/// # Why a separate state value?
///
/// Consumer threads are started with `std::thread::spawn`, which requires that anything moved into
/// the spawned thread is `Send`. That means the *handler* must be `Send`.
///
/// However, sometimes you want per-consumer state that is *thread-scoped* and therefore does not
/// need to be `Send` (for example, `Rc`, `RefCell`, or other single-threaded arenas/allocators).
/// You cannot store such a `!Send` value inside the handler struct itself, because then the handler
/// becomes `!Send` and cannot be moved into the consumer thread.
///
/// `handle_events_and_state_with` takes an `initialize_state` closure (which *is* `Send`) and calls
/// it **inside** the consumer thread after it starts, so the produced state can safely be `!Send`
/// as long as it never leaves that thread.
///
/// ## Example: `Rc<RefCell<_>>` state
/// ```no_run
/// use std::{cell::RefCell, rc::Rc};
///
/// use disrupt_rs::{build_single_producer, BusySpin, Producer, Sequence};
///
/// #[derive(Default)]
/// struct State {
///     // `Rc` is `!Send`, so `State` is `!Send` too.
///     counter: Rc<RefCell<u64>>,
/// }
///
/// #[derive(Default)]
/// struct Event {
///     value: u64,
/// }
///
/// let mut producer = build_single_producer(64, Event::default, BusySpin)
///     .handle_events_and_state_with(
///         |state: &mut State, _event: &Event, _seq: Sequence, _end: bool| {
///             *state.counter.borrow_mut() += 1;
///         },
///         State::default, // constructed inside the consumer thread
///     )
///     .build();
///
/// producer.publish(|e| e.value = 123);
/// drop(producer);
/// ```
pub trait EventHandlerWithState<E, S>: Send {
    /// Called for each published event with mutable access to the state.
    fn on_event(&mut self, state: &mut S, event: &E, sequence: Sequence, end_of_batch: bool);
}

impl<E, S, F> EventHandlerWithState<E, S> for F
where
    F: FnMut(&mut S, &E, Sequence, bool) + Send,
{
    #[inline]
    fn on_event(&mut self, state: &mut S, event: &E, sequence: Sequence, end_of_batch: bool) {
        (self)(state, event, sequence, end_of_batch)
    }
}
