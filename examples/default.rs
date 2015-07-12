#[macro_use]
extern crate chan;

use std::thread;

use chan::Receiver;

fn main() {
    let tick = tick_ms(100, ());
    let boom = after_ms(500, ());
    loop {
        chan_select! {
            default => { println!("   ."); thread::sleep_ms(50); },
            tick.recv() => println!("tick."),
            boom.recv() => { println!("BOOM!"); return; },
        }
    }
}

fn after_ms<T: Send + 'static>(duration: u32, val: T) -> Receiver<T> {
    let (send, recv) = chan::sync(0);
    thread::spawn(move || { thread::sleep_ms(duration); send.send(val) });
    recv
}

fn tick_ms<T>(duration: u32, val: T) -> Receiver<T>
where T: Clone + Send + 'static {
    let (send, recv) = chan::sync(1);
    thread::spawn(move || {
        loop {
            chan_select! {
                default => thread::sleep_ms(duration),
                send.send(val.clone()) => {},
            }
        }
    });
    recv
}
