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
    choices: HashMap<ChannelId, Choice<'c>>,
    ids: Option<Vec<ChannelId>>,
}

struct Choice<'c> {
    subscribe: MaybeSubscribed<'c>,
    unsubscribe: Box<Fn(u64) + 'c>,
    try: Box<FnMut() -> bool + 'c>,
}

enum MaybeSubscribed<'c> {
    No(Box<Fn() -> u64 + 'c>),
    Yes(u64),
}

#[derive(Debug)]
pub struct SelectSendHandle<S> {
    chan: S,
}

#[derive(Debug)]
pub struct SelectRecvHandle<R, T> {
    chan: R,
    val: Rc<RefCell<Option<Option<T>>>>,
}

impl<S: Sender> SelectSendHandle<S> {
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }
}

impl<R: Receiver<Item=T>, T> SelectRecvHandle<R, T> {
    pub fn id(&self) -> ChannelId {
        self.chan.id()
    }

    pub fn into_value(self) -> Option<T> {
        let val = self.val.borrow_mut().take().unwrap();
        val
    }
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
            || self.choices.values().next().unwrap().is_subscribed()
    }

    pub fn select(&mut self) -> ChannelId {
        self.maybe_try_select(false).unwrap()
    }

    pub fn try_select(&mut self) -> Option<ChannelId> {
        self.maybe_try_select(true)
    }

    fn maybe_try_select(&mut self, try: bool) -> Option<ChannelId> {
        fn try_sync(
            ids: &mut Option<Vec<ChannelId>>,
            choices: &mut HashMap<ChannelId, Choice>,
        ) -> Option<ChannelId> {
            let mut ids = ids.as_mut().unwrap();
            rand::thread_rng().shuffle(ids);
            for key in ids {
                if (choices.get_mut(key).unwrap().try)() {
                    return Some(*key);
                }
            }
            None
        }

        self.ids = Some(self.choices.keys().cloned().collect());
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
                choice.subscribe();
            }
        }
        let mut cond_lock = self.cond_mutex.lock().unwrap();
        loop {
            if let Some(key) = try_sync(&mut self.ids, &mut self.choices) {
                return Some(key);
            }
            cond_lock = self.cond.wait(cond_lock).unwrap();
        }
    }

    pub fn send<'s: 'c, S, T>(
        &mut self,
        chan: S,
        val: T,
    ) -> SelectSendHandle<S>
    where S: Sender<Item=T> + Clone + 's, T: 'static {
        let select_id = self.id;
        let mut val = Some(val);
        let chan2 = chan.clone();
        let boxed_try = Box::new(move || {
            let v = match val.take() {
                Some(v) => v,
                None => return false,
            };
            match chan2.try_send_from(v, select_id) {
                Ok(()) => true,
                Err(val2) => { val = Some(val2); false }
            }
        });
        match self.choices.entry(chan.id()) {
            Entry::Occupied(mut choice) => {
                choice.get_mut().try = boxed_try;
            }
            Entry::Vacant(spot) => {
                assert!(self.ids.is_none(),
                        "cannot add new channels after initial select");
                let (chan2, chan3) = (chan.clone(), chan.clone());
                let cond_mutex = self.cond_mutex.clone();
                let condvar = self.cond.clone();
                spot.insert(Choice {
                    subscribe: MaybeSubscribed::No(Box::new(move || {
                        chan2.subscribe(
                            select_id, cond_mutex.clone(), condvar.clone())
                    })),
                    unsubscribe: Box::new(move |key| chan3.unsubscribe(key)),
                    try: boxed_try,
                });
            }
        }
        SelectSendHandle { chan: chan }
    }

    pub fn recv<'r: 'c, R, T>(
        &mut self,
        chan: R,
    ) -> SelectRecvHandle<R, T>
    where R: Receiver<Item=T> + Clone + 'r, T: 'static {
        let select_id = self.id;
        let recv_val = Rc::new(RefCell::new(None));
        let recv_val2 = recv_val.clone();
        let chan2 = chan.clone();
        let boxed_try = Box::new(move || {
            match chan2.try_recv_from(select_id) {
                Ok(val) => { *recv_val2.borrow_mut() = Some(val); true }
                Err(()) => false,
            }
        });
        match self.choices.entry(chan.id()) {
            Entry::Occupied(mut choice) => {
                choice.get_mut().try = boxed_try;
            }
            Entry::Vacant(spot) => {
                assert!(self.ids.is_none(),
                        "cannot add new channels after initial select");
                let (chan2, chan3) = (chan.clone(), chan.clone());
                let cond_mutex = self.cond_mutex.clone();
                let condvar = self.cond.clone();
                spot.insert(Choice {
                    subscribe: MaybeSubscribed::No(Box::new(move || {
                        chan2.subscribe(
                            select_id, cond_mutex.clone(), condvar.clone())
                    })),
                    unsubscribe: Box::new(move |key| chan3.unsubscribe(key)),
                    try: boxed_try,
                });
            }
        }
        SelectRecvHandle { chan: chan, val: recv_val }
    }
}

impl<'c> Choice<'c> {
    fn subscribe(&mut self) {
        self.subscribe = self.subscribe.subscribe();
    }

    fn unsubscribe(&self) {
        match self.subscribe {
            MaybeSubscribed::Yes(key) => (self.unsubscribe)(key),
            _ => {}
        }
    }

    fn is_subscribed(&self) -> bool {
        self.subscribe.is_subscribed()
    }
}

impl<'c> MaybeSubscribed<'c> {
    fn subscribe(&self) -> MaybeSubscribed<'c> {
        match *self {
            MaybeSubscribed::No(ref sub) => MaybeSubscribed::Yes(sub()),
            MaybeSubscribed::Yes(key) => MaybeSubscribed::Yes(key),
        }
    }

    fn is_subscribed(&self) -> bool {
        match *self {
            MaybeSubscribed::Yes(_) => true,
            _ => false,
        }
    }
}

impl<'c> Drop for Select<'c> {
    fn drop(&mut self) {
        for (_, choice) in &mut self.choices {
            choice.unsubscribe();
        }
    }
}

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
