use std::fmt;
use std::ops::Drop;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
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

#[derive(Debug)]
enum SyncInner<T> {
    Unbuffered(Unbuffered<T>),
    Buffered(Buffered<T>),
}

struct Unbuffered<T> {
    id: u64,
    notify: Notifier,
    track: Tracker,
    cond: Condvar,
    sender: Mutex<()>,
    val: Mutex<UnbufferedValue<T>>,
}

#[derive(Debug)]
struct UnbufferedValue<T> {
    val: Option<T>,
    closed: bool,
    nwaiting: usize,
}

struct Buffered<T> {
    id: u64,
    notify: Notifier,
    track: Tracker,
    cap: usize,
    cond: Condvar,
    ring: Mutex<Ring<T>>,
}

#[derive(Debug)]
struct Ring<T> {
    queue: Vec<Option<T>>,
    pos: usize,
    len: usize,
    closed: bool,
}

impl<T> SyncChannel<T> {
    fn new(size: usize) -> SyncChannel<T> {
        let inner = if size == 0 {
            SyncInner::Unbuffered(Unbuffered {
                id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst) as u64,
                notify: Notifier::new(),
                track: Tracker::new(),
                cond: Condvar::new(),
                sender: Mutex::new(()),
                val: Mutex::new(UnbufferedValue {
                    val: None,
                    closed: false,
                    nwaiting: 0,
                }),
            })
        } else {
            let mut queue = Vec::with_capacity(size);
            for _ in 0..size { queue.push(None); }
            SyncInner::Buffered(Buffered {
                id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst) as u64,
                notify: Notifier::new(),
                track: Tracker::new(),
                cap: size,
                cond: Condvar::new(),
                ring: Mutex::new(Ring {
                    queue: queue,
                    pos: 0,
                    len: 0,
                    closed: false,
                }),
            })
        };
        SyncChannel(Arc::new(inner))
    }

    fn id(&self) -> u64 {
        match *self.0 {
            SyncInner::Unbuffered(ref i) => i.id,
            SyncInner::Buffered(ref i) => i.id,
        }
    }

    fn track(&self) -> &Tracker {
        match *self.0 {
            SyncInner::Unbuffered(ref i) => &i.track,
            SyncInner::Buffered(ref i) => &i.track,
        }
    }

    #[allow(dead_code)]
    fn cond_notify(&self) {
        match *self.0 {
            SyncInner::Unbuffered(ref i) => i.cond.notify_all(),
            SyncInner::Buffered(ref i) => i.cond.notify_all(),
        }
    }

    fn notify(&self) -> &Notifier {
        match *self.0 {
            SyncInner::Unbuffered(ref i) => &i.notify,
            SyncInner::Buffered(ref i) => &i.notify,
        }
    }

    fn into_sender(self) -> SyncSender<T> {
        self.track().add_sender();
        SyncSender(self)
    }

    fn into_receiver(self) -> SyncReceiver<T> {
        self.track().add_receiver();
        SyncReceiver(self)
    }

    fn close(&self) {
        match *self.0 {
            SyncInner::Unbuffered(ref i) => i.close(),
            SyncInner::Buffered(ref i) => i.close(),
        }
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
        self.0.track().remove_sender(|| self.0.close());
    }
}

impl<T> Drop for SyncReceiver<T> {
    fn drop(&mut self) {
        self.0.track().remove_receiver(|| ());
    }
}

impl<T> Channel for SyncSender<T> {
    type Item = T;

    fn id(&self) -> ChannelId {
        ChannelId::sender(self.0.id())
    }

    fn subscribe(&self, id: u64, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        self.0.notify().subscribe(id, mutex, condvar)
    }

    fn unsubscribe(&self, key: u64) {
        self.0.notify().unsubscribe(key);
    }
}

impl<T> Channel for SyncReceiver<T> {
    type Item = T;

    fn id(&self) -> ChannelId {
        ChannelId::receiver(self.0.id())
    }

    fn subscribe(&self, id: u64, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        self.0.notify().subscribe(id, mutex, condvar)
    }

    fn unsubscribe(&self, key: u64) {
        self.0.notify().unsubscribe(key);
    }
}

impl<T> Sender for SyncSender<T> {
    fn send(&self, val: T) {
        match *(self.0).0 {
            SyncInner::Unbuffered(ref i) => {
                i.send(val, false, None).ok().unwrap()
            }
            SyncInner::Buffered(ref i) => {
                i.send(val, false, None).ok().unwrap()
            }
        }
    }

    fn try_send(&self, val: T) -> Result<(), T> {
        match *(self.0).0 {
            SyncInner::Unbuffered(ref i) => i.send(val, true, None),
            SyncInner::Buffered(ref i) => i.send(val, true, None),
        }
    }

    fn try_send_from(&self, val: T, id: u64) -> Result<(), T> {
        match *(self.0).0 {
            SyncInner::Unbuffered(ref i) => i.send(val, true, Some(id)),
            SyncInner::Buffered(ref i) => i.send(val, true, Some(id)),
        }
    }
}

impl<T> Receiver for SyncReceiver<T> {
    fn recv(&self) -> Option<T> {
        match *(self.0).0 {
            SyncInner::Unbuffered(ref i) => i.recv(false, None).unwrap(),
            SyncInner::Buffered(ref i) => i.recv(false, None).unwrap(),
        }
    }

    fn try_recv(&self) -> Result<Option<T>, ()> {
        match *(self.0).0 {
            SyncInner::Unbuffered(ref i) => i.recv(true, None),
            SyncInner::Buffered(ref i) => i.recv(true, None),
        }
    }

    fn try_recv_from(&self, id: u64) -> Result<Option<T>, ()> {
        match *(self.0).0 {
            SyncInner::Unbuffered(ref i) => i.recv(true, Some(id)),
            SyncInner::Buffered(ref i) => i.recv(true, Some(id)),
        }
    }
}

impl<T> Buffered<T> {
    fn send(&self, val: T, try: bool, from: Option<u64>) -> Result<(), T> {
        let mut ring = self.ring.lock().unwrap();
        while ring.len == self.cap {
            // We *need* two of these checks. This is here because if the
            // channel is already closed, then the condition variable may
            // never be woken up again, and thus, we'll be dead-locked.
            if ring.closed {
                drop(ring); // don't poison
                panic!("cannot send on a closed channel");
            }
            if try {
                return Err(val);
            }
            ring = self.cond.wait(ring).unwrap();
        }
        // ... and this is necessary because the channel may have been
        // closed while we were waiting for the queue to empty. And we
        // absolutely cannot abide adding to the queue if the channel
        // has been closed.
        if ring.closed {
            drop(ring); // don't poison
            panic!("cannot send on a closed channel");
        }
        ring.push(val);
        drop(ring);
        // println!("send - in - notify from: {:?} (try: {:?})", from, try);
        self.notify.notify(from);
        // println!("send - out - notify from: {:?} (try: {:?})", from, try);
        self.cond.notify_all();
        Ok(())
    }

    fn recv(&self, try: bool, from: Option<u64>) -> Result<Option<T>, ()> {
        let mut ring = self.ring.lock().unwrap();
        while ring.len == 0 {
            if ring.closed {
                return Ok(None);
            }
            if try {
                return Err(());
            }
            ring = self.cond.wait(ring).unwrap();
        }
        let val = ring.pop();
        drop(ring);
        // println!("recv - in - notify from: {:?} (try: {:?})", from, try);
        self.notify.notify(from);
        // println!("recv - out - notify from: {:?} (try: {:?})", from, try);
        self.cond.notify_all();
        Ok(Some(val))
    }

    fn close(&self) {
        let mut ring = self.ring.lock().unwrap();
        ring.closed = true;
        drop(ring);
        self.cond.notify_all();
        self.notify.notify(None);
    }
}

impl<T> Ring<T> {
    fn push(&mut self, val: T) {
        let (pos, len, cap) = (self.pos, self.len, self.queue.len());
        assert!(len < cap);
        self.queue[(pos + len) % cap] = Some(val);
        self.len += 1;
    }

    fn pop(&mut self) -> T {
        let (pos, len, cap) = (self.pos, self.len, self.queue.len());
        assert!(len <= cap);
        assert!(len > 0);
        let val = self.queue[pos].take().expect("non-null item in queue");
        self.pos = (pos + 1) % cap;
        self.len -= 1;
        val
    }
}

impl<T: fmt::Debug> fmt::Debug for Buffered<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let ring = self.ring.lock().unwrap();
        try!(writeln!(f, "Buffered {{"));
        try!(writeln!(f, "    notify: {:?}", self.notify));
        try!(writeln!(f, "    cap: {:?}", self.cap));
        try!(writeln!(f, "    ring: {:?}", *ring));
        try!(writeln!(f, "}}"));
        Ok(())
    }
}

impl<T> Unbuffered<T> {
    fn send(&self, send_val: T, try: bool, from: Option<u64>) -> Result<(), T> {
        let _sender_lock = match try_lock(&self.sender, try) {
            None => return Err(send_val),
            Some(lock) => lock,
        };
        // Since the sender lock has been acquired, that implies any
        // previous senders have completed, which implies that all
        // receivers that could make progress have made progress, and the
        // rest are blocked. Therefore, `val` must be `None`.
        let mut val = self.val.lock().unwrap();
        if val.closed {
            drop(val); // don't poison
            drop(_sender_lock); // don't poison
            panic!("cannot send on a closed channel");
        }
        if try && val.nwaiting == 0 {
            return Err(send_val);
        }
        val.val = Some(send_val);
        self.cond.notify_all();
        drop(val);
        self.notify.notify(from);
        val = self.val.lock().unwrap();
        // At this point, any blocked receivers have woken up and will race
        // to access `val`. So we release the mutex but continue blocking
        // until a receiver has retrieved the value.
        // If there are no blocked receivers, then we continue blocking
        // until there is one that grabs the value.
        while val.val.is_some() {
            // It's possible we could wake up here by the broadcast from
            // `close`, but that's OK: the value was added to the queue
            // before `close` was called, which means a receiver can still
            // retrieve it.
            val = self.cond.wait(val).unwrap();
        }
        // OK, if we're here, then the value we put in has been slurped up
        // by a receiver *and* we've re-acquired the `val` lock. Now we
        // release it and the sender lock to permit other senders to try.
        drop(val);
        drop(_sender_lock);
        // We notify after the lock has been released so that the next time
        // a sender tries to send, it will absolutely not be blocked by *this*
        // send.
        self.notify.notify(from);
        Ok(())
    }

    fn recv(&self, try: bool, from: Option<u64>) -> Result<Option<T>, ()> {
        let mut val = self.val.lock().unwrap();
        while val.val.is_none() {
            if val.closed {
                return Ok(None);
            }
            if try {
                return Err(());
            }
            // We need to notify in case there are any blocking sends.
            // This will wake them up and cause them to try and send
            // something (after we release the `val` lock).
            self.notify.notify(from);
            self.cond.notify_all();
            val.nwaiting += 1;
            val = self.cond.wait(val).unwrap();
            val.nwaiting -= 1;
        }
        let recv_val = val.val.take().unwrap();
        drop(val);
        self.cond.notify_all();
        self.notify.notify(from);
        Ok(Some(recv_val))
    }

    fn close(&self) {
        let mut val = self.val.lock().unwrap();
        val.closed = true;
        drop(val);
        // If there are any blocked receivers, this will wake them up and
        // force them to return.
        self.cond.notify_all();
        self.notify.notify(None);
    }
}

impl<T: fmt::Debug> fmt::Debug for Unbuffered<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let val = self.val.lock().unwrap();
        try!(writeln!(f, "Unbuffered {{"));
        try!(writeln!(f, "    notify: {:?}", self.notify));
        try!(writeln!(f, "    val: {:?}", *val));
        try!(writeln!(f, "}}"));
        Ok(())
    }
}

fn try_lock<T>(mutex: &Mutex<T>, try: bool) -> Option<MutexGuard<T>> {
    if !try {
        Some(mutex.lock().unwrap())
    } else {
        match mutex.try_lock() {
            Ok(g) => Some(g),
            Err(TryLockError::Poisoned(_)) => panic!("poisoned mutex"),
            Err(TryLockError::WouldBlock) => None,
        }
    }
}

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
