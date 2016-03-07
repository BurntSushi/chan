use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, RwLock};

// This data structure is used to track subscriptions to channels.
//
// The flow is that when a "select" like construct is made, then it must
// tell the channels it wants to synchronize with to notify it when some
// activity occurs (which may cause the "select" to resolve one of the
// synchronization events). This is managed by a simple pub/sub model below.
//
// This implementation is extremely naive and probably very inefficient.
// Notably, *every* channel send/recv calls `notify`, which needs to acquire
// a lock and broadcast to every select's condition variable.
//
// N.B. next_id does checked arithmetic, so if one channel is exposed to
// 2^64 subscribers (not necessarily at the same time), then the program
// will crash. This seems like a bad limitation, but like I said, this is
// a naive implementation.

pub struct Notifier(RwLock<Inner>);

struct Inner {
    next_id: u64,
    subscriptions: HashMap<u64, Subscription>,
}

struct Subscription {
    mutex: Arc<Mutex<()>>,
    cond: Arc<Condvar>,
}

impl Notifier {
    pub fn new() -> Notifier {
        Notifier(RwLock::new(Inner {
            next_id: 0,
            subscriptions: HashMap::new(),
        }))
    }

    pub fn notify(&self) {
        let notify = self.0.read().unwrap();
        for sub in notify.subscriptions.values() {
            let _lock = sub.mutex.lock().unwrap();
            sub.cond.notify_all();
        }
    }

    pub fn subscribe(
        &self,
        mutex: Arc<Mutex<()>>,
        condvar: Arc<Condvar>,
    ) -> u64 {
        let mut notify = self.0.write().unwrap();
        let id = notify.next_id;
        notify.next_id = notify.next_id.checked_add(1).unwrap();
        notify.subscriptions.insert(id, Subscription {
            mutex: mutex,
            cond: condvar,
        });
        id
    }

    pub fn unsubscribe(&self, key: u64) {
        let mut notify = self.0.write().unwrap();
        notify.subscriptions.remove(&key);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        let notify = self.0.read().unwrap();
        notify.subscriptions.len()
    }
}

impl fmt::Debug for Notifier {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let notify = self.0.read().unwrap();
        writeln!(f, "Notifier({:?})",
                 notify.subscriptions.keys().collect::<Vec<_>>())
    }
}
