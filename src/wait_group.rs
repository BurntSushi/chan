use std::fmt;
use std::sync::{Arc, Condvar, Mutex};

/// `WaitGroup` provides synchronization on the completion of threads.
///
/// For each thread involved in the synchronization, `add(1)` should be
/// called. Just before a thread terminates, it should call `done`.
/// To synchronize, call `wait`, which will block until the number of `done`
/// calls corresponds to the number of `add(1)` calls.
///
/// # Example
///
/// ```
/// use std::thread;
///
/// let wg = chan::WaitGroup::new();
///
/// for _ in 0..4 {
///     wg.add(1);
///     let wg = wg.clone();
///     thread::spawn(move || {
///         // do some work.
///
///         // And now call done.
///         wg.done();
///     });
/// }
/// // Blocks until `wg.done()` is called for each thread we spawned.
/// wg.wait()
/// ```
#[derive(Clone)]
pub struct WaitGroup(Arc<WaitGroupInner>);

struct WaitGroupInner {
    cond: Condvar,
    count: Mutex<i32>,
}

impl WaitGroup {
    /// Create a new wait group.
    pub fn new() -> WaitGroup {
        WaitGroup(Arc::new(WaitGroupInner {
            cond: Condvar::new(),
            count: Mutex::new(0),
        }))
    }

    /// Add a new thread to the waitgroup.
    ///
    /// # Failure
    ///
    /// If the internal count drops below `0` as a result of calling `add`,
    /// then this function panics.
    pub fn add(&self, delta: i32) {
        let mut count = self.0.count.lock().unwrap();
        *count += delta;
        assert!(*count >= 0);
        self.0.cond.notify_all();
    }

    /// Mark a thread as having finished.
    ///
    /// (This is equivalent to calling `add(-1)`.)
    pub fn done(&self) {
        self.add(-1);
    }

    /// Wait until all threads have completed.
    ///
    /// This unblocks when the internal count is `0`.
    pub fn wait(&self) {
        let mut count = self.0.count.lock().unwrap();
        while *count > 0 {
            count = self.0.cond.wait(count).unwrap();
        }
    }
}

impl fmt::Debug for WaitGroup {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let count = self.0.count.lock().unwrap();
        write!(f, "WaitGroup {{ count: {:?} }}", *count)
    }
}
