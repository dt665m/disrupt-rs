use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, BenchmarkId, Criterion,
    Throughput,
};
use crossbeam::channel::{
    bounded,
    TryRecvError::{Disconnected, Empty},
    TrySendError::Full,
};
use disruptor::{BusySpin, LiteBlockingWaitStrategy, Producer};
use std::{
    hint::black_box,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const DATA_STRUCTURE_SIZE: usize = 128;
const BURST_SIZES: [u64; 3] = [1, 10, 100];
const PAUSES_MS: [u64; 3] = [0, 1, 10];

struct Event {
    data: i64,
}

fn pause(millis: u64) {
    if millis > 0 {
        thread::sleep(Duration::from_millis(millis));
    }
}

pub fn spsc_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc");
    for burst_size in BURST_SIZES.into_iter() {
        group.throughput(Throughput::Elements(burst_size));

        // Base: Benchmark overhead of measurement logic.
        base(&mut group, burst_size as i64);

        for pause_ms in PAUSES_MS.into_iter() {
            let inputs = (burst_size as i64, pause_ms);
            let param = format!("burst: {}, pause: {} ms", burst_size, pause_ms);

            crossbeam(&mut group, inputs, &param);
            disruptor(&mut group, inputs, &param);
            // Add a single comparable case for LiteBlocking without multiplying the whole matrix.
            if burst_size == 100 && pause_ms == 1 {
                disruptor_liteblocking(&mut group, inputs, &param);
            }
        }
    }
    group.finish();
}

// Synthetic benchmark to measure the overhead of the measurement itself.
fn base(group: &mut BenchmarkGroup<WallTime>, burst_size: i64) {
    let sink = Arc::new(AtomicI64::new(0));
    let benchmark_id = BenchmarkId::new("base", burst_size);
    group.bench_with_input(benchmark_id, &burst_size, move |b, size| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                sink.store(0, Ordering::Relaxed);
                for data in 1..=*size {
                    sink.store(black_box(data), Ordering::Relaxed);
                }
                // Wait for the last data element to "be received".
                let last_data = black_box(*size);
                while sink.load(Ordering::Relaxed) != last_data {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });
}

fn crossbeam(group: &mut BenchmarkGroup<WallTime>, inputs: (i64, u64), param: &str) {
    // Use an AtomicI64 to "extract" the value from the receiving thread.
    let sink = Arc::new(AtomicI64::new(0));
    let (s, r) = bounded::<Event>(DATA_STRUCTURE_SIZE);
    let receiver = {
        let sink = Arc::clone(&sink);
        thread::spawn(move || loop {
            match r.try_recv() {
                Ok(event) => {
                    // Only write once per burst: use a negative "token" on the last element.
                    if event.data < 0 {
                        sink.store(event.data, Ordering::Relaxed);
                    }
                }
                Err(Empty) => continue,
                Err(Disconnected) => break,
            }
        })
    };
    let benchmark_id = BenchmarkId::new("Crossbeam", &param);
    group.bench_with_input(benchmark_id, &inputs, move |b, (size, pause_ms)| {
        b.iter_custom(|iters| {
            pause(*pause_ms);
            let start = Instant::now();
            let mut token = 0_i64;
            for _ in 0..iters {
                token -= 1; // negative token is unique per iteration
                sink.store(0, Ordering::Relaxed);
                for data in 1..=*size {
                    let data = if data == *size { token } else { data };
                    let mut event = Event {
                        data: black_box(data),
                    };
                    loop {
                        match s.try_send(event) {
                            Err(Full(e)) => event = e,
                            _ => break,
                        }
                    }
                }
                // Wait for the last data element to be received in the receiver thread.
                while sink.load(Ordering::Relaxed) != token {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });
    receiver.join().expect("Should not have panicked.");
}

fn disruptor(group: &mut BenchmarkGroup<WallTime>, inputs: (i64, u64), param: &str) {
    let factory = || Event { data: 0 };
    // Use an AtomicI64 to "extract" the value from the processing thread.
    let sink = Arc::new(AtomicI64::new(0));
    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &Event, _sequence: i64, end_of_batch: bool| {
            // Only write once per burst: use a negative "token" on the last element.
            if end_of_batch && event.data < 0 {
                sink.store(event.data, Ordering::Relaxed);
            }
        }
    };
    let mut producer = disruptor::build_single_producer(DATA_STRUCTURE_SIZE, factory, BusySpin)
        .handle_events_with(processor)
        .build();
    let benchmark_id = BenchmarkId::new("disruptor", &param);
    group.bench_with_input(benchmark_id, &inputs, move |b, (size, pause_ms)| {
        b.iter_custom(|iters| {
            pause(*pause_ms);
            let start = Instant::now();
            let mut token = 0_i64;
            for _ in 0..iters {
                token -= 1; // negative token is unique per iteration
                sink.store(0, Ordering::Relaxed);
                producer.batch_publish(*size as usize, |iter| {
                    for (i, e) in iter.enumerate() {
                        let last = i + 1 == *size as usize;
                        e.data = black_box(if last { token } else { i as i64 + 1 });
                    }
                });
                // Wait for the last data element to be received inside processor.
                while sink.load(Ordering::Relaxed) != token {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });
}

fn disruptor_liteblocking(group: &mut BenchmarkGroup<WallTime>, inputs: (i64, u64), param: &str) {
    let factory = || Event { data: 0 };
    // Use an AtomicI64 to "extract" the value from the processing thread.
    let sink = Arc::new(AtomicI64::new(0));
    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &Event, _sequence: i64, end_of_batch: bool| {
            if end_of_batch && event.data < 0 {
                sink.store(event.data, Ordering::Relaxed);
            }
        }
    };
    let mut producer = disruptor::build_single_producer(
        DATA_STRUCTURE_SIZE,
        factory,
        LiteBlockingWaitStrategy::default(),
    )
    .handle_events_with(processor)
    .build();
    let benchmark_id = BenchmarkId::new("disruptor_liteblocking", &param);
    group.bench_with_input(benchmark_id, &inputs, move |b, (size, pause_ms)| {
        b.iter_custom(|iters| {
            pause(*pause_ms);
            let start = Instant::now();
            let mut token = 0_i64;
            for _ in 0..iters {
                token -= 1;
                sink.store(0, Ordering::Relaxed);
                producer.batch_publish(*size as usize, |iter| {
                    for (i, e) in iter.enumerate() {
                        let last = i + 1 == *size as usize;
                        e.data = black_box(if last { token } else { i as i64 + 1 });
                    }
                });
                // Wait for the last data element to be received inside processor.
                while sink.load(Ordering::Relaxed) != token {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });
}

criterion_group!(spsc, spsc_benchmark);
criterion_main!(spsc);
