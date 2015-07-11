use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};

use notifier::Notifier;
use tracker::Tracker;
use {Channel, Receiver, Sender, ChannelId};

static NEXT_CHANNEL_ID: AtomicUsize = ATOMIC_USIZE_INIT;

pub fn async<T>() -> (AsyncSender<T>, AsyncReceiver<T>) {
    let send = AsyncChannel::new();
    let recv = send.clone();
    (send.into_sender(), recv.into_receiver())
}

#[derive(Debug)]
pub struct AsyncSender<T>(AsyncChannel<T>);

#[derive(Debug)]
pub struct AsyncReceiver<T>(AsyncChannel<T>);

#[derive(Debug)]
struct AsyncChannel<T>(Arc<AsyncInner<T>>);

struct AsyncInner<T> {
    id: u64,
    notify: Notifier,
    track: Tracker,
    cond: Condvar,
    queue: Mutex<AsyncQueue<T>>,
}

struct AsyncQueue<T> {
    queue: VecDeque<T>,
    closed: bool,
}

impl<T> AsyncChannel<T> {
    fn new() -> AsyncChannel<T> {
        AsyncChannel(Arc::new(AsyncInner {
            id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst) as u64,
            notify: Notifier::new(),
            track: Tracker::new(),
            cond: Condvar::new(),
            queue: Mutex::new(AsyncQueue {
                queue: VecDeque::with_capacity(1024),
                closed: false,
            }),
        }))
    }

    fn _send(&self, val: T, from: Option<u64>) -> Result<(), T> {
        let mut queue = self.0.queue.lock().unwrap();
        queue.queue.push_back(val);
        drop(queue);
        self.0.cond.notify_all();
        self.0.notify.notify(from);
        Ok(())
    }

    fn _recv(&self, try: bool, from: Option<u64>) -> Result<Option<T>, ()> {
        let mut queue = self.0.queue.lock().unwrap();
        while queue.queue.len() == 0 {
            if queue.closed {
                return Ok(None);
            }
            if try {
                return Err(());
            }
            queue = self.0.cond.wait(queue).unwrap();
        }
        let val = queue.queue.pop_front().unwrap();
        drop(queue);
        self.0.cond.notify_all();
        self.0.notify.notify(from);
        Ok(Some(val))
    }

    fn into_sender(self) -> AsyncSender<T> {
        self.0.track.add_sender();
        AsyncSender(self)
    }

    fn into_receiver(self) -> AsyncReceiver<T> {
        self.0.track.add_receiver();
        AsyncReceiver(self)
    }

    fn close(&self) {
        let mut queue = self.0.queue.lock().unwrap();
        queue.closed = true;
        drop(queue);
        self.0.cond.notify_all();
        self.0.notify.notify(None);
    }
}

impl<T> Clone for AsyncChannel<T> {
    fn clone(&self) -> AsyncChannel<T> {
        AsyncChannel(self.0.clone())
    }
}

impl<T> Clone for AsyncSender<T> {
    fn clone(&self) -> AsyncSender<T> {
        self.0.clone().into_sender()
    }
}

impl<T> Clone for AsyncReceiver<T> {
    fn clone(&self) -> AsyncReceiver<T> {
        self.0.clone().into_receiver()
    }
}

impl<T> Drop for AsyncSender<T> {
    fn drop(&mut self) {
        (self.0).0.track.remove_sender(|| self.0.close());
    }
}

impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        (self.0).0.track.remove_receiver(||());
    }
}

impl<T> Channel for AsyncSender<T> {
    type Item = T;
    type GuardItem = AsyncQueue<T>;

    fn id(&self) -> ChannelId {
        ChannelId::sender((self.0).0.id)
    }

    fn subscribe(&self, id: u64, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        (self.0).0.notify.subscribe(id, mutex, condvar)
    }

    fn unsubscribe(&self, key: u64) {
        (self.0).0.notify.unsubscribe(key)
    }

    fn lock<'a>(&'a self) -> MutexGuard<'a, AsyncQueue<T>> {
        (self.0).0.queue.lock().unwrap()
        // let mut lock = Some((self.0).0.queue.lock().unwrap());
        // Box::new(move || drop(lock.take().unwrap()))
    }
}

impl<T> Channel for AsyncReceiver<T> {
    type Item = T;
    type GuardItem = AsyncQueue<T>;

    fn id(&self) -> ChannelId {
        ChannelId::receiver((self.0).0.id)
    }

    fn subscribe(&self, id: u64, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        (self.0).0.notify.subscribe(id, mutex, condvar)
    }

    fn unsubscribe(&self, key: u64) {
        (self.0).0.notify.unsubscribe(key)
    }

    fn lock<'a>(&'a self) -> MutexGuard<'a, AsyncQueue<T>> {
        (self.0).0.queue.lock().unwrap()
        // let mut lock = Some((self.0).0.queue.lock().unwrap());
        // Box::new(move || drop(lock.take().unwrap()))
    }
}

impl<T> Sender for AsyncSender<T> {
    fn send(&self, val: T) {
        (self.0)._send(val, None).ok().unwrap();
    }

    fn try_send(&self, val: T) -> Result<(), T> {
        (self.0)._send(val, None)
    }

    fn try_send_from(&self, val: T, from: u64) -> Result<(), T> {
        (self.0)._send(val, Some(from))
    }
}

impl<T> Receiver for AsyncReceiver<T> {
    fn recv(&self) -> Option<T> {
        (self.0)._recv(false, None).unwrap()
    }

    fn try_recv(&self) -> Result<Option<T>, ()> {
        (self.0)._recv(true, None)
    }

    fn try_recv_from(&self, from: u64) -> Result<Option<T>, ()> {
        (self.0)._recv(true, Some(from))
    }
}

impl<T: fmt::Debug> fmt::Debug for AsyncInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let queue = self.queue.lock().unwrap();
        try!(writeln!(f, "AsyncInner {{"));
        try!(writeln!(f, "    notify: {:?}", self.notify));
        try!(writeln!(f, "    closed: {:?}", queue.closed));
        try!(writeln!(f, "    queue: {:?}", queue.queue));
        try!(writeln!(f, "}}"));
        Ok(())
    }
}
