use disrupt_rs::{build_single_producer, BusySpin, Producer, Sequence};
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

    let (builder, [flushed]) = build_single_producer(1024, factory, BusySpin).new_gates::<1>();
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

    let builder = builder
        .handle_events_with(journal) // stage 1
        .and_then_with_dependents(&[flushed.clone()]); // stage boundary gated by external seq
    let mut producer = builder.handle_events_with(engine).build(); // stage 2

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
