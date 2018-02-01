#[macro_use]
extern crate chan;

use std::thread;
use std::time::Duration;

fn main() {
    let tick = chan::tick_ms(100);
    let boom = chan::after_ms(500);
    loop {
        chan_select! {
            default => {
                println!("   .");
                thread::sleep(Duration::from_millis(50));
            },
            tick.recv() => println!("tick."),
            boom.recv() => { println!("BOOM!"); return; },
        }
    }
}
