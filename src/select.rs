use std::cell::RefCell;
use std::collections::hash_map::{HashMap, Entry};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use rand::{self, Rng};

use {ChannelId, Receiver, Sender, Data};

/// Select encapsulates synchronization on at most one channel operation.
///
/// This type was *specifically built* to work inside a macro like
/// `chan_select!`. As a result, it is extremely unergonomic to use manually
/// without the macro (see the `select_manual` test). Therefore, it is hidden
/// from the public API. It is, in every sense of the word, an implementation
/// detail.
///
/// If we want to expose a select interface not built on a macro then it
/// probably shouldn't be this one. However, doing it in a way that is both
/// ergonomic and hetergeneous is probably tricky.
#[doc(hidden)]
pub struct Select<'c> {
    /// A condition variable that is notified when one of the participating
    /// channels has a synchronization event.
    cond: Arc<Condvar>,
    /// A mutex for guarding notification from channels.
    cond_mutex: Arc<Mutex<()>>,
    /// The set of all arms.
    choices: HashMap<ChannelId, Box<Choice + 'c>>,
    /// Scratch space for quickly shuffling the order in which we try to
    /// synchronize an operation in select.
    ids: Option<Vec<ChannelId>>,
}

/// The result of adding a send operation to `Select`.
///
/// This exposes a uniform interface with `SelectRecvHandle`. Namely, the id
/// of the underlying channel can be accessed.
#[doc(hidden)]
#[derive(Debug)]
pub struct SelectSendHandle<'s, T: 's> {
    chan: &'s Sender<T>,
}

/// The result of adding a recv operation to `Select`.
///
/// This exposes a uniform interface with `SelectRecvHandle`. Namely, the id
/// of the underlying channel can be accessed.
///
/// This also stores a reference to the value that has been received. The
/// value is shared with a `Choice` in `Select`'s map of subscribed channels.
#[doc(hidden)]
#[derive(Debug)]
pub struct SelectRecvHandle<'r, T: 'r> {
    chan: &'r Receiver<T>,
    val: Rc<RefCell<Option<Option<T>>>>,
}

/// Expose a uniform interface over send and receive operations.
///
/// In particular, this trait is used to *erase* the type parameter in the
/// `Receiver` and `Sender` types. This is essential to building a hetergeneous
/// select.
trait Choice {
    /// Subscribe the owning `Select` to the channel in this choice.
    fn subscribe(&mut self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>);
    /// Unsubscribe the owning `Select` from the channel in this choice.
    fn unsubscribe(&self);
    /// Return the subscription identifier.
    fn subscription(&self) -> Option<u64>;
    /// Attempt to synchronize.
    ///
    /// Returns `true` if and only if the operation synchronized.
    fn try(&mut self) -> bool;
    /// Lock the underlying channel.
    fn lock(&mut self);
    /// Unlock the underlying channel.
    fn unlock(&mut self);
}

struct SendChoice<'s, T: 's> {
    chan: &'s Sender<T>,
    guard: Option<MutexGuard<'s, Data<T>>>,
    id: Option<u64>,
    val: Option<T>,
}

struct RecvChoice<'r, T: 'r> {
    chan: &'r Receiver<T>,
    guard: Option<MutexGuard<'r, Data<T>>>,
    id: Option<u64>,
    val: Rc<RefCell<Option<Option<T>>>>,
}

impl<'c> Select<'c> {
    /// Create a new `Select`.
    pub fn new() -> Select<'c> {
        Select {
            cond: Arc::new(Condvar::new()),
            cond_mutex: Arc::new(Mutex::new(())),
            choices: HashMap::new(),
            ids: None,
        }
    }

    fn is_subscribed(&self) -> bool {
        self.choices.is_empty()
            || self.choices.values().next().unwrap().subscription().is_some()
    }

    /// Perform a select. Block until exactly one channel operation
    /// synchronizes.
    pub fn select(&mut self) -> ChannelId {
        self.maybe_try_select(false).unwrap()
    }

    /// Perform a select. If all channel operations are blocked, return `None`.
    /// (N.B. This will *never* subscribe to channels.)
    pub fn try_select(&mut self) -> Option<ChannelId> {
        self.maybe_try_select(true)
    }

    #[allow(unused_assignments)]
    fn maybe_try_select(&mut self, try: bool) -> Option<ChannelId> {
        fn try_sync<'c>(
            ids: &mut Option<Vec<ChannelId>>,
            choices: &mut HashMap<ChannelId, Box<Choice + 'c>>,
        ) -> Option<ChannelId> {
            let mut ids = ids.as_mut().unwrap();
            rand::thread_rng().shuffle(ids);
            for key in ids {
                if choices.get_mut(key).unwrap().try() {
                    return Some(*key);
                }
            }
            None
        }

        // This is our initial try. If `try` is `true` (i.e., there is a
        // `default` arm), then we quit if the initial try fails.
        if self.ids.is_none() {
            self.ids = Some(self.choices.keys().cloned().collect());
        }
        if let Some(key) = try_sync(&mut self.ids, &mut self.choices) {
            return Some(key);
        }
        if try {
            return None;
        }
        // At this point, we've tried to pick one of the
        // synchronization events without initiating a subscription,
        // but nothing succeeded. Before we sit and wait, we need to
        // tell all of our channels to notify us when something
        // changes.
        if !self.is_subscribed() {
            for (_, choice) in &mut self.choices {
                choice.subscribe(self.cond_mutex.clone(), self.cond.clone());
            }
        }

        // This is an extremely delicate dance.
        //
        // The key is that before we do anything, we need to make sure all
        // involved channels are synchronized. We achieve this by acquiring
        // the mutex inside of each channel.
        //
        // Once that's done, we can attempt to synchronize at most one of the
        // channel operations. If we succeed, then we unlock all of the
        // channels and return the id of the choice that synchronized.
        //
        // If we don't succeed, then we need to block and wait for something
        // to happen until we try again. Doing this is tricky.
        //
        // The key is that when a channel synchronizes, it will notify any
        // subscribed `Select` instances because a synchronization means
        // something has changed and `Select` should try to synchronize one
        // of its channel operations.
        //
        // However, it is critical that a channel only notify `Select` when
        // `Select` is ready to receive notifications. i.e., we must
        // synchronize the channel's notification with `Select`'s wait on
        // the condition variable.
        //
        // Therefore, after we've tried synchronization and failed, we need
        // to lock `Select`'s mutex *before unlocking the channels*. This
        // will prevent channels from trying to notify `Select` before it is
        // ready to receive a notification. (If it misses a notification then
        // we risk deadlock.)
        //
        // Once `Select` is locked, we unlock all of the channels, unlock
        // the `Select` and block. This guarantees that notifications to this
        // `Select` are synchronized with waiting on the condition variable.
        //
        // Once the condition variable is notified, we immediately lock the
        // channels and repeat the process.
        for (_, choice) in &mut self.choices {
            choice.lock();
        }
        let mut cond_lock;
        loop {
            if let Some(key) = try_sync(&mut self.ids, &mut self.choices) {
                for (_, choice) in &mut self.choices {
                    choice.unlock();
                }
                return Some(key);
            }
            cond_lock = self.cond_mutex.lock().unwrap();
            for (_, choice) in &mut self.choices {
                choice.unlock();
            }
            drop(self.cond.wait(cond_lock).unwrap());
            for (_, choice) in &mut self.choices {
                choice.lock();
            }
        }
    }

    /// Register a new send operation with the select.
    pub fn send<'s: 'c, T: 'static>(
        &mut self,
        chan: &'s Sender<T>,
        val: T,
    ) -> SelectSendHandle<'s, T> {
        let mut choice = SendChoice {
            chan: chan,
            guard: None,
            id: None,
            val: Some(val),
        };
        match self.choices.entry(chan.id()) {
            Entry::Occupied(mut prev_choice) => {
                choice.id = prev_choice.get().subscription();
                *prev_choice.get_mut() = Box::new(choice);
            }
            Entry::Vacant(spot) => {
                assert!(self.ids.is_none(),
                        "cannot add new channels after initial select");
                spot.insert(Box::new(choice));
            }
        }
        SelectSendHandle { chan: chan }
    }

    /// Register a new receive operation with the select.
    pub fn recv<'r: 'c, T: 'static>(
        &mut self,
        chan: &'r Receiver<T>,
    ) -> SelectRecvHandle<'r, T> {
        let mut choice = RecvChoice {
            chan: chan,
            guard: None,
            id: None,
            val: Rc::new(RefCell::new(None)),
        };
        let handle_val_loc = choice.val.clone();
        match self.choices.entry(chan.id()) {
            Entry::Occupied(mut prev_choice) => {
                choice.id = prev_choice.get().subscription();
                *prev_choice.get_mut() = Box::new(choice);
            }
            Entry::Vacant(spot) => {
                assert!(self.ids.is_none(),
                        "cannot add new channels after initial select");
                spot.insert(Box::new(choice));
            }
        }
        SelectRecvHandle { chan: chan, val: handle_val_loc }
    }
}

impl<'c> Drop for Select<'c> {
    fn drop(&mut self) {
        for (_, choice) in &mut self.choices {
            choice.unsubscribe();
        }
    }
}

impl<'s, T> Choice for SendChoice<'s, T> {
    fn subscribe(&mut self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) {
        self.id = Some(self.chan.inner().notify.subscribe(mutex, condvar));
    }

    fn unsubscribe(&self) {
        match self.id {
            Some(id) => self.chan.inner().notify.unsubscribe(id),
            None => {}
        }
    }

    fn subscription(&self) -> Option<u64> {
        self.id
    }

    fn try(&mut self) -> bool {
        let v = match self.val.take() {
            Some(v) => v,
            None => return false,
        };
        // If this choice has been locked, then we need to use that lock
        // when trying to synchronize. We must also be careful to put the
        // lock back where it was. Dropping it would be disastrous!
        let try = match self.guard.take() {
            None => self.chan.try_send(v),
            Some(g) => {
                let op = self.chan.send_op(g, true, v);
                let (lock, result) = op.into_result_lock();
                self.guard = Some(lock);
                result
            }
        };
        match try {
            Ok(()) => true,
            Err(v) => { self.val = Some(v); false }
        }
    }

    fn lock(&mut self) {
        self.guard = Some(self.chan.inner().lock());
    }

    fn unlock(&mut self) {
        self.guard.take();
    }
}

impl<'r, T> Choice for RecvChoice<'r, T> {
    fn subscribe(&mut self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) {
        self.id = Some(self.chan.inner().notify.subscribe(mutex, condvar));
    }

    fn unsubscribe(&self) {
        println!("unsubscribing recv: {:?}", self.id);
        match self.id {
            Some(id) => self.chan.inner().notify.unsubscribe(id),
            None => {}
        }
    }

    fn subscription(&self) -> Option<u64> {
        self.id
    }

    fn try(&mut self) -> bool {
        // If this choice has been locked, then we need to use that lock
        // when trying to synchronize. We must also be careful to put the
        // lock back where it was. Dropping it would be disastrous!
        let try = match self.guard.take() {
            None => self.chan.try_recv(),
            Some(g) => {
                let op = self.chan.recv_op(g, true);
                let (lock, result) = op.into_result_lock();
                self.guard = Some(lock);
                result
            }
        };
        match try {
            Ok(v) => { *self.val.borrow_mut() = Some(v); true }
            Err(()) => false,
        }
    }

    fn lock(&mut self) {
        self.guard = Some(self.chan.inner().lock());
    }

    fn unlock(&mut self) {
        self.guard.take();
    }
}

impl<'s, T> SelectSendHandle<'s, T> {
    /// Return the id of the underlying channel.
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }
}

impl<'r, T> SelectRecvHandle<'r, T> {
    /// Return the id of the underlying channel.
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }

    /// Return the retrieved value.
    ///
    /// Panics if this channel was not chosen to synchronize.
    pub fn into_value(self) -> Option<T> {
        let val = self.val.borrow_mut().take().unwrap();
        val
    }
}
