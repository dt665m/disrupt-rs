use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, BenchmarkId, Criterion,
    Throughput,
};
use disrupt_rs::{BusySpin, LiteBlockingWaitStrategy, Producer};
use std::{
    hint::{black_box, spin_loop},
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
    time::Instant,
};

const RING_SIZE: usize = 1024;
const EVENTS_PER_ITER: usize = 16_384;
const BATCH_SIZE: usize = 64;
const GAP_SPINS: [u32; 4] = [0, 50, 200, 1_000];
const SPIN_TRIES: [u32; 3] = [0, 1_000, 10_000];

#[derive(Default)]
struct Event {
    data: i64,
}

fn producer_gap(spins: u32) {
    for _ in 0..spins {
        spin_loop();
    }
}

fn bench_busyspin(group: &mut BenchmarkGroup<WallTime>, gap_spins: u32) {
    let sink = Arc::new(AtomicI64::new(0));
    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &Event, _sequence: i64, end_of_batch: bool| {
            // We publish `data = 1` only for the last element of the iteration; track only that
            // single event to avoid per-event cross-thread contention.
            if end_of_batch && event.data == 1 {
                sink.store(1, Ordering::Relaxed);
            }
        }
    };

    let mut producer = disrupt_rs::build_single_producer(RING_SIZE, Event::default, BusySpin)
        .handle_events_with(processor)
        .build();

    let bench_id = BenchmarkId::new("busyspin", format!("gap_spins={}", gap_spins));
    group.throughput(Throughput::Elements(EVENTS_PER_ITER as u64));
    group.bench_function(bench_id, move |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                sink.store(0, Ordering::Relaxed);
                for batch in 0..(EVENTS_PER_ITER / BATCH_SIZE) {
                    let last_batch = batch + 1 == (EVENTS_PER_ITER / BATCH_SIZE);
                    producer.batch_publish(BATCH_SIZE, |iter| {
                        for (i, e) in iter.enumerate() {
                            let last = last_batch && (i + 1 == BATCH_SIZE);
                            e.data = black_box(if last { 1 } else { 0 });
                        }
                    });
                    producer_gap(gap_spins);
                }
                while sink.load(Ordering::Relaxed) != 1 {
                    spin_loop();
                }
            }
            start.elapsed()
        })
    });
}

fn bench_liteblocking_spin(group: &mut BenchmarkGroup<WallTime>, gap_spins: u32, spin_tries: u32) {
    let sink = Arc::new(AtomicI64::new(0));
    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &Event, _sequence: i64, end_of_batch: bool| {
            if end_of_batch && event.data == 1 {
                sink.store(1, Ordering::Relaxed);
            }
        }
    };

    // The goal is to create a workload where the consumer frequently reaches the end of available
    // data, but the producer publishes again quickly enough that the waiter typically satisfies
    // the next wait during the spin window (i.e., it doesn't sleep).
    let strategy = LiteBlockingWaitStrategy::default().with_spin_tries(spin_tries);
    let mut producer = disrupt_rs::build_single_producer(RING_SIZE, Event::default, strategy)
        .handle_events_with(processor)
        .build();

    let bench_id = BenchmarkId::new(
        "liteblocking_spin",
        format!("gap_spins={}, spin_tries={}", gap_spins, spin_tries),
    );
    group.throughput(Throughput::Elements(EVENTS_PER_ITER as u64));
    group.bench_function(bench_id, move |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                sink.store(0, Ordering::Relaxed);
                for batch in 0..(EVENTS_PER_ITER / BATCH_SIZE) {
                    let last_batch = batch + 1 == (EVENTS_PER_ITER / BATCH_SIZE);
                    producer.batch_publish(BATCH_SIZE, |iter| {
                        for (i, e) in iter.enumerate() {
                            let last = last_batch && (i + 1 == BATCH_SIZE);
                            e.data = black_box(if last { 1 } else { 0 });
                        }
                    });
                    producer_gap(gap_spins);
                }
                while sink.load(Ordering::Relaxed) != 1 {
                    spin_loop();
                }
            }
            start.elapsed()
        })
    });
}

pub fn spin_window_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("spin_window_spsc");

    for gap_spins in GAP_SPINS {
        bench_busyspin(&mut group, gap_spins);
        for spin_tries in SPIN_TRIES {
            bench_liteblocking_spin(&mut group, gap_spins, spin_tries);
        }
    }

    group.finish();
}

criterion_group!(spin_window, spin_window_benchmark);
criterion_main!(spin_window);
