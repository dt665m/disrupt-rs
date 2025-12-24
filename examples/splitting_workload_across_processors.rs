use disruptor::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct Event {
    value: u64,
}

fn main() {
    let factory = Event::default;

    let handled_even = Arc::new(AtomicUsize::new(0));
    let handled_odd = Arc::new(AtomicUsize::new(0));

    let even_counter = handled_even.clone();
    let processor0 = move |e: &Event, sequence: Sequence, _end_of_batch: bool| {
        let _ = e.value;
        if sequence % 2 == 0 {
            even_counter.fetch_add(1, Ordering::Relaxed);
        }
    };

    let odd_counter = handled_odd.clone();
    let processor1 = move |e: &Event, sequence: Sequence, _end_of_batch: bool| {
        let _ = e.value;
        if sequence % 2 == 1 {
            odd_counter.fetch_add(1, Ordering::Relaxed);
        }
    };

    let mut producer = disruptor::build_single_producer(256, factory, BusySpin)
        .handle_events_with(processor0)
        .handle_events_with(processor1)
        .build();

    for i in 0..100u64 {
        producer.publish(|e| e.value = i);
    }
    drop(producer);

    let even = handled_even.load(Ordering::Relaxed);
    let odd = handled_odd.load(Ordering::Relaxed);
    println!("handled_even={even} handled_odd={odd} total={}", even + odd);
}
