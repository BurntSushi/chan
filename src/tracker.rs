use std::sync::Mutex;

pub struct Tracker(Mutex<TrackerCounts>);

struct TrackerCounts {
    senders: usize,
    receivers: usize,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker(Mutex::new(TrackerCounts { senders: 0, receivers: 0 }))
    }

    pub fn add_sender(&self) {
        let mut counts = self.0.lock().unwrap();
        counts.senders += 1;
    }

    pub fn add_receiver(&self) {
        let mut counts = self.0.lock().unwrap();
        counts.receivers += 1;
    }

    pub fn remove_sender<F: FnMut()>(&self, mut at_zero: F) {
        let mut counts = self.0.lock().unwrap();
        assert!(counts.senders > 0);
        counts.senders -= 1;
        if counts.senders == 0 {
            at_zero();
        }
    }

    pub fn remove_receiver<F: FnMut()>(&self, mut at_zero: F) {
        let mut counts = self.0.lock().unwrap();
        assert!(counts.receivers > 0);
        counts.receivers -= 1;
        if counts.receivers == 0 {
            at_zero();
        }
    }
}
