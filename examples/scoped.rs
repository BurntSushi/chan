#![allow(deprecated, unused_imports)]
#![cfg_attr(feature = "nightly", feature(scoped))]

extern crate chan;

use std::thread;

#[cfg(feature = "nightly")]
fn main() {
    // Channels implement `Sync`, which means any number of threads can use the
    // same channel value at the same time (without cloning/incrementing the
    // internal channel reference count).
    //
    // This also demonstrates that one can send borrowed references across
    // channels.
    let val = 5;
    let (s, r) = chan::sync(2);
    thread::scoped(|| {
        s.send(&val);
    }).join();
    s.send(&val);
    println!("{:?}", r.recv());
    println!("{:?}", r.recv());
}

#[cfg(not(feature = "nightly"))]
fn main() {}
