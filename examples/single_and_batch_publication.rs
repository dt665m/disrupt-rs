use disruptor::*;

// The event on the ring buffer.
struct Event {
    price: f64,
}

fn main() {
    // Factory closure for initializing events in the Ring Buffer.
    let factory = || Event { price: 0.0 };

    // Closure for processing events.
    let processor = |e: &Event, sequence: Sequence, end_of_batch: bool| {
        let _ = (e.price, sequence, end_of_batch);
        // Your processing logic here.
    };

    let size = 64;
    let mut producer = disruptor::build_single_producer(size, factory, BusySpin)
        .handle_events_with(processor)
        .build();

    // Publish single events into the Disruptor via the `Producer` handle.
    for i in 0..10 {
        producer.publish(|e| {
            e.price = i as f64;
        });
    }

    // Publish a batch of events into the Disruptor.
    producer.batch_publish(5, |iter| {
        for e in iter {
            // `iter` is guaranteed to yield 5 events.
            e.price = 42.0;
        }
    });
    // When `producer` goes out of scope, the Disruptor will drain and shut down.
}
