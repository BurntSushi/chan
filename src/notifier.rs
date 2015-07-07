use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};

pub struct Notifier(Mutex<Inner>);

struct Inner {
    next_id: u64,
    subscriptions: HashMap<u64, Arc<Condvar>>,
}

impl Notifier {
    pub fn new() -> Notifier {
        Notifier(Mutex::new(Inner {
            next_id: 0,
            subscriptions: HashMap::new(),
        }))
    }

    pub fn notify(&self) {
        let notify = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for condvar in notify.subscriptions.values() {
            condvar.notify_all();
        }
    }

    pub fn subscribe(&self, condvar: Arc<Condvar>) -> u64 {
        let mut notify = self.0.lock().unwrap();
        let id = notify.next_id;
        notify.next_id = notify.next_id.checked_add(1).unwrap();
        notify.subscriptions.insert(id, condvar);
        id
    }

    pub fn unsubscribe(&self, key: u64) {
        let mut notify = self.0.lock().unwrap();
        notify.subscriptions.remove(&key);
    }
}

impl fmt::Debug for Notifier {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let notify = self.0.lock().unwrap();
        writeln!(f, "Notifier({:?})",
                 notify.subscriptions.keys().collect::<Vec<_>>())
    }
}
