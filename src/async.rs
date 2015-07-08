use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};

use notifier::Notifier;
use {Channel, Receiver, Sender, ChannelId};

static NEXT_CHANNEL_ID: AtomicUsize = ATOMIC_USIZE_INIT;

#[derive(Debug)]
pub struct AsyncChannel<T>(Arc<AsyncInner<T>>);

struct AsyncInner<T> {
    id: u64,
    notify: Notifier,
    cond: Condvar,
    queue: Mutex<AsyncQueue<T>>,
}

struct AsyncQueue<T> {
    queue: VecDeque<T>,
    closed: bool,
}

impl<T> AsyncChannel<T> {
    pub fn new() -> AsyncChannel<T> {
        AsyncChannel(Arc::new(AsyncInner {
            id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst) as u64,
            notify: Notifier::new(),
            cond: Condvar::new(),
            queue: Mutex::new(AsyncQueue {
                queue: VecDeque::with_capacity(1024),
                closed: false,
            }),
        }))
    }

    fn _recv(&self, try: bool) -> Result<Option<T>, ()> {
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
        self.0.cond.notify_all();
        self.0.notify.notify();
        Ok(Some(val))
    }
}

impl<T> Channel for AsyncChannel<T> {
    type Item = T;

    fn id(&self) -> ChannelId {
        ChannelId::sender(self.0.id)
    }

    fn subscribe(&self, condvar: Arc<Condvar>) -> u64 {
        self.0.notify.subscribe(condvar)
    }

    fn unsubscribe(&self, key: u64) {
        self.0.notify.unsubscribe(key)
    }
}

impl<T> Sender for AsyncChannel<T> {
    fn send(&self, val: T) {
        self.try_send(val).ok().unwrap();
    }

    fn try_send(&self, val: T) -> Result<(), T> {
        let mut queue = self.0.queue.lock().unwrap();
        queue.queue.push_back(val);
        self.0.cond.notify_all();
        self.0.notify.notify();
        Ok(())
    }

    fn close(&self) {
        let mut queue = self.0.queue.lock().unwrap();
        queue.closed = true;
        self.0.cond.notify_all();
        self.0.notify.notify();
    }
}

impl<T> Receiver for AsyncChannel<T> {
    fn recv(&self) -> Option<T> {
        self._recv(false).unwrap()
    }

    fn try_recv(&self) -> Result<Option<T>, ()> {
        self._recv(true)
    }
}

impl<T> Clone for AsyncChannel<T> {
    fn clone(&self) -> AsyncChannel<T> {
        AsyncChannel(self.0.clone())
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
