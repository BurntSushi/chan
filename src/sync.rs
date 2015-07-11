use std::fmt;
use std::ops::Drop;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};

use notifier::Notifier;
use tracker::Tracker;
use {Channel, Receiver, Sender, ChannelId};

// This enables us to (in practice) uniquely identify any particular channel.
// A better approach would be to use the pointer's address in memory, but it
// looks like `Arc` doesn't support that (yet?).
//
// Any other ideas? ---AG
//
// N.B. This is combined with ChannelId to distinguish between the sending
// and receiving halves of a channel.
static NEXT_CHANNEL_ID: AtomicUsize = ATOMIC_USIZE_INIT;

pub fn sync<T>(size: usize) -> (SyncSender<T>, SyncReceiver<T>) {
    let send = SyncChannel::new(size);
    let recv = send.clone();
    (send.into_sender(), recv.into_receiver())
}

#[derive(Debug)]
pub struct SyncSender<T>(SyncChannel<T>);

#[derive(Debug)]
pub struct SyncReceiver<T>(SyncChannel<T>);

#[derive(Debug)]
struct SyncChannel<T>(Arc<SyncInner<T>>);

struct SyncInner<T> {
    id: u64,
    notify: Notifier,
    track: Tracker,
    cond: Condvar,
    cap: usize,
    data: Mutex<Data<T>>,
}

#[derive(Debug)]
pub struct Data<T> {
    closed: bool,
    nwaiting: usize,
    queue: Ring<T>,
}

#[derive(Debug)]
struct Ring<T> {
    queue: Vec<Option<T>>,
    pos: usize,
    len: usize,
}

impl<T> SyncChannel<T> {
    fn new(size: usize) -> SyncChannel<T> {
        let queue = if size == 0 {
            vec![None]
        } else {
            let mut queue = Vec::with_capacity(size);
            for _ in 0..size { queue.push(None); }
            queue
        };
        SyncChannel(Arc::new(SyncInner {
            id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst) as u64,
            notify: Notifier::new(),
            track: Tracker::new(),
            cond: Condvar::new(),
            cap: size,
            data: Mutex::new(Data {
                closed: false,
                nwaiting: 0,
                queue: Ring {
                    queue: queue,
                    pos: 0,
                    len: 0,
                },
            }),
        }))
    }

    fn into_sender(self) -> SyncSender<T> {
        self.0.track.add_sender();
        SyncSender(self)
    }

    fn into_receiver(self) -> SyncReceiver<T> {
        self.0.track.add_receiver();
        SyncReceiver(self)
    }
}

impl<T> SyncInner<T> {
    fn close(&self) {
        let mut data = self.data.lock().unwrap();
        data.closed = true;
        self.notify(None);
    }

    fn notify(&self, from: Option<u64>) {
        self.cond.notify_all();
        self.notify.notify(from);
    }

    fn buffered_send(&self, val: T, try: bool, from: Option<u64>) -> Result<(), T> {
        let mut data = self.data.lock().unwrap();
        while data.queue.len == self.cap {
            // We *need* two of these checks. This is here because if the
            // channel is already closed, then the condition variable may
            // never be woken up again, and thus, we'll be dead-locked.
            if data.closed {
                drop(data); // don't poison
                panic!("cannot send on a closed channel");
            }
            if try {
                return Err(val);
            }
            data = self.cond.wait(data).unwrap();
        }
        // ... and this is necessary because the channel may have been
        // closed while we were waiting for the queue to empty. And we
        // absolutely cannot abide adding to the queue if the channel
        // has been closed.
        //
        // N.B. This should not be able to happen in safe code, because it
        // implies a client has a reference to something with a refcount of
        // zero.
        if data.closed {
            drop(data); // don't poison
            panic!("cannot send on a closed channel");
        }
        data.queue.push(val);
        self.notify(from);
        Ok(())
    }

    fn buffered_recv(&self, try: bool, from: Option<u64>) -> Result<Option<T>, ()> {
        let mut data = self.data.lock().unwrap();
        while data.queue.len == 0 {
            if data.closed {
                return Ok(None);
            }
            if try {
                return Err(());
            }
            data = self.cond.wait(data).unwrap();
        }
        let val = data.queue.pop();
        self.notify(from);
        Ok(Some(val))
    }

    fn unbuffered_send(&self, send_val: T, try: bool, from: Option<u64>) -> Result<(), T> {
        let mut data = self.data.lock().unwrap();
        while data.queue.has_value() {
            if try {
                return Err(send_val);
            }
            data = self.cond.wait(data).unwrap();
        }

        if data.closed {
            drop(data); // don't poison
            panic!("cannot send on a closed channel");
        }
        if try && data.nwaiting == 0 {
            return Err(send_val);
        }
        data.queue.put(send_val);
        self.notify(from);
        // At this point, any blocked receivers have woken up and will race
        // to access `val`. So we release the mutex but continue blocking
        // until a receiver has retrieved the value.
        // If there are no blocked receivers, then we continue blocking
        // until there is one that grabs the value.
        while data.queue.has_value() {
            // It's possible we could wake up here by the broadcast from
            // `close`, but that's OK: the value was added to the queue
            // before `close` was called, which means a receiver can still
            // retrieve it.
            data = self.cond.wait(data).unwrap();
        }
        // OK, if we're here, then the value we put in has been slurped up
        // by a receiver *and* we've re-acquired the `val` lock. Now we
        // release it and the sender lock to permit other senders to try.
        drop(data);
        // We notify after the lock has been released so that the next time
        // a sender tries to send, it will absolutely not be blocked by *this*
        // send.
        self.notify(from);
        Ok(())
    }

    fn unbuffered_recv(&self, try: bool, from: Option<u64>) -> Result<Option<T>, ()> {
        let mut data = self.data.lock().unwrap();
        while !data.queue.has_value() {
            if data.closed {
                return Ok(None);
            }
            if try {
                return Err(());
            }
            // We need to notify in case there are any blocking sends.
            // This will wake them up and cause them to try and send
            // something (after we release the `val` lock).
            self.notify(from);
            data.nwaiting += 1;
            data = self.cond.wait(data).unwrap();
            data.nwaiting -= 1;
        }
        let recv_val = data.queue.take();
        self.notify(from);
        Ok(Some(recv_val))
    }
}

impl<T> Ring<T> {
    // Only used for buffered: add new element to queue.
    // Assumes that len < cap, panics otherwise.
    fn push(&mut self, val: T) {
        let (pos, len, cap) = (self.pos, self.len, self.queue.len());
        assert!(len < cap);
        self.queue[(pos + len) % cap] = Some(val);
        self.len += 1;
    }

    // Only used for buffered: pop least recently added element.
    // Assumes that 0 < len <= cap, panics otherwise.
    fn pop(&mut self) -> T {
        let (pos, len, cap) = (self.pos, self.len, self.queue.len());
        assert!(len <= cap);
        assert!(len > 0);
        let val = self.queue[pos].take().expect("non-null item in queue");
        self.pos = (pos + 1) % cap;
        self.len -= 1;
        val
    }

    // Only used for unbuffered: put single element.
    // Assumes that len(queue) == 1, panics otherwise.
    fn put(&mut self, val: T) {
        assert!(self.queue.len() == 1);
        self.queue[0] = Some(val);
    }

    // Only used for unbuffered: pop single element.
    // Assumes that len(queue) == 1, panics otherwise.
    fn take(&mut self) -> T {
        assert!(self.queue.len() == 1);
        self.queue[0].take().expect("non-null item from sender")
    }

    // Only used for unbuffered: return true if there's an element.
    // Assumes that len(queue) == 1, panics otherwise.
    fn has_value(&self) -> bool {
        assert!(self.queue.len() == 1);
        self.queue[0].is_some()
    }
}

impl<T> Clone for SyncChannel<T> {
    fn clone(&self) -> SyncChannel<T> {
        SyncChannel(self.0.clone())
    }
}

impl<T> Clone for SyncSender<T> {
    fn clone(&self) -> SyncSender<T> {
        self.0.clone().into_sender()
    }
}

impl<T> Clone for SyncReceiver<T> {
    fn clone(&self) -> SyncReceiver<T> {
        self.0.clone().into_receiver()
    }
}

impl<T> Drop for SyncSender<T> {
    fn drop(&mut self) {
        (self.0).0.track.remove_sender(|| (self.0).0.close());
    }
}

impl<T> Drop for SyncReceiver<T> {
    fn drop(&mut self) {
        (self.0).0.track.remove_receiver(|| ());
    }
}

impl<T> Channel for SyncSender<T> {
    type Item = T;
    type GuardItem = Data<T>;

    fn id(&self) -> ChannelId {
        ChannelId::sender((self.0).0.id)
    }

    fn subscribe(&self, id: u64, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        (self.0).0.notify.subscribe(id, mutex, condvar)
    }

    fn unsubscribe(&self, key: u64) {
        (self.0).0.notify.unsubscribe(key);
    }

    fn lock<'a>(&'a self) -> MutexGuard<'a, Data<T>> {
        (self.0).0.data.lock().unwrap()
        // let mut lock = Some((self.0).0.data.lock().unwrap());
        // Box::new(move || drop(lock.take().unwrap()))
    }
}

impl<T> Channel for SyncReceiver<T> {
    type Item = T;
    type GuardItem = Data<T>;

    fn id(&self) -> ChannelId {
        ChannelId::receiver((self.0).0.id)
    }

    fn subscribe(&self, id: u64, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        (self.0).0.notify.subscribe(id, mutex, condvar)
    }

    fn unsubscribe(&self, key: u64) {
        (self.0).0.notify.unsubscribe(key);
    }

    fn lock<'a>(&'a self) -> MutexGuard<'a, Data<T>> {
        (self.0).0.data.lock().unwrap()
        // let mut lock = Some((self.0).0.data.lock().unwrap());
        // Box::new(move || drop(lock.take().unwrap()))
    }
}

impl<T> Sender for SyncSender<T> {
    fn send(&self, val: T) {
        if (self.0).0.cap == 0 {
            (self.0).0.unbuffered_send(val, false, None).ok().unwrap();
        } else {
            (self.0).0.buffered_send(val, false, None).ok().unwrap();
        }
    }

    fn try_send(&self, val: T) -> Result<(), T> {
        if (self.0).0.cap == 0 {
            (self.0).0.unbuffered_send(val, true, None)
        } else {
            (self.0).0.buffered_send(val, true, None)
        }
    }

    fn try_send_from(&self, val: T, id: u64) -> Result<(), T> {
        if (self.0).0.cap == 0 {
            (self.0).0.unbuffered_send(val, true, Some(id))
        } else {
            (self.0).0.buffered_send(val, true, Some(id))
        }
    }
}

impl<T> Receiver for SyncReceiver<T> {
    fn recv(&self) -> Option<T> {
        if (self.0).0.cap == 0 {
            (self.0).0.unbuffered_recv(false, None).unwrap()
        } else {
            (self.0).0.buffered_recv(false, None).unwrap()
        }
    }

    fn try_recv(&self) -> Result<Option<T>, ()> {
        if (self.0).0.cap == 0 {
            (self.0).0.unbuffered_recv(true, None)
        } else {
            (self.0).0.buffered_recv(true, None)
        }
    }

    fn try_recv_from(&self, id: u64) -> Result<Option<T>, ()> {
        if (self.0).0.cap == 0 {
            (self.0).0.unbuffered_recv(true, Some(id))
        } else {
            (self.0).0.buffered_recv(true, Some(id))
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for SyncInner<T> {
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

// fn try_lock<T>(mutex: &Mutex<T>, try: bool) -> Option<MutexGuard<T>> {
    // if !try {
        // Some(mutex.lock().unwrap())
    // } else {
        // match mutex.try_lock() {
            // Ok(g) => Some(g),
            // Err(TryLockError::Poisoned(_)) => panic!("poisoned mutex"),
            // Err(TryLockError::WouldBlock) => None,
        // }
    // }
// }

#[cfg(test)]
mod tests {
    use Sender;
    use super::{SyncSender, sync};

    #[test]
    #[should_panic]
    fn no_send_on_close() {
        let (send, _) = sync(1);
        // cheat and get a sender without increasing sender count.
        // (this is only possible with private API!)
        let cheat_send = SyncSender(send.0.clone());
        drop(send);
        // Ok, increase sender count now, after the channel has already
        // been closed.
        ::std::mem::forget(cheat_send.clone());
        cheat_send.send(5);
    }

    #[test]
    #[should_panic]
    fn no_send_on_close_unbuffered() {
        // See comments in test `no_send_on_close` for explanation.
        let (send, _) = sync(0);
        let cheat_send = SyncSender(send.0.clone());
        drop(send);
        ::std::mem::forget(cheat_send.clone());
        cheat_send.send(5);
    }
}
