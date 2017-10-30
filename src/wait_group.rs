use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::panicking;
use std::sync::atomic::{AtomicBool, Ordering};

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
    poisoned: AtomicBool,
}

impl Drop for WaitGroup {
    fn drop(&mut self) {
        let mut count = self.0.count.lock().unwrap();
        // We require count to be over zero to not trigger another panic when the WaitGroup
        // of the waiting thread gets dropped on panic.
        // Caveat: We can't be sure if we are panicking after done is being called
        if *count > 0 && panicking() {
            *count -= 1;
            assert!(*count >= 0);
            self.0.poisoned.store(true, Ordering::Release);
            self.0.cond.notify_all();
        }
    }
}

impl WaitGroup {
    /// Create a new wait group.
    pub fn new() -> WaitGroup {
        WaitGroup(Arc::new(WaitGroupInner {
            cond: Condvar::new(),
            count: Mutex::new(0),
            poisoned: AtomicBool::new(false),
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
        let result = self.try_wait();

        if let Err(()) = result {
            panic!("Another thread this thread was waiting for panicked and dropped the WaitGroup.");
        }
    }

    /// Wait until all threads have completed. Returns a Result to tell whether a thread panicked.
    ///
    /// This unblocks when the internal count is `0`.
    pub fn try_wait(&self) -> Result<(), ()> {
        {
            let mut count = self.0.count.lock().unwrap();
            while *count > 0 {
                count = self.0.cond.wait(count).unwrap();
            }
        }
        if self.0.poisoned.load(Ordering::Acquire) {
            Err(())
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for WaitGroup {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let count = self.0.count.lock().unwrap();
        write!(f, "WaitGroup {{ count: {:?} }}", *count)
    }
}

#[cfg(test)]
mod tests {
    use std::thread::{spawn, sleep};
    use std::time::{Instant, Duration};
    use super::WaitGroup;

    #[test]
    fn test_wait() {

        let wg1 = WaitGroup::new();
        let wg2 = wg1.clone();
        wg2.add(1);

        let start_time = Instant::now();

        spawn(move || {
            sleep(Duration::from_secs(1));
            wg2.done();
        });

        wg1.wait();

        assert!(Instant::now().duration_since(start_time) > Duration::from_secs(1));

        // This does have some shaky indeterminism,
        // but in *practice* it should never fail.
        assert!(Instant::now().duration_since(start_time) < Duration::from_secs(2));

    }

    #[test]
    #[should_panic]
    fn test_panicking() {

        let wg1 = WaitGroup::new();
        let wg2 = wg1.clone();
        wg2.add(1);

        let start_time = Instant::now();

        spawn(move || {
            sleep(Duration::from_secs(1));
            panic!("Thread panicked and done won't be called!");
            wg2.done();
        });

        wg1.wait();
    }

    #[test]
    fn test_try_wait() {

        let wg1 = WaitGroup::new();
        let wg2 = wg1.clone();
        wg2.add(1);
        let wg3 = wg1.clone();
        wg3.add(1);

        let start_time = Instant::now();

        spawn(move || {
            sleep(Duration::from_secs(1));
            panic!("Sleeping thread panicked and done won't be called!");
            wg2.done();
        });

        spawn(move || {
            panic!("Non-sleeping thread panicked and done won't be called!");
            wg3.done();
        });

        let panicked = wg1.try_wait();

        // Even though the non-sleeping thread panicked right away, try_wait should still
        // wait for all threads to finish (via done() or panic) and only then return the error.

        assert!(Instant::now().duration_since(start_time) > Duration::from_secs(1));

        // This does have some shaky indeterminism,
        // but in *practice* it should never fail.
        assert!(Instant::now().duration_since(start_time) < Duration::from_secs(2));


        assert_eq!(panicked, Err(()));

    }
}
