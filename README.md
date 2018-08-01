**This crate has reached its end-of-life and is now deprecated.**

The intended successor of this crate is the
[`crossbeam-channel`](https://github.com/crossbeam-rs/crossbeam-channel).
Its API is strikingly similar, but comes with a much better `select!` macro,
better performance, a better test suite and an all-around better
implementation.

If you were previously using this crate because of `chan-signal`, then it is
simple to reproduce a similar API with `crossbeam-channel` and the
[`signal-hook`](https://github.com/vorner/signal-hook)
crate. For example, here's `chan-signal`'s `notify` function:

```rust
extern crate crossbeam_channel as channel;
extern crate signal_hook;

fn notify(signals: &[c_int]) -> Result<channel::Receiver<c_int>> {
    let (s, r) = channel::bounded(100);
    let signals = signal_hook::iterator::Signals::new(signals)?;
    thread::spawn(move || {
        for signal in signals.forever() {
            s.send(signal);
        }
    });
    Ok(r)
}
```

This crate may continue to receives bug fixes, but should otherwise be
considered dead.


chan
====

This crate provides experimental support for multi-producer/multi-consumer
channels. This includes rendezvous, synchronous and asynchronous channels.

Channels are never complete without a way to synchronize on multiple channels
at the same time. Therefore, this crate provides a `chan_select!` macro that
can select on any combination of channel send or receive operations.

[![Build status](https://api.travis-ci.org/BurntSushi/chan.png)](https://travis-ci.org/BurntSushi/chan)
[![](http://meritbadge.herokuapp.com/chan)](https://crates.io/crates/chan)

Dual-licensed under MIT or the [UNLICENSE](http://unlicense.org).


### Documentation

Please help me improve the documentation!

[http://burntsushi.net/rustdoc/chan/](http://burntsushi.net/rustdoc/chan/)


### Example: selecting on multiple channels

```rust
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
```


### Example: wait for OS signals

This requires the `chan-signal` crate on crates.io.

```rust
#[macro_use]
extern crate chan;
extern crate chan_signal;

use chan_signal::Signal;
use std::thread;

fn main() {
    // Signal gets a value when the OS sent a INT or TERM signal.
    let signal = chan_signal::notify(&[Signal::INT, Signal::TERM]);
    // When our work is complete, send a sentinel value on `sdone`.
    let (sdone, rdone) = chan::sync(0);
    // Run work.
    thread::spawn(move || run(sdone));

    // Wait for a signal or for work to be done.
    chan_select! {
        signal.recv() -> signal => {
            println!("received signal: {:?}", signal)
        },
        rdone.recv() => {
            println!("Program completed normally.");
        }
    }
}

fn run(_sdone: chan::Sender<()>) {
    println!("Running work for 5 seconds.");
    println!("Can you send a signal quickly enough?");
    // Do some work.
    thread::sleep_ms(5000);

    // _sdone gets dropped which closes the channel and causes `rdone`
    // to unblock.
}
```


### Differences with `std::sync::mpsc`

Rust's standard library has a multi-producer/single-consumer channel. Here are
the differences (which I believe amount to a massive increase in ergonomics):

* `chan` channels are multi-producer/multi-consumer, so you can clone senders
  and receivers with reckless abandon.
* `chan` uses **no `unsafe` code**.
* `chan`'s asynchronous channels have lower throughput than `std::sync::mpsc`.
  (Run the benchmarks with `cargo bench`.)
* `chan`'s synchronous channels have comparable throughput with
  `std::sync::mpsc`.
* `chan` has a select macro that works on stable and can synchronize across
  channel send *or* receive operations. It also supports a "default" case
  that is executed when all channel operations are blocked. (This effectively
  replaces uses of `try_send`/`try_recv` in `std::sync::mpsc`. Indeed, `chan`
  omits these methods entirely.)
* `chan`'s send operations will deadlock if all receivers have been dropped.
  This is a deliberate choice inspired by Go. I'm open to changing it.
* `chan`'s channels implement `Sync`. When we get a working scoped API, you
  won't need to clone channels to pass them into other threads (when you can
  prove that the channel outlives the thread of course).


### Motivation

The purpose of this crate is to provide a safe concurrent abstraction for
communicating sequential processes. Performance takes a second seat to
semantics and ergonomics. If you're looking for high performance
synchronization, `chan` is probably not the right fit.

Moreover, `chan` synchronizes across operating system threads, so in order to
spin up multiple concurrent routines, you need to launch a thread for each one.
This may be more cost than you're willing to pay.

Bottom line: `chan` doesn't have any place in a high performance web server,
but it might have a place in structuring coarse grained concurrency in
applications.


### Also...

[chan-signal](https://github.com/BurntSushi/chan-signal) is a Rust library
for using channels in `chan` to receive OS signals. (Unix only at the moment.)

[cmail](https://github.com/BurntSushi/rust-cmail) is a program to periodically
email the output of long running commands. I ported this from an
[earlier version that I wrote in Go](https://github.com/BurntSushi/cmail).
