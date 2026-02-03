use disrupt_rs::{build_multi_producer, BusySpin, ProcessorSettings, Producer, Sequence};
use std::thread;

struct Event {
    price: f64,
}

fn maybe_pin<B, E, W>(builder: B, core_id: Option<usize>) -> B
where
    B: ProcessorSettings<E, W>,
    W: disrupt_rs::wait_strategies::WaitStrategy,
{
    match core_id {
        Some(id) => builder.pin_at_core(id),
        None => builder,
    }
}

fn main() {
    let factory = || Event { price: 0.0 };

    let core_ids = core_affinity::get_core_ids();
    let core0 = core_ids.as_ref().and_then(|ids| ids.get(0)).map(|c| c.id);
    let core1 = core_ids.as_ref().and_then(|ids| ids.get(1)).map(|c| c.id);
    let core2 = core_ids.as_ref().and_then(|ids| ids.get(2)).map(|c| c.id);

    // Closure for processing events.
    let h1 = |e: &Event, sequence: Sequence, end_of_batch: bool| {
        let _ = (e.price, sequence, end_of_batch);
    };
    let h2 = |e: &Event, sequence: Sequence, end_of_batch: bool| {
        let _ = (e.price, sequence, end_of_batch);
    };
    let h3 = |e: &Event, sequence: Sequence, end_of_batch: bool| {
        let _ = (e.price, sequence, end_of_batch);
    };

    let builder =
        maybe_pin(build_multi_producer(64, factory, BusySpin), core0).handle_events_with(h1);
    let builder = maybe_pin(builder, core1).handle_events_with(h2);
    let builder = builder.and_then();
    let builder = maybe_pin(builder, core2).handle_events_with(h3);

    let mut producer1 = builder.build();

    // Create another producer.
    let mut producer2 = producer1.clone();

    // Publish into the Disruptor.
    thread::scope(|s| {
        s.spawn(move || {
            for i in 0..10 {
                producer1.publish(|e| {
                    e.price = i as f64;
                });
            }
        });
        s.spawn(move || {
            for i in 10..20 {
                producer2.publish(|e| {
                    e.price = i as f64;
                });
            }
        });
    });
    // Producers drop here -> Disruptor drains and shuts down.
}
