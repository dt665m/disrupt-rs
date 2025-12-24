use disruptor::*;
use std::thread;

// The event on the ring buffer.
struct Event {
    price: f64,
}

fn main() {
    // Factory closure for initializing events in the Ring Buffer.
    let factory = || Event { price: 0.0 };

    let size = 64;
    let builder = disruptor::build_single_producer(size, factory, BusySpin);
    let (mut poller, builder) = builder.event_poller();
    let mut producer = builder.build();

    // Publish single events into the Disruptor via the `Producer` handle.
    for i in 0..10 {
        producer.publish(|e| {
            e.price = i as f64;
        });
    }
    drop(producer); // signal shutdown once drained

    loop {
        match poller.poll() {
            Ok(mut events) => {
                // `&mut events` implements ExactSizeIterator so events can be
                // batch processed and handled with e.g. a for loop.
                for (sequence, event) in &mut events {
                    let _ = (sequence, event.price);
                }
            } // dropping `events` signals that reading is done
            Err(Polling::NoEvents) => {
                thread::yield_now();
            }
            Err(Polling::Shutdown) => break,
        }
    }
}
