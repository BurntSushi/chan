#![allow(deprecated, unused_imports)]
#![cfg_attr(feature = "nightly", feature(scoped))]

extern crate chan;

use std::thread;

#[cfg(feature = "nightly")]
fn main() {
    // Channels implement `Sync`, which means any number of threads can use the
    // same channel value at the same time (without cloning/incrementing the
    // internal channel reference count).
    let (s, r) = chan::sync(2);
    thread::scoped(|| {
        s.send(5);
    }).join();
    s.send(1);
    println!("{:?}", r.recv());
    println!("{:?}", r.recv());
}

#[cfg(not(feature = "nightly"))]
fn main() {}
