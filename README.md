![Crates.io](https://img.shields.io/crates/v/disruptor)
![Crates.io](https://img.shields.io/crates/d/disruptor)
![Build](https://github.com/nicholassm/disruptor-rs/actions/workflows/build_and_test.yml/badge.svg)
[![codecov](https://codecov.io/gh/nicholassm/disruptor-rs/graph/badge.svg?token=VW03K0AMI0)](https://codecov.io/gh/nicholassm/disruptor-rs)

# Disruptor

This library is a low latency, inter-thread communication library written in Rust.

It's heavily inspired by the brilliant
[Disruptor library from LMAX](https://github.com/LMAX-Exchange/disruptor).

# Contents

- [Getting Started](#getting-started)
- [Patterns](#patterns)
- [Features](#features)
- [Design Choices](#design-choices)
- [Correctness](#correctness)
- [Performance](#performance)
- [Related Work](#related-work)
- [Contributions](#contributions)
- [Roadmap](#roadmap)

# Getting Started

Add the following to your `Cargo.toml` file:

    disruptor = "3.6.1"

To read details of how to use the library, check out the documentation on [docs.rs/disruptor](https://docs.rs/disruptor).

The runnable, compiling examples live in `examples/` (compile-check: `cargo check-examples`).

## Processing Events

There are two ways to process events:

1. Supply a closure to the Disruptor and let it manage the processing thread(s).
2. Use the `EventPoller` API where you can poll for events (and manage your own threads).

Both have comparable performance so use what fits your use case best. (See also benchmarks.)

## Single and Batch Publication With Managed Threads

Here's a minimal example demonstrating both single and batch publication. Note, batch publication should be used whenever possible for best latency and throughput (see benchmarks below).

See `examples/single_and_batch_publication.rs` (run: `cargo run --example single_and_batch_publication`).

## Pinning Threads and Dependencies Between Processors

The library also supports pinning threads on cores to avoid latency induced by context switching.
A more advanced usage demonstrating this and with multiple producers and multiple interdependent consumers could look like this:

See `examples/pinning_threads_and_dependencies.rs` (run: `cargo run --example pinning_threads_and_dependencies`).

## Processors with State

If you need to store some state in the processor thread which is neither `Send` nor `Sync`, e.g. a `Rc<RefCell<i32>>`, then you can create a closure for initializing that state and pass it along with the processing closure when you build the Disruptor. The Disruptor will then pass a mutable reference to your state on each event. As an example:

See `examples/processor_with_state.rs` (run: `cargo run --example processor_with_state`).

## Event Polling

An alternative to storing state in the processor is to use the Event Poller API:

See `examples/event_poller.rs` (run: `cargo run --example event_poller`).

## Event Polling with a Dependency Gate (e.g. WAL / fsync boundary)

If you want a polled stage to act as a durability boundary (e.g. “only let downstream stages see
events that are safely on disk”), combine:

- an `EventPoller` stage (you manage the thread),
- `and_then_with_dependents(vec![gate])` for the next stage, and
- a `DependentSequence` (`gate`) that the poller advances only after the durable write succeeds.

Important nuance: dropping the `EventGuard` advances the poller’s internal cursor to the upper
available sequence (even if you didn't iterate all events). For durability semantics you should
process all events yielded by the guard before letting it drop.

See `examples/event_poller_dependency_gate.rs` (run: `cargo run --example event_poller_dependency_gate`).

# Patterns

## A Disruptor with Different Event Types

Let's assume you have multiple different types of producers that each publish distinct events.
It could be an exchange where you receive e.g. client logins, logouts, orders, etc.

You can model this by using an enum as event type:

See `examples/different_event_types.rs` (run: `cargo run --example different_event_types`).

## Splitting Workload Across Processors

Let's assume you have a high ingress rate of events and you need to split the work across multiple processors
to cope with the load. You can do that by assigning an `id` to each processor and then only process events
with a sequence number that modulo the number of processors equal the `id`.
Here, for simplicity, we split it across two processors:

See `examples/splitting_workload_across_processors.rs` (run: `cargo run --example splitting_workload_across_processors`).

This scheme ensures each event is processed once.

## Consumer Dependency Graph (e.g. WAL fsync or replication ACK)

You can gate a downstream stage on external sequences (e.g., “flushed to disk” or “replicated”) so it only
processes events that have crossed those durability boundaries.

See `examples/consumer_dependency_graph.rs` (run: `cargo run --example consumer_dependency_graph`).

You can pass multiple gates (e.g., `vec![flushed, replicated]`), and the stage will only advance to the
minimum of all dependencies. Producer backpressure still uses the slowest consumer/gate, so you remain
overwrite-safe.

# Features

- [x] Single Producer Single Consumer (SPSC).
- [x] Single Producer Multi Consumer (SPMC) with consumer interdependencies.
- [x] Multi Producer Single Consumer (MPSC).
- [x] Multi Producer Multi Consumer (MPMC) with consumer interdependencies.
- [x] Busy-spin wait strategies.
- [x] Blocking+Waking wait strategies.
- [x] Batch publication of events.
- [x] Batch consumption of events.
- [x] Event Poller API.
- [x] Thread affinity can be set for the event processor thread(s).
- [x] Set thread name of each event processor thread.
- [x] Dependent sequences (gates) on handler stages

# Design Choices

Everything in the library is about low-latency and this heavily influences all choices made in this library.
As an example, you cannot allocate an event and *move* that into the ringbuffer.
Instead, events are allocated on startup to ensure they are co-located in memory to increase cache coherency.
However, you can still allocate e.g. a struct and move ownership to a field in the event on the Ringbuffer.

There's also no use of dynamic dispatch - everything is monomorphed.

# Correctness

This library needs to use Unsafe to achieve low latency.
Although the absence of bugs cannot be guaranteed, these approaches have been used to eliminate bugs:

- Minimal usage of Unsafe blocks.
- High test coverage.
- All tests are run on Miri in CI/CD.
- Verification in TLA+ (see the `verification/` folder).

## Important Limitations / Failure Modes

- **Sequence number limit:** sequence numbers are `i64` and the library does not guard against
  publishing more than `2^63 - 1` events over the lifetime of a Disruptor instance. Exceeding this
  limit is **undefined behavior** (kept unchecked for performance).
- **Consumer panics:** if a consumer thread panics (e.g. inside your event handler), shutdown will
  panic when the library joins that thread.
- **Multi-producer clone misuse:** degenerate misuse (e.g. cloning and `mem::forget`-ing clones in a
  loop) can force the process to abort in order to avoid refcount overflow.

# Performance

The SPSC and MPSC Disruptor variants have been benchmarked and compared to Crossbeam. See the code in the `benches/spsc.rs` and `benches/mpsc.rs` files.

The results below of the SPSC benchmark are gathered from running the benchmarks on a 2016 Macbook Pro running a 2,6 GHz Quad-Core Intel Core i7. So on a modern Intel Xeon the numbers should be even better. Furthermore, it's not possible to isolate cores on Mac and pin threads which would produce even more stable results. This is future work.

If you have any suggestions to improving the benchmarks, please feel free to open an issue.

To provide a somewhat realistic benchmark not only burst of different sizes are considered but also variable pauses between bursts: 0 ms, 1 ms and 10 ms.

The latencies below are the mean latency per element with 95% confidence interval (standard `criterion` settings). Capturing all latencies and calculating misc. percentiles (and in particular the max latency) is future work. However, I expect the below measurements to be representative for the actual performance you can achieve in a real application.

## No Pause Between Bursts

*Latency:*

|  Burst Size | Crossbeam | Disruptor | Improvement |
|------------:|----------:|----------:|------------:|
|           1 |     65 ns |     32 ns |         51% |
|          10 |     68 ns |      9 ns |         87% |
|         100 |     29 ns |      8 ns |         72% |

*Throughput:*

|  Burst Size |  Crossbeam |   Disruptor | Improvement |
|------------:|-----------:|------------:|------------:|
|           1 |  15.2M / s |   31.7M / s |        109% |
|          10 |  14.5M / s |  117.3M / s |        709% |
|         100 |  34.3M / s |  119.7M / s |        249% |

## 1 ms Pause Between Bursts

*Latency:*

|  Burst Size | Crossbeam |  Disruptor | Improvement |
|------------:|----------:|-----------:|------------:|
|           1 |     63 ns |      33 ns |         48% |
|          10 |     67 ns |       8 ns |         88% |
|         100 |     30 ns |       9 ns |         70% |

*Throughput:*

|  Burst Size |  Crossbeam |  Disruptor | Improvement |
|------------:|-----------:|-----------:|------------:|
|           1 |  15.9M / s |  30.7M / s |         93% |
|          10 |  14.9M / s | 117.7M / s |        690% |
|         100 |  33.8M / s | 105.0M / s |        211% |

## 10 ms Pause Between Bursts

*Latency:*

|  Burst Size | Crossbeam | Disruptor | Improvement |
|------------:|----------:|----------:|------------:|
|           1 |     51 ns |     32 ns |         37% |
|          10 |     67 ns |      9 ns |         87% |
|         100 |     30 ns |     10 ns |         67% |

*Throughput:*

|  Burst Size | Crossbeam |  Disruptor | Improvement |
|------------:|----------:|-----------:|------------:|
|           1 | 19.5M / s |  31.6M / s |         62% |
|          10 | 14.9M / s | 114.5M / s |        668% |
|         100 | 33.6M / s | 105.0M / s |        213% |

## Conclusion

There's clearly a difference between the Disruptor and the Crossbeam libs. However, this is not because the Crossbeam library is not a great piece of software. It is. The Disruptor trades CPU and memory resources for lower latency and higher throughput and that is why it's able to achieve these results. The Disruptor also excels if you can publish batches of events as
demonstrated in the benchmarks with bursts of 10 and 100 events.

Both libraries greatly improves as the burst size goes up but the Disruptor's performance is more resilient to the pauses between bursts which is one of the design goals.

# Related Work

There are multiple other Rust projects that mimic the LMAX Disruptor library:

1. [Turbine](https://github.com/polyfractal/Turbine)
2. [Disrustor](https://github.com/sklose/disrustor)

A key feature that this library supports is multiple producers from different threads that neither of the above libraries support (at the time of writing).

# Contributions

You are welcome to create a Pull-Request or open an issue with suggestions for improvements.

Changes are accepted solely at my discretion and I will focus on whether the changes are a good fit for the purpose and design of this crate.

# Roadmap

Empty! All the items have been implemented.
