//! Module for structs for building a Multi Producer Disruptor in a type safe way.
//!
//! To get started building a Multi Producer Disruptor, invoke [super::build_multi_producer].

use std::{marker::PhantomData, sync::Arc};

use crate::{
    barrier::Barrier,
    builder::ProcessorSettings,
    consumer::{
        event_poller::EventPoller, MultiConsumerBarrier, MultiConsumerDependentsBarrier,
        SingleConsumerBarrier,
    },
    event_handler::{EventHandler, EventHandlerWithState},
    producer::multi::{MultiProducer, MultiProducerBarrier},
    sequence::DependentSequence,
    sequence::GatedSequence,
    wait_strategies::WaitStrategy,
};

use super::{Builder, Shared, MC, NC, SC};

type GateArray<const N: usize, W> = [GatedSequence<<W as WaitStrategy>::Notifier>; N];
type MPGateResult<S, E, W, B, const N: usize> = (MPBuilder<S, E, W, B>, GateArray<N, W>);
type MPPollerResult<S, E, W, B> = (
    EventPoller<E, B, <W as WaitStrategy>::Notifier>,
    MPBuilder<S, E, W, B>,
);

/// First step in building a Disruptor with a [MultiProducer].
pub struct MPBuilder<State, E, W, B>
where
    W: WaitStrategy,
{
    state: PhantomData<State>,
    shared: Shared<E, W>,
    producer_barrier: Arc<MultiProducerBarrier>,
    dependent_barrier: Arc<B>,
}

impl<S, E, W, B> ProcessorSettings<E, W> for MPBuilder<S, E, W, B>
where
    W: WaitStrategy,
{
    fn shared(&mut self) -> &mut Shared<E, W> {
        &mut self.shared
    }
}

impl<S, E, W, B> Builder<E, W, B> for MPBuilder<S, E, W, B>
where
    E: 'static + Send + Sync,
    W: 'static + WaitStrategy,
    B: 'static + Barrier,
{
    fn dependent_barrier(&self) -> Arc<B> {
        Arc::clone(&self.dependent_barrier)
    }
}

impl<S, E, W, B> MPBuilder<S, E, W, B>
where
    E: 'static + Send + Sync,
    W: 'static + WaitStrategy,
    B: 'static + Barrier,
{
    /// Create new gated sequences tied to this builder's notifier.
    ///
    /// Use the returned gates to advance external dependencies. They automatically wake blocked
    /// downstream consumers when advanced.
    pub fn new_gates<const N: usize>(self) -> MPGateResult<S, E, W, B, N> {
        let notifier = self.shared.notifier.clone();
        let gating: [DependentSequence; N] = std::array::from_fn(|_| DependentSequence::new());
        let gated_sequences: [GatedSequence<W::Notifier>; N] =
            std::array::from_fn(|i| GatedSequence::new(gating[i].clone(), notifier.clone()));

        (
            MPBuilder {
                state: PhantomData,
                shared: self.shared,
                producer_barrier: self.producer_barrier,
                dependent_barrier: self.dependent_barrier,
            },
            gated_sequences,
        )
    }
}

impl<E, W, B> MPBuilder<NC, E, W, B>
where
    E: 'static + Send + Sync,
    W: 'static + WaitStrategy,
    B: 'static + Barrier,
{
    pub(super) fn new<F>(
        size: usize,
        event_factory: F,
        wait_strategy: W,
        producer_barrier: Arc<MultiProducerBarrier>,
        dependent_barrier: Arc<B>,
    ) -> Self
    where
        F: FnMut() -> E,
    {
        let shared = Shared::new(size, event_factory, wait_strategy);
        Self {
            state: PhantomData,
            shared,
            producer_barrier,
            dependent_barrier,
        }
    }

    /// Get an EventPoller.
    pub fn event_poller(mut self) -> MPPollerResult<SC, E, W, B> {
        let event_poller = self.get_event_poller();

        (
            event_poller,
            MPBuilder {
                state: PhantomData,
                shared: self.shared,
                producer_barrier: self.producer_barrier,
                dependent_barrier: self.dependent_barrier,
            },
        )
    }

    /// Add an event handler.
    pub fn handle_events_with<EH>(mut self, event_handler: EH) -> MPBuilder<SC, E, W, B>
    where
        EH: 'static + EventHandler<E>,
    {
        self.add_event_handler(event_handler);
        MPBuilder {
            state: PhantomData,
            shared: self.shared,
            producer_barrier: self.producer_barrier,
            dependent_barrier: self.dependent_barrier,
        }
    }

    /// Add an event handler with a per-consumer state value.
    ///
    /// `initialize_state` is moved into the consumer thread and invoked there, which allows the
    /// produced `S` to be `!Send` (as long as it stays thread-local). See [`EventHandlerWithState`]
    /// for more details.
    pub fn handle_events_and_state_with<EH, S, IS>(
        mut self,
        event_handler: EH,
        initialize_state: IS,
    ) -> MPBuilder<SC, E, W, B>
    where
        EH: 'static + EventHandlerWithState<E, S>,
        IS: 'static + Send + FnOnce() -> S,
    {
        self.add_event_handler_with_state(event_handler, initialize_state);
        MPBuilder {
            state: PhantomData,
            shared: self.shared,
            producer_barrier: self.producer_barrier,
            dependent_barrier: self.dependent_barrier,
        }
    }
}

impl<E, W, B> MPBuilder<SC, E, W, B>
where
    E: 'static + Send + Sync,
    W: 'static + WaitStrategy,
    B: 'static + Barrier,
{
    /// Get an EventPoller.
    pub fn event_poller(mut self) -> MPPollerResult<MC, E, W, B> {
        let event_poller = self.get_event_poller();

        (
            event_poller,
            MPBuilder {
                state: PhantomData,
                shared: self.shared,
                producer_barrier: self.producer_barrier,
                dependent_barrier: self.dependent_barrier,
            },
        )
    }

    /// Add an event handler.
    pub fn handle_events_with<EH>(mut self, event_handler: EH) -> MPBuilder<MC, E, W, B>
    where
        EH: 'static + EventHandler<E>,
    {
        self.add_event_handler(event_handler);
        MPBuilder {
            state: PhantomData,
            shared: self.shared,
            producer_barrier: self.producer_barrier,
            dependent_barrier: self.dependent_barrier,
        }
    }

    /// Add an event handler with a per-consumer state value.
    ///
    /// `initialize_state` is moved into the consumer thread and invoked there, which allows the
    /// produced `S` to be `!Send` (as long as it stays thread-local). See [`EventHandlerWithState`]
    /// for more details.
    pub fn handle_events_and_state_with<EH, S, IS>(
        mut self,
        event_handler: EH,
        initialize_state: IS,
    ) -> MPBuilder<MC, E, W, B>
    where
        EH: 'static + EventHandlerWithState<E, S>,
        IS: 'static + Send + FnOnce() -> S,
    {
        self.add_event_handler_with_state(event_handler, initialize_state);
        MPBuilder {
            state: PhantomData,
            shared: self.shared,
            producer_barrier: self.producer_barrier,
            dependent_barrier: self.dependent_barrier,
        }
    }

    /// Complete the (concurrent) consumption of events so far and let new consumers process
    /// events after all previous consumers have read them.
    pub fn and_then(mut self) -> MPBuilder<NC, E, W, SingleConsumerBarrier> {
        // Guaranteed to be present by construction.
        let consumer_cursors = self.shared().current_consumer_cursors.as_mut().unwrap();
        let dependent_barrier = Arc::new(SingleConsumerBarrier::new(consumer_cursors.remove(0)));

        MPBuilder {
            state: PhantomData,
            shared: self.shared,
            producer_barrier: self.producer_barrier,
            dependent_barrier,
        }
    }

    /// Finish the build and get a [`MultiProducer`].
    pub fn build(mut self) -> MultiProducer<E, SingleConsumerBarrier, W> {
        let mut consumer_cursors = self.shared().current_consumer_cursors.take().unwrap();
        // Guaranteed to be present by construction.
        let consumer_barrier = SingleConsumerBarrier::new(consumer_cursors.remove(0));
        MultiProducer::new(
            self.shared.shutdown_at_sequence,
            self.shared.ring_buffer,
            self.producer_barrier,
            self.shared.consumers,
            consumer_barrier,
            self.shared.notifier.clone(),
        )
    }
}

impl<E, W, B> MPBuilder<MC, E, W, B>
where
    E: 'static + Send + Sync,
    W: 'static + WaitStrategy,
    B: 'static + Barrier,
{
    /// Get an EventPoller.
    pub fn event_poller(mut self) -> MPPollerResult<MC, E, W, B> {
        let event_poller = self.get_event_poller();

        (
            event_poller,
            MPBuilder {
                state: PhantomData,
                shared: self.shared,
                producer_barrier: self.producer_barrier,
                dependent_barrier: self.dependent_barrier,
            },
        )
    }

    /// Add an event handler.
    pub fn handle_events_with<EH>(mut self, event_handler: EH) -> MPBuilder<MC, E, W, B>
    where
        EH: 'static + EventHandler<E>,
    {
        self.add_event_handler(event_handler);
        self
    }

    /// Add an event handler with a per-consumer state value.
    ///
    /// `initialize_state` is moved into the consumer thread and invoked there, which allows the
    /// produced `S` to be `!Send` (as long as it stays thread-local). See [`EventHandlerWithState`]
    /// for more details.
    pub fn handle_events_and_state_with<EH, S, IS>(
        mut self,
        event_handler: EH,
        initialize_state: IS,
    ) -> MPBuilder<MC, E, W, B>
    where
        EH: 'static + EventHandlerWithState<E, S>,
        IS: 'static + Send + FnOnce() -> S,
    {
        self.add_event_handler_with_state(event_handler, initialize_state);
        self
    }

    /// Complete the (concurrent) consumption of events so far and let new consumers process
    /// events after all previous consumers have read them.
    pub fn and_then(mut self) -> MPBuilder<NC, E, W, MultiConsumerBarrier> {
        let consumer_cursors = self
            .shared()
            .current_consumer_cursors
            .replace(vec![])
            .unwrap();
        let dependent_barrier = Arc::new(MultiConsumerBarrier::new(consumer_cursors));

        MPBuilder {
            state: PhantomData,
            shared: self.shared,
            producer_barrier: self.producer_barrier,
            dependent_barrier,
        }
    }

    /// Complete consumption of events so far and gate the next stage on the slowest consumer *and*
    /// the provided gated sequences (e.g., flushed WAL offset).
    ///
    /// Use the same gates you got from [`new_gates`](Self::new_gates) to advance external
    /// dependencies. They automatically wake blocked downstream consumers.
    pub fn and_then_with_dependents(
        mut self,
        gates: &[GatedSequence<W::Notifier>],
    ) -> MPBuilder<NC, E, W, MultiConsumerDependentsBarrier> {
        let consumer_cursors = self
            .shared()
            .current_consumer_cursors
            .replace(vec![])
            .unwrap();
        let gating: Vec<DependentSequence> = gates.iter().map(|gate| gate.dependent()).collect();

        let dependent_barrier = Arc::new(MultiConsumerDependentsBarrier::new(
            consumer_cursors,
            gating,
        ));

        MPBuilder {
            state: PhantomData,
            shared: self.shared,
            producer_barrier: self.producer_barrier,
            dependent_barrier,
        }
    }

    /// Finish the build and get a [`MultiProducer`].
    pub fn build(mut self) -> MultiProducer<E, MultiConsumerBarrier, W> {
        let consumer_cursors = self.shared().current_consumer_cursors.take().unwrap();
        let consumer_barrier = MultiConsumerBarrier::new(consumer_cursors);
        MultiProducer::new(
            self.shared.shutdown_at_sequence,
            self.shared.ring_buffer,
            self.producer_barrier,
            self.shared.consumers,
            consumer_barrier,
            self.shared.notifier.clone(),
        )
    }
}
