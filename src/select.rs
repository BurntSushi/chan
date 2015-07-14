use std::cell::RefCell;
use std::collections::hash_map::{HashMap, Entry};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use rand::{self, Rng};

use {ChannelId, Receiver, Sender, Data};

#[doc(hidden)]
pub struct Select<'c> {
    cond: Arc<Condvar>,
    cond_mutex: Arc<Mutex<()>>,
    choices: HashMap<ChannelId, Box<Choice + 'c>>,
    ids: Option<Vec<ChannelId>>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct SelectSendHandle<'s, T: 's> {
    chan: &'s Sender<T>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct SelectRecvHandle<'r, T: 'r> {
    chan: &'r Receiver<T>,
    val: Rc<RefCell<Option<Option<T>>>>,
}

trait Choice {
    fn subscribe(&self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64;
    fn unsubscribe(&self);
    fn subscription(&self) -> Option<u64>;
    fn try(&mut self) -> bool;
    fn lock(&mut self);
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

    pub fn select(&mut self) -> ChannelId {
        self.maybe_try_select(false).unwrap()
    }

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
            for (_, choice) in &self.choices {
                choice.subscribe(self.cond_mutex.clone(), self.cond.clone());
            }
        }

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
    fn subscribe(&self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        self.chan.inner().notify.subscribe(mutex, condvar)
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
    fn subscribe(&self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        self.chan.inner().notify.subscribe(mutex, condvar)
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
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }
}

impl<'r, T> SelectRecvHandle<'r, T> {
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }

    pub fn into_value(self) -> Option<T> {
        let val = self.val.borrow_mut().take().unwrap();
        val
    }
}
