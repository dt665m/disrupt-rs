use disrupt_rs::{build_single_producer, BusySpin, DependentSequence, Producer, Sequence};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[derive(Default)]
struct Event {
    value: u64,
}

fn wal_append(_event: &Event) {
    // append event to a write-ahead log
}

fn wal_fsync() -> std::io::Result<()> {
    // fsync/fdatasync/commit
    Ok(())
}

fn apply_business_logic(_event: &Event) {
    // do something with the event
}

fn main() {
    let factory = Event::default;

    // External gate the handler will advance after fsync/ack.
    let flushed = Arc::new(DependentSequence::new());

    let last_flushed = Arc::new(AtomicU64::new(0));
    let last_applied = Arc::new(AtomicU64::new(0));

    let journal = {
        let flushed = flushed.clone();
        let last_flushed = last_flushed.clone();
        move |event: &Event, seq: Sequence, _eob: bool| {
            wal_append(event);
            wal_fsync().unwrap();
            flushed.set(seq); // advance only after persistence
            last_flushed.store(event.value, Ordering::Relaxed);
        }
    };

    let engine = {
        let last_applied = last_applied.clone();
        move |event: &Event, _seq: Sequence, _eob: bool| {
            apply_business_logic(event);
            last_applied.store(event.value, Ordering::Relaxed);
        }
    };

    let mut producer = build_single_producer(1024, factory, BusySpin)
        .handle_events_with(journal) // stage 1
        .and_then_with_dependents(vec![flushed.clone()]) // stage boundary gated by external seq
        .handle_events_with(engine) // stage 2
        .build();

    for i in 0..10u64 {
        producer.publish(|e| e.value = i);
    }
    drop(producer);

    println!(
        "last_flushed={} last_applied={}",
        last_flushed.load(Ordering::Relaxed),
        last_applied.load(Ordering::Relaxed)
    );
}
