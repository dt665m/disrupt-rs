use disrupt_rs::*;

#[derive(Debug)]
enum Event {
    Login {
        id: i32,
    },
    Logout {
        id: i32,
    },
    Order {
        user_id: i32,
        price: f64,
        quantity: i32,
    },
    Heartbeat,
}

fn main() {
    let factory = || Event::Heartbeat;

    let processor = |e: &Event, _sequence: Sequence, _end_of_batch: bool| match e {
        Event::Login { id } => {
            let _ = id;
        }
        Event::Logout { id } => {
            let _ = id;
        }
        Event::Order {
            user_id,
            price,
            quantity,
        } => {
            let _ = (user_id, price, quantity);
        }
        Event::Heartbeat => {}
    };

    let mut producer = disrupt_rs::build_multi_producer(64, factory, BusySpin)
        .handle_events_with(processor)
        .build();

    let mut login_producer = producer.clone();
    let mut logout_producer = producer.clone();
    let mut order_producer = producer.clone();

    login_producer.publish(|e| *e = Event::Login { id: 1 });
    order_producer.publish(|e| {
        *e = Event::Order {
            user_id: 1,
            price: 100.0,
            quantity: 10,
        }
    });
    logout_producer.publish(|e| *e = Event::Logout { id: 1 });
    producer.publish(|e| *e = Event::Heartbeat);
}
