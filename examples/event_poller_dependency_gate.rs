use disrupt_rs::wait_strategies::WaitStrategy;
use disrupt_rs::{build_single_producer, BusySpin, Polling, Producer, Sequence};

#[derive(Debug, Default)]
struct Event {
    value: u64,
}

fn wal_append(_seq: Sequence, _e: &Event) {
    // write record bytes to a log buffer / file
}

fn wal_fsync() {
    // ensure durability (fsync / fdatasync / commit)
}

fn main() {
    // Stage 1: poller (user-managed).
    let (mut poller, builder) =
        build_single_producer(1024, Event::default, BusySpin).event_poller();

    let (builder, [gated_gate]) = builder.new_gates::<1>();
    let builder = builder.and_then_with_dependents(&[gated_gate.clone()]);

    let mut producer = builder
        .handle_events_with(|e: &Event, seq: Sequence, _eob: bool| {
            println!("downstream sees seq={seq} value={}", e.value);
        })
        .build();

    // Publish some events.
    for i in 0..10u64 {
        producer.publish(|e| e.value = i);
    }
    drop(producer); // signal shutdown once drained

    let mut waiter = BusySpin.new_waiter();

    loop {
        match poller.poll_wait(&mut waiter) {
            Ok(mut events) => {
                let mut last_seq: Option<Sequence> = None;

                // This yields the actual ringbuffer sequence for each event.
                for (seq, e) in &mut events {
                    wal_append(seq, e);
                    last_seq = Some(seq);
                }

                if let Some(last_seq) = last_seq {
                    wal_fsync(); // durability boundary
                    gated_gate.set(last_seq); // release downstream up to `last_seq` (auto-wakes)
                }
                // `events` dropped here -> poller cursor advances to the polled upper bound.
            }
            Err(Polling::NoEvents) => continue,
            Err(Polling::Shutdown) => break,
        }
    }
}
