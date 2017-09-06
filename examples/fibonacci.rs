#![deny(warnings)]
#[macro_use]
extern crate chan;

use std::thread;

use chan::{Receiver, Sender};

fn fibonacci(c: Sender<u64>, quit: Receiver<()>) {
    let (mut x, mut y) = (0, 1);
    loop {
        chan_select! {
            c.send(x) => {
                let oldx = x;
                x = y;
                y = oldx + y;
            },
            quit.recv() => {
                println!("quit");
                return;
            }
        }
    }
}

fn main() {
    let (csend, crecv) = chan::sync(0);
    let (qsend, qrecv) = chan::sync(0);
    thread::spawn(move || {
        for _ in 0..10 {
            println!("{}", crecv.recv().unwrap());
        }
        qsend.send(());
    });
    fibonacci(csend, qrecv);
}
