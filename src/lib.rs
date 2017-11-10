/*!
This crate provides an implementation of a multi-producer, multi-consumer
channel. Channels come in three varieties:

1. Asynchronous channels. Sends never block. Its buffer is only limited by the
   available resources on the system.
2. Synchronous buffered channels. Sends block when the buffer is full. The
   buffer is depleted by receiving on the channel.
3. Rendezvous channels (synchronous channels without a buffer). Sends block
   until a receive has consumed the value sent. When a sender and receiver
   synchronize, they are said to *rendezvous*.

Asynchronous channels are created with `chan::async()`. Synchronous channels
are created with `chan::sync(k)` where `k` is the buffer size. Rendezvous
channels are created with `chan::sync(0)`.

All channels are split into the same two types upon creation: a `Sender` and
a `Receiver`. Additional senders and receivers can be created with reckless
abandon by calling `clone`.

When all senders are dropped, the channel is closed and no other sends are
possible. In a channel with a buffer, receivers continue to consume values
until the buffer is empty, at which point, a `None` value is always returned
immediately.

No special semantics are enforced when all receivers are dropped. Asynchronous
sends will continue to work. Synchronous sends will block indefinitely when
the buffer is full. A send on a rendezvous channel will also block
indefinitely. (**NOTE**: This could be changed!)

All channels satisfy *both* `Send` and `Sync` and can be freely mixed in
`chan_select!`. Said differently, the synchronization semantics of a channel
are encoded upon construction, but are otherwise indistinguishable to the
type system.

Values sent on channels are subject to the normal restrictions Rust has on
values crossing thread boundaries. i.e., Values must implement `Send` and/or
`Sync`. (An `Rc<T>` *cannot* be sent on a channel, but a channel can be sent
on a channel!)


# Example: rendezvous channel

A simple example demonstrating a rendezvous channel:

```
use std::thread;

let (send, recv) = chan::sync(0);
thread::spawn(move || send.send(5));
assert_eq!(recv.recv(), Some(5)); // blocks until the previous send occurs
```


# Example: synchronous channel

Similarly, an example demonstrating a synchronous channel:

```
let (send, recv) = chan::sync(1);
send.send(5); // doesn't block because of the buffer
assert_eq!(recv.recv(), Some(5));
```


# Example: multiple producers and multiple consumers

An example demonstrating multiple consumers and multiple producers:

```
use std::thread;

let r = {
    let (s, r) = chan::sync(0);
    for letter in vec!['a', 'b', 'c', 'd'] {
        let s = s.clone();
        thread::spawn(move || {
            for _ in 0..10 {
                s.send(letter);
            }
        });
    }
    // This extra lexical scope will drop the initial
    // sender we created. Thus, the channel will be
    // closed when all threads spawned above has completed.
    r
};

// A wait group lets us synchronize the completion of multiple threads.
let wg = chan::WaitGroup::new();
for _ in 0..4 {
    wg.add(1);
    let wg = wg.clone();
    let r = r.clone();
    thread::spawn(move || {
        for letter in r {
            println!("Received letter: {}", letter);
        }
        wg.done();
    });
}

// If this was the end of the process and we didn't call `wg.wait()`, then
// the process might quit before all of the consumers were done.
// `wg.wait()` will block until all `wg.done()` calls have finished.
wg.wait();
```


# Example: Select on multiple channel sends/receives

An example showing how to use `chan_select!` to synchronize on sends
or receives.

```
#[macro_use]
extern crate chan;

use std::thread;

// Emits the fibonacci sequence on the given channel until `quit` receives
// a sentinel value.
fn fibonacci(s: chan::Sender<u64>, quit: chan::Receiver<()>) {
    let (mut x, mut y) = (0, 1);
    loop {
        // Select will block until at least one of `s.send` or `quit.recv`
        // is ready to succeed. At which point, it will choose exactly one
        // send/receive to synchronize.
        chan_select! {
            s.send(x) => {
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
    let (s, r) = chan::sync(0);
    let (qs, qr) = chan::sync(0);
    // Spawn a thread and ask for the first 10 numbers in the fibonacci
    // sequence.
    thread::spawn(move || {
        for _ in 0..10 {
            println!("{}", r.recv().unwrap());
        }
        // Dropping all sending channels causes the receive channel to
        // immediately and always synchronize (because the channel is closed).
        drop(qs);
    });
    fibonacci(s, qr);
}
```


# Example: non-blocking sends/receives

This crate specifically does not expose methods like `try_send` or `try_recv`.
Instead, you should prefer using `chan_select!` to perform a non-blocking
send or receive. This can be done by telling select what to do when no
synchronization events are available.

```
# #[macro_use] extern crate chan; fn main() {
let (s, _) = chan::sync(0);
chan_select! {
    default => println!("Send failed."),
    s.send("some data") => println!("Send succeeded."),
}
# }
```

When `chan_select!` first runs, it will check if `s.send(...)` can succeed
*without blocking*. If so, `chan_select!` will permit the channels to
rendezvous. However, if there is no `recv` call to accept the send, then
`chan_select!` will immediately execute the `default` arm.


# Example: the sentinel channel idiom

When writing concurrent programs with `chan`, you will often find that you need
to somehow "wait" until some operation is done. For example, let's say you want
to run a function in a separate thread, but wait until it completes. Here's
one way to do it:

```rust
use std::thread;

fn do_work(done: chan::Sender<()>) {
    // do something

    // signal that we're done.
    done.send(());
}

fn main() {
    let (sdone, rdone) = chan::sync(0);
    thread::spawn(move || do_work(sdone));
    // block until work is done, and then quit the program.
    rdone.recv();
}
```

In effect, we've created a new channel that sends unit values. When we're
done doing work, we send a unit value and `main` waits for it to be delivered.

Another way of achieving the same thing is to simply close the channel. Once
the channel is closed, any previously blocked receive operations become
immediately unblocked. What's even cooler is that channels are closed
automatically when all senders are dropped. So the new program looks something
like this:

```rust
use std::thread;

fn do_work(_done: chan::Sender<()>) {
    // do something
}

fn main() {
    let (sdone, rdone) = chan::sync(0);
    thread::spawn(move || do_work(sdone));
    // block until work is done, and then quit the program.
    rdone.recv();
}
```

We no longer need to explicitly do anything with the `_done` channel. We give
`do_work` ownership of the channel, but as soon as the function stops
executing, `_done` is dropped, the channel is closed and `rdone.recv()`
unblocks.


# Example: I want more!

There are some examples in this crate's repository:
https://github.com/BurntSushi/chan/tree/master/examples

Here is a nice example using the `chan-signal` crate to read lines from
stdin while gracefully quitting after receiving a `INT` or `TERM`
signal:
https://github.com/BurntSushi/chan-signal/blob/master/examples/read_names.rs

A non-trivial program for periodically sending email with the output of
running a command: https://github.com/BurntSushi/rust-cmail (The source is
commented more heavily than normal.)


# When are channel operations non-blocking?

Non-blocking in this context means "a send/recv operation can synchronize
immediately." (Under the hood, a mutex may still be acquired, which could
block.)

The following is a list of all cases where a channel operation is considered
non-blocking:

* A send on a synchronous channel whose buffer is not full.
* A receive on a synchronous channel with a non-empty buffer.
* A send on an asynchronous channel.
* A rendezvous send or recv when a corresponding recv or send operation is
already blocked, respectively.
* A receive on any closed channel.

Non-blocking semantics are important because they affect the behavior of
`chan_select!`. In particular, a `chan_select!` with a `default` arm will
execute the `default` case if and only if all other operations are blocked.


# Which channel type should I use?

[From Ken Kahn](http://www.eros-os.org/pipermail/e-lang/2003-January/008183.html):

> About 25 years ago I went to dinner with Carl Hewitt and Robin Milner (of
> CSS and pi calculus fame) and they were arguing about synchronous vs.
> asynchronous communication primitives. Carl used the post office metaphor
> while Robin used the telephone. Both quickly admitted that one can implement
> one in the other.

With three channel types to choose from, it may not always be clear which one
you should use. In fact, there has been a long debate over which are better.
Here are some rough guidelines:

* Historically, asynchronous channels have been associated with the actor
model, which means they're a little out of place in a library inspired by
communicating sequential processes. Nevertheless, an unconstrained buffer can
be occasionally useful.
* Synchronous channels are useful because their stricter synchronization
semantics can make it easier to reason about the flow of your program. In
particular, with a rendezvous channel, one knows that a `send` unblocks only
when a corresponding `recv` consumes the sent value. This makes it *feel*
an awful lot like a function call!


# Warning: leaks

Channels can be leaked! In particular, if all receivers have been dropped,
then any future sends will block. Usually this is indicative of a bug in your
program.

For example, consider a "generator" style pattern where a thread produces
values on a channel and another thread consumes in an iterator.

```no_run
use std::thread;

let (s, r) = chan::sync(0);

thread::spawn(move || {
    for val in r {
        if val >= 2 {
            break;
        }
    }
});

s.send(1);
s.send(2);
// This will deadlock because the loop in the thread
// above quits after receiving `2`.
s.send(3);
```

If the iterator loop quits early, the channel's buffer could fill up, which
will indefinitely block all future send operations.

(These leaks/deadlocks are detectable in most circumstances, and a `send`
operation could be made to wake up and either return an error or panic. The
semantics here are still experimental.)


# Warning: more leaks

It will always be possible to leak a channel in safe code regardless of the
channel's semantics. For example:

```no_run
use std::mem::forget;

let (s, r) = chan::sync::<()>(0);
forget(s);
// Blocks forever because the channel is never closed.
r.recv();
```

In this case, it is impossible for the channel to close because the internal
reference count will never reach `0`.


# Warning: performance

The primary purpose of this crate is to provide a safe, concurrent abstraction.
Notably, it is *not* a zero-cost abstraction. It is not even a near-zero-cost
abstraction. Throughput on a channel is startlingly low (see the benchmarks
in this crate's repository). Therefore, the channels provided in this crate
are most useful as a means to structure concurrent programs at a coarse level.

If your requirements call for performant synchronization of data, `chan` is not
the crate you're looking for.


# Prior art

The semantics encoded in the channels provided by this crate should mirror or
closely mirror the semantics provided by channels in Go. This includes
select statements! The major difference between concurrent programs written
with `chan` and concurrent programs written with Go is that Go programs can
benefit from being fast and loose with creating goroutines. In `chan`, each
"goroutine" is just an OS thread.

In terms of writing code:

1. Go programs will feature explicit closing of channels. In `chan`, channels
   are closed **only** when all senders have been dropped.
2. Since there is no such thing as a "nil" channel in `chan`, the semantics Go
   has for nil channels (both sends and receives block indefinitely) do not
   exist in `chan`.
3. `chan` does not expose `len` or `cap` methods. (For no reason other than
   to start with a totally minimal API. In particular, calling `len` or `cap`
   on a channel is often The Wrong Thing. But not always. So this restriction
   may be lifted in the future.)
4. In `chan`, all channels are either senders or receivers. There is no
   "bidirectional" channel. This is manifest in how channel memory is managed:
   channels are closed when all senders are dropped.

Of course, Go is not the origin of these ideas, but it has been the
strongest influence on the design of this library, and at least one of its
authors has done substantial research on the integration of CSP and programming
languages.
*/

#![deny(missing_docs)]

extern crate rand;

use std::collections::VecDeque;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Drop;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use notifier::Notifier;
pub use select::{Select, SelectRecvHandle, SelectSendHandle};
use tracker::Tracker;
pub use wait_group::WaitGroup;

// This enables us to (in practice) uniquely identify any particular channel.
// A better approach would be to use the pointer's address in memory, but it
// looks like `Arc` doesn't support that (yet?).
//
// Any other ideas? ---AG
//
// N.B. This is combined with ChannelId to distinguish between the sending
// and receiving halves of a channel.
static NEXT_CHANNEL_ID: AtomicUsize = ATOMIC_USIZE_INIT;

mod notifier;
mod select;
mod tracker;
mod wait_group;

/// Create a synchronous channel with a possibly empty buffer.
///
/// When the `size` is zero, the buffer is empty and the channel becomes a
/// rendezvous channel. A rendezvous channel blocks send operations until
/// a corresponding receive operation consumes the sent value.
///
/// When the `size` is non-zero, the send operations will only block when the
/// buffer is full. Send operations only unblock when a receive operation
/// removes an element from the buffer.
///
/// Values are guaranteed to be received in the same order that they are sent.
///
/// The send and receive values returned can be cloned arbitrarily (i.e.,
/// multi-producer/multi-consumer) and moved to other threads.
///
/// When all senders are dropped, the channel is closed automatically. No
/// more values may be sent on a closed channel. Once a channel is closed and
/// the buffer is empty, all receive operations return `None` immediately.
/// (If a channel is closed and there are still values in the buffer, then
/// receive operations will retrieve those first.)
///
/// When all receivers are dropped, no special action is taken. When the buffer
/// is full, all subsequent send operations will block indefinitely.
///
/// # Examples
///
/// An example of a rendezvous channel:
///
/// ```
/// use std::thread;
///
/// let (send, recv) = chan::sync(0);
/// thread::spawn(move || send.send(5));
/// assert_eq!(recv.recv(), Some(5)); // blocks until the previous send occurs
/// ```
///
/// An example of a synchronous buffered channel:
///
/// ```
/// let (send, recv) = chan::sync(1);
///
/// send.send(5); // doesn't block because of the buffer
/// assert_eq!(recv.recv(), Some(5));
///
/// drop(send); // closes the channel
/// assert_eq!(recv.recv(), None);
/// ```
pub fn sync<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    let send = Channel::new(size, false);
    let recv = send.clone();
    (send.into_sender(), recv.into_receiver())
}

/// Create an asynchronous channel with an unbounded buffer.
///
/// Since the buffer is unbounded, send operations always succeed immediately.
///
/// Receive operations succeed only when there is at least one value in the
/// buffer.
///
/// Values are guaranteed to be received in the same order that they are sent.
///
/// The send and receive values returned can be cloned arbitrarily (i.e.,
/// multi-producer/multi-consumer) and moved to other threads.
///
/// When all senders are dropped, the channel is closed automatically. No
/// more values may be sent on a closed channel. Once a channel is closed and
/// the buffer is empty, all receive operations return `None` immediately.
/// (If a channel is closed and there are still values in the buffer, then
/// receive operations will retrieve those first.)
///
/// When all receivers are dropped, no special action is taken. When the buffer
/// is full, all subsequent send operations will block indefinitely.
///
/// # Example
///
/// Asynchronous channels are nice when you just want to enqueue a bunch
/// of values up front:
///
/// ```
/// let (s, r) = chan::async();
///
/// for i in 0..10 {
///     s.send(i);
/// }
///
/// drop(s); // closing the channel lets the iterator stop
/// let numbers: Vec<i32> = r.iter().collect();
/// assert_eq!(numbers, (0..10).collect::<Vec<i32>>());
/// ```
///
/// (Others should help me come up with more compelling examples of
/// asynchronous channels.)
pub fn async<T>() -> (Sender<T>, Receiver<T>) {
    let send = Channel::new(0, true);
    let recv = send.clone();
    (send.into_sender(), recv.into_receiver())
}

/// Creates a new rendezvous channel that is dropped after a timeout.
///
/// When the channel is dropped, any receive operation on the returned channel
/// will be unblocked.
///
/// # Example
///
/// ```
/// let wait = chan::after_ms(1000);
/// // Unblocks after 1 second.
/// wait.recv();
/// ```
pub fn after(duration: Duration) -> Receiver<()> {
    let (send, recv) = sync(0);
    thread::spawn(move || {
        thread::sleep(duration);
        drop(send);
    });
    recv
}

/// Creates a new rendezvous channel that is dropped after a timeout.
///
/// `duration` is specified in milliseconds.
///
/// When the channel is dropped, any receive operation on the returned channel
/// will be unblocked.
///
/// N.B. This will eventually be deprecated when we get a proper duration type.
///
/// # Example
///
/// ```
/// let wait = chan::after_ms(1000);
/// // Unblocks after 1 second.
/// wait.recv();
/// ```
pub fn after_ms(duration: u32) -> Receiver<()> {
    #![allow(deprecated)]
    let (send, recv) = sync(0);
    thread::spawn(move || {
        thread::sleep_ms(duration);
        drop(send);
    });
    recv
}

/// Creates a new rendezvous channel that is "ticked" every duration.
///
/// When `duration` is `0`, no ticks are ever sent.
///
/// When `duration` is non-zero, then a new channel is created and sent at
/// every duration. When the sent channel is dropped, the timer is reset
/// and the process repeats after the duration.
///
/// This is especially convenient because it keeps the ticking in sync with
/// the code that uses it. Namely, the ticks won't "build up."
///
/// N.B. There is no way to reclaim the resources used by this function.
/// If you stop receiving on the channel returned, then the thread spawned by
/// `tick_ms` will block indefinitely.
///
/// # Examples
///
/// This is most useful when used in `chan_select!` because the received
/// sentinel channel gets dropped only after the correspond arm has
/// executed. At which point, the ticker is reset and waits to tick until
/// `duration` milliseconds lapses *after* the `chan_select!` arm is executed.
///
/// ```
/// # #[macro_use] extern crate chan; fn main() {
/// use std::thread;
/// use std::time::Duration;
///
/// let tick = chan::tick(Duration::from_millis(100));
/// let boom = chan::after(Duration::from_millis(500));
/// loop {
///     chan_select! {
///         default => {
///             println!("   .");
///             thread::sleep(Duration::from_millis(50));
///         },
///         tick.recv() => println!("tick."),
///         boom.recv() => { println!("BOOM!"); return; },
///     }
/// }
/// # }
/// ```
pub fn tick(duration: Duration) -> Receiver<Sender<()>> {
    let (send, recv) = sync(0);
    if duration.as_secs() == 0 && duration.subsec_nanos() == 0 {
        // Leak the send channel so that it never gets closed and
        // `recv` never synchronizes.
        ::std::mem::forget(send);
    } else {
        thread::spawn(move || {
            loop {
                thread::sleep(duration);
                let (sdone, rdone) = sync(0);
                send.send(sdone);
                // Block until `sdone` gets closed by the caller.
                rdone.recv();
            }
        });
    }
    recv
}

/// Creates a new rendezvous channel that is "ticked" every duration.
///
/// `duration` is specified in milliseconds.
///
/// When `duration` is `0`, no ticks are ever sent.
///
/// When `duration` is non-zero, then a new channel is created and sent at
/// every duration. When the sent channel is dropped, the timer is reset
/// and the process repeats after the duration.
///
/// This is especially convenient because it keeps the ticking in sync with
/// the code that uses it. Namely, the ticks won't "build up."
///
/// N.B. There is no way to reclaim the resources used by this function.
/// If you stop receiving on the channel returned, then the thread spawned by
/// `tick_ms` will block indefinitely.
///
/// # Examples
///
/// This is most useful when used in `chan_select!` because the received
/// sentinel channel gets dropped only after the correspond arm has
/// executed. At which point, the ticker is reset and waits to tick until
/// `duration` milliseconds lapses *after* the `chan_select!` arm is executed.
///
/// ```
/// # #[macro_use] extern crate chan; fn main() {
/// use std::thread;
/// use std::time::Duration;
///
/// let tick = chan::tick_ms(100);
/// let boom = chan::after_ms(500);
/// loop {
///     chan_select! {
///         default => {
///             println!("   .");
///             thread::sleep(Duration::from_millis(50));
///         },
///         tick.recv() => println!("tick."),
///         boom.recv() => { println!("BOOM!"); return; },
///     }
/// }
/// # }
/// ```
pub fn tick_ms(duration: u32) -> Receiver<Sender<()>> {
    #![allow(deprecated)]
    let (send, recv) = sync(0);
    if duration == 0 {
        // Leak the send channel so that it never gets closed and
        // `recv` never synchronizes.
        ::std::mem::forget(send);
    } else {
        thread::spawn(move || {
            loop {
                thread::sleep_ms(duration);
                let (sdone, rdone) = sync(0);
                send.send(sdone);
                // Block until `sdone` gets closed by the caller.
                rdone.recv();
            }
        });
    }
    recv
}

/// A value that uniquely identifies one half of a channel.
///
/// For any `s: Sender<T>`, `s.id() == s.clone().id()`. Similarly for
/// any `r: Receiver<T>`.
#[doc(hidden)]
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChannelId(ChannelKey);

#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ChannelKey {
    Sender(u64),
    Receiver(u64),
}

impl ChannelId {
    fn sender(id: u64) -> ChannelId {
        ChannelId(ChannelKey::Sender(id))
    }

    fn receiver(id: u64) -> ChannelId {
        ChannelId(ChannelKey::Receiver(id))
    }
}

/// An iterator over values received in a channel.
pub struct Iter<T> {
    chan: Receiver<T>,
}

impl<T> Iterator for Iter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> { self.chan.recv() }
}

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = Iter<T>;
    fn into_iter(self) -> Iter<T> { Iter { chan: self } }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<T>;
    fn into_iter(self) -> Iter<T> { self.iter() }
}

/// The sending half of a channel.
///
/// Senders can be cloned any number of times and sent to other threads.
///
/// Senders also implement `Sync`, which means they can be shared among threads
/// without cloning if the channels can be proven to outlive the execution
/// of the threads.
///
/// When all sending halves of a channel are dropped, the channel is closed
/// automatically. When a channel is closed, no new values can be sent on the
/// channel. Also, all receive operations either return any values left in the
/// buffer or return immediately with `None`.
#[derive(Debug)]
pub struct Sender<T>(Channel<T>);

/// The receiving half of a channel.
///
/// Receivers can be cloned any number of times and sent to other threads.
///
/// Receivers also implement `Sync`, which means they can be shared among
/// threads without cloning if the channels can be proven to outlive the
/// execution of the threads.
///
/// When all receiving halves of a channel are dropped, no special action is
/// taken. If the buffer in the channel is full, all sends will block
/// indefinitely.
#[derive(Debug)]
pub struct Receiver<T>(Channel<T>);

/// All senders and receivers are just newtypes around a more base channel.
///
/// i.e., All senders and receivers have direct access to any underlying
/// buffer.
#[derive(Debug)]
struct Channel<T>(Arc<Inner<T>>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChannelType {
    Async,
    Rendezvous,
    Buffered,
}

struct Inner<T> {
    /// An auto-incrementing id.
    id: u64,
    /// Manages subscriptions to channels (e.g., from a `chan_select!`).
    notify: Notifier,
    /// Tracks reference counts of senders and receivers.
    track: Tracker,
    /// A condition variable on the contents of `data`.
    cond: Condvar,
    /// The capacity of a synchronous buffer. This corresponds to the number
    /// of elements allowed in the buffer before send operations block.
    ///
    /// For asynchronous and rendezvous channels, this is always 0.
    cap: usize,
    /// The type of the channel.
    ty: ChannelType,
    /// Synchronized data in the channel. e.g., the queued values.
    data: Mutex<Data<T>>,
}

#[derive(Debug)]
struct Data<T> {
    /// Whether the channel is closed or not. Once set to `true` it can never
    /// be changed.
    closed: bool,
    /// The number of senders waiting. (Currently only used in rendezvous
    /// channels.)
    waiting_send: usize,
    /// The number of receivers waiting. (Currently only used in rendezvous
    /// channels.)
    waiting_recv: usize,
    /// The actual data stored by the user.
    user: UserData<T>,
}

#[derive(Debug)]
enum UserData<T> {
    /// Used for rendezvous channels. We only need to ever store one value.
    One(Option<T>),
    /// A ring buffer for synchronous channels.
    /// There's definitely a more efficient representation, but I don't think
    /// we really care.
    Ring { queue: Vec<Option<T>>, pos: usize, len: usize },
    /// An unbounded queue for asynchronous channels.
    Queue(VecDeque<T>),
}

// The SendOp and RecvOp types unify the return values of all channel
// operations. Their primary purpose is to permit the caller to retrieve the
// channel's lock after the channel operation is done without the lock ever
// being released. (This is critical functionality for `Select`.)
//
// N.B. The `WouldBlock` variants are only constructed if a non-blocking
// operation is used (i.e., try_send or try_recv).

struct SendOp<'a, T: 'a> {
    lock: MutexGuard<'a, Data<T>>,
    kind: SendOpKind<T>,
}

#[derive(Debug)]
enum SendOpKind<T> {
    Ok,
    Closed(T),
    WouldBlock(T),
}

struct RecvOp<'a, T: 'a> {
    lock: MutexGuard<'a, Data<T>>,
    kind: RecvOpKind<T>,
}

#[derive(Debug)]
enum RecvOpKind<T> {
    Ok(T),
    Closed,
    WouldBlock,
}

impl<T> Sender<T> {
    /// Send a value on this channel.
    ///
    /// If this is an asnychronous channel, `send` never blocks.
    ///
    /// If this is a synchronous channel, `send` only blocks when the buffer
    /// is full.
    ///
    /// If this is a rendezvous channel, `send` blocks until a corresponding
    /// `recv` retrieves `val`.
    ///
    /// Values are guaranteed to be received in the same order that they
    /// are sent.
    ///
    /// This operation will never `panic!` but it can deadlock.
    pub fn send(&self, val: T) {
        self.send_op(self.inner().lock(), false, val).unwrap()
    }

    fn try_send(&self, val: T) -> Result<(), T> {
        self.send_op(self.inner().lock(), true, val).into_result()
    }

    fn send_op<'a>(
        &'a self,
        data: MutexGuard<'a, Data<T>>,
        try: bool,
        val: T,
    ) -> SendOp<'a, T> {
        match self.inner().ty {
            ChannelType::Async => self.inner().async_send(data, val),
            ChannelType::Rendezvous => {
                self.inner().rendezvous_send(data, try, val)
            }
            ChannelType::Buffered => {
                self.inner().buffered_send(data, try, val)
            }
        }
    }

    fn inner(&self) -> &Inner<T> {
        &(self.0).0
    }

    fn id(&self) -> ChannelId {
        ChannelId::sender(self.inner().id)
    }
}

impl<T> Receiver<T> {
    /// Receive a value on this channel.
    ///
    /// If this is an asnychronous channel, `recv` only blocks when the
    /// buffer is empty.
    ///
    /// If this is a synchronous channel, `recv` only blocks when the buffer
    /// is empty.
    ///
    /// If this is a rendezvous channel, `recv` blocks until a corresponding
    /// `send` sends a value.
    ///
    /// For all channels, if the channel is closed and the buffer is empty,
    /// then `recv` always and immediately returns `None`. (If the buffer is
    /// non-empty on a closed channel, then values from the buffer are
    /// returned.)
    ///
    /// Values are guaranteed to be received in the same order that they
    /// are sent.
    ///
    /// This operation will never `panic!` but it can deadlock if the channel
    /// is never closed.
    pub fn recv(&self) -> Option<T> {
        self.recv_op(self.inner().lock(), false).unwrap()
    }

    fn try_recv(&self) -> Result<Option<T>, ()> {
        self.recv_op(self.inner().lock(), true).into_result()
    }

    fn recv_op<'a>(
        &'a self,
        data: MutexGuard<'a, Data<T>>,
        try: bool,
    ) -> RecvOp<'a, T> {
        self.inner().recv(data, try)
    }

    /// Return an iterator for receiving values on this channel.
    ///
    /// This iterator yields values (blocking if necessary) until the channel
    /// is closed.
    pub fn iter(&self) -> Iter<T> { Iter { chan: self.clone() } }

    fn inner(&self) -> &Inner<T> {
        &(self.0).0
    }

    fn id(&self) -> ChannelId {
        ChannelId::receiver(self.inner().id)
    }
}

impl<T> Channel<T> {
    fn new(size: usize, async: bool) -> Channel<T> {
        let (user, ty) = if async {
            (
                UserData::Queue(VecDeque::with_capacity(1024)),
                ChannelType::Async,
            )
        } else if size == 0 {
            (UserData::One(None), ChannelType::Rendezvous)
        } else {
            let mut queue = Vec::with_capacity(size);
            for _ in 0..size { queue.push(None); }
            (
                UserData::Ring { queue: queue, pos: 0, len: 0 },
                ChannelType::Buffered,
            )
        };
        Channel(Arc::new(Inner {
            id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst) as u64,
            notify: Notifier::new(),
            track: Tracker::new(),
            cond: Condvar::new(),
            cap: size,
            ty: ty,
            data: Mutex::new(Data {
                closed: false,
                waiting_send: 0,
                waiting_recv: 0,
                user: user,
            }),
        }))
    }

    fn into_sender(self) -> Sender<T> {
        self.0.track.add_sender();
        Sender(self)
    }

    fn into_receiver(self) -> Receiver<T> {
        self.0.track.add_receiver();
        Receiver(self)
    }
}

impl<T> Inner<T> {
    fn lock(&self) -> MutexGuard<Data<T>> {
        self.data.lock().unwrap()
    }

    fn close(&self) {
        let mut data = self.lock();
        data.closed = true;
        self.notify();
    }

    fn notify(&self) {
        self.cond.notify_all();
        self.notify.notify();
    }

    // The following are all of the core channel operations wrapped up in a
    // pretty bow. They all follow a reasonably similar pattern (with the
    // rendezvous `send` diverging the most) which is roughly:
    //
    // 1. Accept locked access to the channel's data.
    // 2. Check if the operation can continue. (For sends, we block if the
    //    buffer is full. For receives, we block if the buffer is empty.)
    // 2a. If we need to block, then we release the mutex given to us and
    //     block on a condition variable.
    // 2b. When awoken, go to (2).
    // 3. If we don't need to block, then we are guaranteed to synchronize*
    //    either by adding a value to the buffer or removing a value.
    // 4. Wake all other senders and receivers that are blocked on the
    //    same condition variable mentioned in (2a).
    //
    // * Not true for sending on rendezvous channels, since we need to make
    // sure that a recv consumes the value.
    //
    // Interestingly, the recv operation for all three types of channels
    // is exactly the same (modulo the underlying data structure).

    fn recv<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
    ) -> RecvOp<'a, T> {
        while data.user.len() == 0 {
            if data.closed {
                return RecvOp::closed(data);
            }
            if try {
                return RecvOp::blocked(data);
            }
            if self.ty == ChannelType::Rendezvous {
                self.notify();
            }
            data.waiting_recv += 1;
            data = self.cond.wait(data).unwrap();
            data.waiting_recv -= 1;
        }
        let val = data.user.pop();
        self.notify();
        RecvOp::ok(data, val)
    }

    // The asynchronous send is the easiest. Just push the data and notify.
    fn async_send<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        val: T,
    ) -> SendOp<'a, T> {
        data.user.push(val);
        self.notify();
        SendOp::ok(data)
    }

    // Buffered send is pretty much the dual of recv.
    fn buffered_send<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
        val: T,
    ) -> SendOp<'a, T> {
        while data.user.len() == self.cap {
            if data.closed {
                return SendOp::closed(data, val);
            }
            if try {
                return SendOp::blocked(data, val);
            }
            data = self.cond.wait(data).unwrap();
        }
        if data.closed {
            return SendOp::closed(data, val);
        }
        data.user.push(val);
        self.notify();
        SendOp::ok(data)
    }

    // Rendezvous send is the trickiest because we need to:
    //
    //  1) Make sure no other senders interfere. We do this by ensuring
    //     that there are no other waiting senders before trying to
    //     synchronize with a receiver.
    //  2) Wait for a receiver to consume the sent value. We do this by
    //     waiting on the condition variable until the value we put
    //     in the buffer is gone.
    fn rendezvous_send<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
        val: T,
    ) -> SendOp<'a, T> {
        while data.waiting_send == 1 || data.user.len() == 1 {
            if try {
                return SendOp::blocked(data, val);
            }
            data = self.cond.wait(data).unwrap();
        }
        // invariant: at most one sender can be here.
        if data.closed {
            return SendOp::closed(data, val);
        }
        if try && data.waiting_recv == 0 {
            return SendOp::blocked(data, val);
        }
        data.user.push(val);
        // We need to wake up any blocked receivers so they get a chance to
        // grab the value we pushed.
        self.notify();
        while data.user.len() == 1 {
            data.waiting_send += 1;
            data = self.cond.wait(data).unwrap();
            data.waiting_send -= 1;
        }
        // And now we need to make sure we wake up any previous blocked
        // senders so they get a shot at synchronizing.
        self.notify();
        SendOp::ok(data)
    }
}

impl<T> UserData<T> {
    fn push(&mut self, val: T) {
        match *self {
            UserData::One(ref mut val_loc) => *val_loc = Some(val),
            UserData::Ring { ref mut queue, pos, ref mut len } => {
                let cap = queue.len();
                assert!(*len < cap);
                queue[(pos + *len) % cap] = Some(val);
                *len += 1;
            }
            UserData::Queue(ref mut deque) => deque.push_back(val),
        }
    }

    fn pop(&mut self) -> T {
        match *self {
            UserData::One(ref mut val) => val.take().unwrap(),
            UserData::Ring { ref mut queue, ref mut pos, ref mut len } => {
                let cap = queue.len();
                assert!(*len <= cap);
                assert!(*len > 0);
                let val = queue[*pos].take().expect("non-null item in queue");
                *pos = (*pos + 1) % cap;
                *len -= 1;
                val
            }
            UserData::Queue(ref mut deque) => deque.pop_front().unwrap(),
        }
    }

    fn len(&self) -> usize {
        match *self {
            UserData::One(ref val) => if val.is_some() { 1 } else { 0 },
            UserData::Ring { len, .. } => len,
            UserData::Queue(ref deque) => deque.len(),
        }
    }
}

impl<'a, T> SendOp<'a, T> {
    fn ok(lock: MutexGuard<'a, Data<T>>) -> SendOp<'a, T> {
        SendOp { lock: lock, kind: SendOpKind::Ok }
    }

    fn closed(lock: MutexGuard<'a, Data<T>>, val: T) -> SendOp<'a, T> {
        SendOp { lock: lock, kind: SendOpKind::Closed(val) }
    }

    fn blocked(lock: MutexGuard<'a, Data<T>>, val: T) -> SendOp<'a, T> {
        SendOp { lock: lock, kind: SendOpKind::WouldBlock(val) }
    }

    fn unwrap(self) {
        self.into_result().ok().unwrap();
    }

    fn into_result(self) -> Result<(), T> {
        self.into_result_lock().1
    }

    fn into_result_lock(self) -> (MutexGuard<'a, Data<T>>, Result<(), T>) {
        match self.kind {
            SendOpKind::Ok => (self.lock, Ok(())),
            SendOpKind::WouldBlock(val) => (self.lock, Err(val)),
            SendOpKind::Closed(_) => {
                // If we're here, then there was a `send` on a closed
                // channel. But the only way a channel gets closed is if
                // all values that can `send` have been dropped.
                //
                // Unless there's a way to cause a destructor to run while
                // still retaining a valid reference to the sender, this should
                // be impossible (or there's a bug in my code).
                drop(self.lock); // avoid poisoning?
                panic!("cannot send on a closed channel");
            }
        }
    }
}

impl<'a, T> RecvOp<'a, T> {
    fn ok(lock: MutexGuard<'a, Data<T>>, val: T) -> RecvOp<'a, T> {
        RecvOp { lock: lock, kind: RecvOpKind::Ok(val) }
    }

    fn closed(lock: MutexGuard<'a, Data<T>>) -> RecvOp<'a, T> {
        RecvOp { lock: lock, kind: RecvOpKind::Closed }
    }

    fn blocked(lock: MutexGuard<'a, Data<T>>) -> RecvOp<'a, T> {
        RecvOp { lock: lock, kind: RecvOpKind::WouldBlock }
    }

    fn unwrap(self) -> Option<T> {
        self.into_result().ok().unwrap()
    }

    fn into_result(self) -> Result<Option<T>, ()> {
        self.into_result_lock().1
    }

    fn into_result_lock(self)
                       -> (MutexGuard<'a, Data<T>>, Result<Option<T>, ()>) {
        (self.lock, match self.kind {
            RecvOpKind::Ok(val) => Ok(Some(val)),
            RecvOpKind::WouldBlock => Err(()),
            RecvOpKind::Closed => Ok(None),
        })
    }
}

impl<T> Clone for Channel<T> {
    fn clone(&self) -> Channel<T> {
        Channel(self.0.clone())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        self.0.clone().into_sender()
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Receiver<T> {
        self.0.clone().into_receiver()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.inner().track.remove_sender(|| self.inner().close());
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.inner().track.remove_receiver(|| ());
    }
}

impl<T> Hash for Sender<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

impl<T> Hash for Receiver<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

impl<T> PartialEq for Sender<T> {
    fn eq(&self, other: &Sender<T>) -> bool {
        self.id() == other.id()
    }
}


impl<T> PartialEq for Receiver<T> {
    fn eq(&self, other: &Receiver<T>) -> bool {
        self.id() == other.id()
    }
}

impl<T> Eq for Sender<T> {}
impl<T> Eq for Receiver<T> {}

impl<T: fmt::Debug> fmt::Debug for Inner<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data = self.data.lock().unwrap();
        try!(writeln!(f, "SyncInner {{"));
        try!(writeln!(f, "    id: {:?},", self.id));
        try!(writeln!(f, "    cap: {:?},", self.cap));
        try!(writeln!(f, "    notify: {:?},", self.notify));
        try!(writeln!(f, "    data: {:?},", &*data));
        write!(f, "}}")
    }
}

/// Synchronize on at most one channel send or receive operation.
///
/// This is a *heterogeneous* select. Namely, it supports any mix of
/// asynchronous, synchronous or rendezvous channels, any mix of send or
/// receive operations and any mix of types on channels.
///
/// Here is how select operates:
///
/// 1. It first examines all send and receive operations. If one or more of
/// them can succeed without blocking, then it randomly selects *one*,
/// executes the operation and runs the code in the corresponding arm.
/// 2. If all operations are blocked and there is a `default` arm, then the
/// code in the `default` arm is executed.
/// 3. If all operations are blocked and there is no `default` arm, then
/// `Select` will subscribe to all channels involved. `Select` will be
/// notified when state in one of the channels has changed. This will wake
/// `Select` up, and it will retry the steps in (1). If all operations remain
/// blocked, then (3) is repeated.
///
///
/// # Example
///
/// Which one synchronizes first?
///
/// ```
/// # #[macro_use] extern crate chan; fn main() {
/// use std::thread;
///
/// let (asend, arecv) = chan::sync(0);
/// let (bsend, brecv) = chan::sync(0);
///
/// thread::spawn(move || asend.send(5));
/// thread::spawn(move || brecv.recv());
///
/// chan_select! {
///     arecv.recv() -> val => {
///         println!("arecv received: {:?}", val);
///     },
///     bsend.send(10) => {
///         println!("bsend sent");
///     },
/// }
/// # }
/// ```
///
/// See the "failure modes" section below for more examples of the syntax.
///
///
/// # Example: empty select
///
/// An empty select, `chan_select! {}` will block indefinitely.
///
///
/// # Warning
///
/// `chan_select!` is simultaneously the most wonderful and horrifying thing
/// in this crate.
///
/// It is wonderful because it is essential for the
/// composition of channel operations in a concurrent program. Without select,
/// channels becomes much less expressive.
///
/// It is horrifying because the macro used to define it is *extremely*
/// sensitive. My hope is that it is simply my own lack of creativity at fault
/// and that others can help me fix it, but we may just be fundamentally stuck
/// with something like this until a proper compiler plugin can rescue us.
///
///
/// # Failure modes
///
/// When I say that this macro is sensitive, what I mean is, "if you misstep
/// on the syntax, you will be slapped upside the head with an irrelevant
/// error message."
///
/// Consider this:
///
/// ```ignore
/// chan_select! {
///     default => {
///         println!("   .");
///         thread::sleep(Duration::from_millis(50));
///     }
///     tick.recv() => println!("tick."),
///     boom.recv() => { println!("BOOM!"); return; },
/// }
/// ```
///
/// The compiler will tell you that the "recursion limit reached while
/// expanding the macro."
///
/// The actual problem is that **every** arm requires a trailing comma,
/// regardless of whether the arm is wrapped in a `{ ... }` or not. So it
/// should be written `default => { ... },`. (I'm told that various highly
/// skilled individuals could remove this restriction.)
///
/// Here's another. Can you spot the problem? I swear it's not commas this
/// time.
///
/// ```ignore
/// chan_select! {
///     tick.recv() => println!("tick."),
///     boom.recv() => { println!("BOOM!"); return; },
///     default => {
///         println!("   .");
///         thread::sleep(Duration::from_millis(50));
///     },
/// }
/// ```
///
/// This produces the same "recursion limit" error as above.
///
/// The actual problem is that the `default` arm *must* come first (or it must
/// be omitted completely).
///
/// Yet another:
///
/// ```ignore
/// chan_select! {
///     default => {
///         println!("   .");
///         thread::sleep(Duration::from_millis(50));
///     },
///     tick().recv() => println!("tick."),
///     boom.recv() => { println!("BOOM!"); return; },
/// }
/// ```
///
/// Again, you'll get the same "recursion limit" error.
///
/// The actual problem is that the channel operations must be of the form
/// `ident.recv()` or `ident.send()`. You cannot use an arbitrary expression
/// in place of `ident` that evaluates to a channel! To fix this, you must
/// rebind `tick()` to an identifier outside of `chan_select!`.
#[macro_export]
macro_rules! chan_select {
    ($select:ident, default => $default:expr, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr,
    )+) => {
        chan_select!(
            $select,
            default => $default,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };
    ($select:ident, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr,
    )+) => {
        chan_select!(
            $select,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };
    ($select:ident, default => $default:expr, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+) => {{
        let sel = &mut $select;
        $(let $chan = sel.$meth(&$chan $(, $send)*);)+
        let which = sel.try_select();
        $(if which == Some($chan.id()) {
            $(let $name = $chan.into_value();)*
            $code
        } else)+
        { $default }
    }};
    ($select:ident, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+) => {{
        let sel = &mut $select;
        $(let $chan = sel.$meth(&$chan $(, $send)*);)+
        let which = sel.select();
        $(if which == $chan.id() {
            $(let $name = $chan.into_value();)*
            $code
        } else)+
        { unreachable!() }
    }};
    (default => $default:expr) => {{ $default }};
    (default => $default:expr,) => {{ $default }};
    ($select:ident, default => $default:expr) => {{ $default }};
    ($select:ident, default => $default:expr,) => {{ $default }};
    ($select:ident) => {{
        let mut sel = &mut $select;
        sel.select(); // blocks forever
    }};
    () => {{
        let mut sel = $crate::Select::new();
        chan_select!(sel);
    }};
    ($(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+$(,)*) => {
        let mut sel = $crate::Select::new();
        chan_select!(
            sel,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };
    (default => $default:expr, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+$(,)*) => {
        let mut sel = $crate::Select::new();
        chan_select!(
            sel,
            default => $default,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };

    ($select:ident, default => $default:expr, $(
        $chan:ident$(($($dontcare:expr)*))*.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+$(,)*) => {
        compile_error!("The channel name in each case must be a simple identifier, not a function call.  At least one of the cases violates this rule.");
    };
    (default => $default:expr, $(
        $chan:ident$(($($dontcare:expr)*))*.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+$(,)*) => {
        compile_error!("The channel name in each case must be a simple identifier, not a function call.  At least one of the cases violates this rule.");
    };

    ($select:ident, default => $default:expr, $(
        $($chan:ident$(($($dontcare:expr)*))*).+
        $(-> $name:pat)* => $code:expr
    ),+$(,)*) => {
        compile_error!("The channel name in each case must be a simple identifier, not a function call, and you cannot access any members of that identifier other than the method (e.g. recv()). At least one of the cases violates this rule.");
    };
    (default => $default:expr, $(
        $($chan:ident$(($($dontcare:expr)*))*).+
        $(-> $name:pat)* => $code:expr
    ),+$(,)*) => {
        compile_error!("The channel name in each case must be a simple identifier, not a function call, and you cannot access any members of that identifier other than the method (e.g. recv()). At least one of the cases violates this rule.");
    };

    ($select:ident, default => $default:expr, $($tt:tt)*) => {
        compile_error!("There is a comma missing after one of the select cases.");
    };
    (default => $default:expr, $($tt:tt)*) => {
        compile_error!("There is a comma missing after one of the select cases.");
    };

    ($select:ident, default => $($tt:tt)*) => {
        compile_error!("There is a comma missing after the default case.");
    };
    (default => $($tt:tt)*) => {
        compile_error!("There is a comma missing after the default case.");
    };
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::{WaitGroup, async, sync};

    #[test]
    fn simple() {
        let (send, recv) = sync(1);
        send.send(5);
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_rendezvous() {
        let (send, recv) = sync(0);
        thread::spawn(move || send.send(5));
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_async() {
        let (send, recv) = async();
        send.send(5);
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_iter() {
        let (send, recv) = sync(1);
        thread::spawn(move || {
            for i in 0..100 {
                send.send(i);
            }
        });
        let recvd: Vec<i32> = recv.iter().collect();
        assert_eq!(recvd, (0..100).collect::<Vec<i32>>());
    }

    #[test]
    fn simple_iter_rendezvous() {
        let (send, recv) = sync(0);
        thread::spawn(move || {
            for i in 0..100 {
                send.send(i);
            }
        });
        let recvd: Vec<i32> = recv.iter().collect();
        assert_eq!(recvd, (0..100).collect::<Vec<i32>>());
    }

    #[test]
    fn simple_iter_async() {
        let (send, recv) = async();
        thread::spawn(move || {
            for i in 0..100 {
                send.send(i);
            }
        });
        let recvd: Vec<i32> = recv.iter().collect();
        assert_eq!(recvd, (0..100).collect::<Vec<i32>>());
    }

    #[test]
    fn simple_try() {
        let (send, recv) = sync(1);
        send.try_send(5).is_err();
        recv.try_recv().is_err();
    }

    #[test]
    fn simple_try_rendezvous() {
        let (send, recv) = sync(0);
        send.try_send(5).is_err();
        recv.try_recv().is_err();
    }

    #[test]
    fn simple_try_async() {
        let (send, recv) = async();
        recv.try_recv().is_err();
        send.try_send(5).is_ok();
    }

    #[test]
    fn select_manual() {
        let (s1, r1) = sync(1);
        let (s2, r2) = sync(1);
        s1.send(1);
        s2.send(2);

        let mut sel = ::Select::new();
        let mut sel = &mut sel;
        let c1 = sel.recv(&r1);
        let c2 = sel.recv(&r2);
        let which = sel.select();
        if which == c1.id() {
            println!("r1");
        } else if which == c2.id() {
            println!("r2");
        } else {
            unreachable!();
        }
    }

    #[test]
    fn select() {
        let (sticka, rticka) = sync(1);
        let (stickb, rtickb) = sync(1);
        let (stickc, rtickc) = sync(1);
        let (send, recv) = sync(0);
        thread::spawn(move || {
            loop {
                sticka.send("ticka");
                thread::sleep(Duration::from_millis(100));
                println!("RECV: {:?}", recv.recv());
            }
        });
        thread::spawn(move || {
            loop {
                stickb.send("tickb");
                thread::sleep(Duration::from_millis(50));
            }
        });
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1000));
            stickc.send(());
        });

        loop {
            let mut stop = false;
            chan_select! {
                rticka.recv() -> val => println!("{:?}", val),
                rtickb.recv() -> val => println!("{:?}", val),
                rtickc.recv() => stop = true,
                send.send("fubar".to_owned()) => println!("SENT!"),
            }
            if stop {
                break;
            }
        }
        println!("select done!");
    }

    // Regression test: https://github.com/BurntSushi/chan/issues/6
    #[test]
    fn select_no_leak_recv() {
        let (s0, r0) = sync::<i32>(0);
        let s1 = s0.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            s1.send(1);
        });

        // No subscriptions made yet.
        assert_eq!(s0.inner().notify.len(), 0);

        {
            let mut sel = ::Select::new();
            let mut sel = &mut sel;
            let c = sel.recv(&r0);
            assert_eq!(c.id(), sel.select());
            // Select hasn't been dropped yet and therefore hasn't
            // unsubscribed.
            assert_eq!(s0.inner().notify.len(), 1);
        }

        // Select should have been dropped and should have unsubscribed itself.
        assert_eq!(s0.inner().notify.len(), 0);
    }

    // Regression test: https://github.com/BurntSushi/chan/issues/6
    #[test]
    fn select_no_leak_send() {
        let (s0, r0) = sync::<i32>(0);
        let r1 = r0.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            r1.recv();
        });

        // No subscriptions made yet.
        assert_eq!(s0.inner().notify.len(), 0);

        {
            let mut sel = ::Select::new();
            let mut sel = &mut sel;
            let c = sel.send(&s0, 1);
            assert_eq!(c.id(), sel.select());
            // Select hasn't been dropped yet and therefore hasn't
            // unsubscribed.
            assert_eq!(s0.inner().notify.len(), 1);
        }

        // Select should have been dropped and should have unsubscribed itself.
        assert_eq!(s0.inner().notify.len(), 0);
    }

    #[test]
    fn mpmc() {
        let (send, recv) = sync(1);
        for i in 0..4 {
            let send = send.clone();
            thread::spawn(move || {
                for work in vec!['a', 'b', 'c'] {
                    send.send((i, work));
                }
            });
        }
        let wg_done = WaitGroup::new();
        for i in 0..4 {
            wg_done.add(1);
            let wg_done = wg_done.clone();
            let recv = recv.clone();
            thread::spawn(move || {
                for (sent_from, work) in recv {
                    println!("sent from {} to {}, work: {}",
                             sent_from, i, work);
                }
                println!("worker {} done!", i);
                wg_done.done();
            });
        }
        drop(send);
        wg_done.wait();
        println!("mpmc done!");
    }
}
