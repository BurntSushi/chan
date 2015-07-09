use std::sync::atomic::{AtomicUsize, Ordering};

// A tracker that reference counts the two sides of a channel.
//
// After writing this, it seems like a blatant (and naive) reproduction of
// `Arc`. Can we instead just use `Arc` with a `Drop` impl on the inner type?

pub struct Tracker {
    senders: AtomicUsize,
    receivers: AtomicUsize,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker {
            senders: AtomicUsize::new(0),
            receivers: AtomicUsize::new(0),
        }
    }

    pub fn add_sender(&self) {
        self.senders.fetch_add(1, Ordering::SeqCst);
    }

    pub fn add_receiver(&self) {
        self.receivers.fetch_add(1, Ordering::SeqCst);
    }

    pub fn remove_sender<F: FnMut()>(&self, mut at_zero: F) {
        let prev = self.senders.fetch_sub(1, Ordering::SeqCst);
        assert!(prev > 0);
        if prev == 1 {
            at_zero();
        }
    }

    pub fn remove_receiver<F: FnMut()>(&self, mut at_zero: F) {
        let prev = self.receivers.fetch_sub(1, Ordering::SeqCst);
        assert!(prev > 0);
        if prev == 1 {
            at_zero();
        }
    }

    // We may want these methods for detecting deadlock and returning an error.

    #[allow(dead_code)]
    pub fn any_senders(&self) -> bool {
        self.senders.load(Ordering::SeqCst) > 0
    }

    #[allow(dead_code)]
    pub fn any_receivers(&self) -> bool {
        self.receivers.load(Ordering::SeqCst) > 0
    }
}
