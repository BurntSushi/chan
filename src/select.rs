use std::cell::RefCell;
use std::collections::hash_map::{HashMap, Entry};
use std::ops::Drop;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};

use rand::{self, Rng};

static NEXT_SELECT_ID: AtomicUsize = ATOMIC_USIZE_INIT;
static NEXT_CHOOSE_ID: AtomicUsize = ATOMIC_USIZE_INIT;

use {Receiver, Sender, ChannelId};

pub struct Select<'c> {
    id: u64,
    cond: Arc<Condvar>,
    cond_mutex: Arc<Mutex<()>>,
    choices: HashMap<ChannelId, Box<Choice + 'c>>,
    ids: Option<Vec<ChannelId>>,
}

#[derive(Debug)]
pub struct SelectSendHandle<'s, S: 's> {
    chan: &'s S,
}

#[derive(Debug)]
pub struct SelectRecvHandle<'r, R: 'r, T> {
    chan: &'r R,
    val: Rc<RefCell<Option<Option<T>>>>,
}

trait Choice {
    fn subscribe(&self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64;
    fn unsubscribe(&self);
    fn subscription(&self) -> Option<u64>;
    fn try(&self) -> bool;
    fn lock<'a>(&'a self) -> Box<FnMut() + 'a>;
}

struct SendChoice<'s, S: 's, T> {
    chan: &'s S,
    select_id: u64,
    id: Option<u64>,
    val: RefCell<Option<T>>,
}

struct RecvChoice<'r, R: 'r, T> {
    chan: &'r R,
    select_id: u64,
    id: Option<u64>,
    val: Rc<RefCell<Option<Option<T>>>>,
}

impl<'c> Select<'c> {
    pub fn new() -> Select<'c> {
        Select {
            id: NEXT_SELECT_ID.fetch_add(1, Ordering::SeqCst) as u64,
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

    fn maybe_try_select(&mut self, try: bool) -> Option<ChannelId> {
        fn try_sync<'c>(
            ids: &mut Option<Vec<ChannelId>>,
            choices: &HashMap<ChannelId, Box<Choice + 'c>>,
        ) -> Option<ChannelId> {
            let mut ids = ids.as_mut().unwrap();
            rand::thread_rng().shuffle(ids);
            for key in ids {
                if choices.get(key).unwrap().try() {
                    return Some(*key);
                }
            }
            None
        }

        self.ids = Some(self.choices.keys().cloned().collect());
        if let Some(key) = try_sync(&mut self.ids, &self.choices) {
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

        let mut chan_locks = Vec::with_capacity(self.choices.len());
        for (_, choice) in &self.choices {
            chan_locks.push(choice.lock());
        }
        let mut cond_lock = self.cond_mutex.lock().unwrap();
        loop {
            if let Some(key) = try_sync(&mut self.ids, &self.choices) {
                return Some(key);
            }
            for mut unlock in chan_locks.into_iter().rev() {
                unlock();
            }
            cond_lock = self.cond.wait(cond_lock).unwrap();
            chan_locks = Vec::with_capacity(self.choices.len());
            for (_, choice) in &self.choices {
                chan_locks.push(choice.lock());
            }
        }
    }

    pub fn send<'s: 'c, S, T>(
        &mut self,
        chan: &'s S,
        val: T,
    ) -> SelectSendHandle<'s, S>
    where S: Sender<Item=T> + Clone + 's, T: 'static {
        let mut choice = SendChoice {
            chan: chan,
            select_id: self.id,
            id: None,
            val: RefCell::new(Some(val)),
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

    pub fn recv<'r: 'c, R, T>(
        &mut self,
        chan: &'r R,
    ) -> SelectRecvHandle<'r, R, T>
    where R: Receiver<Item=T> + Clone + 'r, T: 'static {
        let mut choice = RecvChoice {
            chan: chan,
            select_id: self.id,
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

impl<'s, S: Sender<Item=T>, T> Choice for SendChoice<'s, S, T> {
    fn subscribe(&self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        self.chan.subscribe(self.select_id, mutex, condvar)
    }

    fn unsubscribe(&self) {
        match self.id {
            Some(id) => self.chan.unsubscribe(id),
            None => {}
        }
    }

    fn subscription(&self) -> Option<u64> {
        self.id
    }

    fn try(&self) -> bool {
        let v = match self.val.borrow_mut().take() {
            Some(v) => v,
            None => return false,
        };
        match self.chan.try_send_from(v, self.select_id) {
            Ok(()) => true,
            Err(v) => { *self.val.borrow_mut() = Some(v); false }
        }
    }

    fn lock<'a>(&'a self) -> Box<FnMut() + 'a> {
        self.chan.lock()
    }
}

impl<'r, R: Receiver<Item=T>, T> Choice for RecvChoice<'r, R, T> {
    fn subscribe(&self, mutex: Arc<Mutex<()>>, condvar: Arc<Condvar>) -> u64 {
        self.chan.subscribe(self.select_id, mutex, condvar)
    }

    fn unsubscribe(&self) {
        match self.id {
            Some(id) => self.chan.unsubscribe(id),
            None => {}
        }
    }

    fn subscription(&self) -> Option<u64> {
        self.id
    }

    fn try(&self) -> bool {
        match self.chan.try_recv_from(self.select_id) {
            Ok(v) => { *self.val.borrow_mut() = Some(v); true }
            Err(()) => false,
        }
    }

    fn lock<'a>(&'a self) -> Box<FnMut() + 'a> {
        self.chan.lock()
    }
}

impl<'s, S: Sender> SelectSendHandle<'s, S> {
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }
}

impl<'r, R: Receiver<Item=T>, T> SelectRecvHandle<'r, R, T> {
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }

    pub fn into_value(self) -> Option<T> {
        let val = self.val.borrow_mut().take().unwrap();
        val
    }
}

/*
pub struct Choose<'a, T> {
    id: u64,
    cond: Arc<Condvar>,
    cond_mutex: Arc<Mutex<()>>,
    chans: Vec<(u64, Box<Receiver<Item=T> + 'a>)>,
}

impl<'a, T> Choose<'a, T> {
    pub fn new() -> Choose<'a, T> {
        Choose {
            id: NEXT_CHOOSE_ID.fetch_add(1, Ordering::SeqCst) as u64,
            cond: Arc::new(Condvar::new()),
            cond_mutex: Arc::new(Mutex::new(())),
            chans: vec![],
        }
    }

    pub fn recv<C>(mut self, chan: C) -> Choose<'a, T>
            where C: Receiver<Item=T> + 'a {
        let key = chan.subscribe(
            self.id, self.cond_mutex.clone(), self.cond.clone());
        self.chans.push((key, Box::new(chan)));
        self
    }

    pub fn choose(&mut self) -> Option<T> {
        loop {
            match self.try_choose() {
                Ok(v) => return v,
                Err(()) => {}
            }
            let _ = self.cond.wait(self.cond_mutex.lock().unwrap()).unwrap();
        }
    }

    pub fn try_choose(&mut self) -> Result<Option<T>, ()> {
        let mut rng = rand::thread_rng();
        rng.shuffle(&mut self.chans);
        for &(_, ref chan) in &self.chans {
            match chan.try_recv_from(self.id) {
                Ok(v) => return Ok(v),
                Err(()) => continue,
            }
        }
        Err(())
    }
}

impl<'a, T> Drop for Choose<'a, T> {
    fn drop(&mut self) {
        for &(key, ref chan) in &self.chans {
            chan.unsubscribe(key);
        }
    }
}
*/
