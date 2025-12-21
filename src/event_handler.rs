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
