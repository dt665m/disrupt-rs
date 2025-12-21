use disruptor::{
    build_multi_producer, build_single_producer, EventHandler, EventHandlerWithState, Producer,
    YieldingWaitStrategy,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct Event {
    value: usize,
}

struct CountingHandler {
    seen: Arc<AtomicUsize>,
}

impl EventHandler<Event> for CountingHandler {
    #[inline]
    fn on_event(&mut self, _event: &Event, _sequence: i64, _end_of_batch: bool) {
        self.seen.fetch_add(1, Ordering::Relaxed);
    }
}

struct State {
    local: usize,
    published: Arc<AtomicUsize>,
}

struct StatefulHandler;

impl EventHandlerWithState<Event, State> for StatefulHandler {
    #[inline]
    fn on_event(&mut self, state: &mut State, _event: &Event, _sequence: i64, _end_of_batch: bool) {
        state.local += 1;
        // Make state mutation observable outside the consumer thread.
        state.published.store(state.local, Ordering::Relaxed);
    }
}

#[test]
fn struct_event_handler_works() {
    let seen = Arc::new(AtomicUsize::new(0));
    let handler = CountingHandler { seen: seen.clone() };

    let mut producer = build_single_producer(8, Event::default, YieldingWaitStrategy)
        .handle_events_with(handler)
        .build();

    for i in 0..128 {
        producer.publish(|e| e.value = i);
    }

    drop(producer); // joins consumer threads
    assert_eq!(seen.load(Ordering::Relaxed), 128);
}

#[test]
fn stateful_struct_handler_works_and_mutates_state() {
    let published = Arc::new(AtomicUsize::new(0));

    let mut producer = build_single_producer(8, Event::default, YieldingWaitStrategy)
        .handle_events_and_state_with(StatefulHandler, {
            let published = published.clone();
            move || State {
                local: 0,
                published,
            }
        })
        .build();

    for i in 0..77 {
        producer.publish(|e| e.value = i);
    }

    drop(producer);
    assert_eq!(published.load(Ordering::Relaxed), 77);
}

#[test]
fn stateful_closure_handler_works() {
    let published = Arc::new(AtomicUsize::new(0));

    let mut producer = build_single_producer(8, Event::default, YieldingWaitStrategy)
        .handle_events_and_state_with(
            |state: &mut State, _e: &Event, _seq, _eob| {
                state.local += 1;
                state.published.store(state.local, Ordering::Relaxed);
            },
            {
                let published = published.clone();
                move || State {
                    local: 0,
                    published,
                }
            },
        )
        .build();

    for i in 0..33 {
        producer.publish(|e| e.value = i);
    }

    drop(producer);
    assert_eq!(published.load(Ordering::Relaxed), 33);
}

#[test]
fn multi_producer_with_mixed_handlers_works() {
    let seen = Arc::new(AtomicUsize::new(0));
    let published = Arc::new(AtomicUsize::new(0));

    let producer = build_multi_producer(64, Event::default, YieldingWaitStrategy)
        .handle_events_with(CountingHandler { seen: seen.clone() })
        .handle_events_and_state_with(StatefulHandler, {
            let published = published.clone();
            move || State {
                local: 0,
                published,
            }
        })
        .build();

    let mut producer1 = producer.clone();
    let mut producer2 = producer;

    let t1 = std::thread::spawn(move || {
        for i in 0..1000 {
            producer1.publish(|e| e.value = i);
        }
        // drop producer1
    });
    let t2 = std::thread::spawn(move || {
        for i in 1000..2000 {
            producer2.publish(|e| e.value = i);
        }
        // drop producer2 (last one)
    });

    t1.join().unwrap();
    t2.join().unwrap();

    // CountingHandler sees all events.
    assert_eq!(seen.load(Ordering::Relaxed), 2000);
    // StatefulHandler stores a running total; final value should match total events.
    assert_eq!(published.load(Ordering::Relaxed), 2000);
}
