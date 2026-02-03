use disrupt_rs::*;
use std::{cell::RefCell, rc::Rc};

struct Event {
    price: f64,
}

#[derive(Default)]
struct State {
    data: Rc<RefCell<i32>>,
}

fn main() {
    let factory = || Event { price: 0.0 };
    let initial_state = State::default;

    // Closure for processing events *with* state.
    let processor = |s: &mut State, e: &Event, sequence: Sequence, end_of_batch: bool| {
        let _ = (e.price, sequence, end_of_batch);

        // Mutate your custom state:
        *s.data.borrow_mut() += 1;
    };

    let size = 64;
    let mut producer = disrupt_rs::build_single_producer(size, factory, BusySpin)
        .handle_events_and_state_with(processor, initial_state)
        .build();

    for i in 0..10 {
        producer.publish(|e| {
            e.price = i as f64;
        });
    }
}
