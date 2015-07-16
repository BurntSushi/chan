#[macro_use]
extern crate chan;

use std::thread;

fn main() {
    let tick = chan::tick_ms(100);
    let boom = chan::after_ms(500);
    loop {
        chan_select! {
            default => { println!("   ."); thread::sleep_ms(50); },
            tick.recv() => println!("tick."),
            boom.recv() => { println!("BOOM!"); return; },
        }
    }
}
