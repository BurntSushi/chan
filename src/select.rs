use std::cell::RefCell;
use std::collections::hash_map::{HashMap, Entry};
use std::ops::Drop;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};

use rand::{self, Rng};

use {Receiver, Sender};

pub struct Choose<'a, T> {
    cond: Arc<Condvar>,
    cond_mutex: Mutex<()>,
    chans: Vec<(u64, Box<Receiver<Item=T> + 'a>)>,
}

impl<'a, T> Choose<'a, T> {
    pub fn new() -> Choose<'a, T> {
        Choose {
            cond: Arc::new(Condvar::new()),
            cond_mutex: Mutex::new(()),
            chans: vec![],
        }
    }

    pub fn recv<C>(mut self, chan: C) -> Choose<'a, T>
            where C: Receiver<Item=T> + 'a {
        let key = chan.subscribe(self.cond.clone());
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
            match chan.try_recv() {
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

// BREADCRUMBS: Most of the problems with `Select` stem from the fact that
// the `send` method takes ownership of a value. If that `send` is activated,
// then that value is lost and the entire `Select` must be re-constructed
// (which is expensive).
//
// A possible solution to this is to demand that the sent value be cloneable.
// `Select` can take ownership, but clone the value before sending it.
// This doesn't work so hot if cloning is expensive, but we can make the
// argument that the caller needs to be responsible for cheap cloning. In
// particular, they should probably use `Arc`. But we should not demand
// `Arc` because it would be silly to shoehorn its use with `Copy` types
// (or other types that are cheap to clone).

pub struct Select {
    cond: Arc<Condvar>,
    cond_mutex: Mutex<()>,
    choices: HashMap<ChoiceKey, Choice>,
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChoiceKey {
    Sender(u64),
    Receiver(u64),
}

struct Choice {
    subscribe: MaybeSubscribed,
    unsubscribe: Box<Fn(u64) + 'static>,
    try: Box<FnMut() -> bool + 'static>,
}

enum MaybeSubscribed {
    No(Box<Fn() -> u64 + 'static>),
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
    pub fn id(&self) -> ChoiceKey {
        ChoiceKey::Sender(self.chan.id())
    }
}

impl<R: Receiver<Item=T>, T> SelectRecvHandle<R, T> {
    pub fn id(&self) -> ChoiceKey {
        ChoiceKey::Receiver(self.chan.id())
    }

    pub fn into_value(self) -> Option<T> {
        let val = self.val.borrow_mut().take().unwrap();
        val
    }
}

impl Select {
    pub fn new() -> Select {
        Select {
            cond: Arc::new(Condvar::new()),
            cond_mutex: Mutex::new(()),
            choices: HashMap::new(),
        }
    }

    fn is_subscribed(&self) -> bool {
        self.choices.is_empty()
            || self.choices.values().next().unwrap().is_subscribed()
    }

    pub fn select(&mut self, default: bool) -> Option<ChoiceKey> {
        let mut first = true;
        loop {
            if let Some(key) = self.try() {
                return Some(key);
            }
            if first {
                if default {
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
            }
            first = false;
            let _ = self.cond.wait(self.cond_mutex.lock().unwrap()).unwrap();
        }
    }

    fn try(&mut self) -> Option<ChoiceKey> {
        for (&key, choice) in &mut self.choices {
            if (choice.try)() {
                return Some(key);
            }
        }
        None
    }

    pub fn send<S, T>(
        &mut self,
        chan: S,
        val: T,
    ) -> SelectSendHandle<S>
    where S: Sender<Item=T> + Clone + 'static, T: 'static {
        let key = ChoiceKey::Sender(chan.id());
        let mut val = Some(val);
        let chan2 = chan.clone();
        let boxed_try = Box::new(move || {
            let v = match val.take() {
                Some(v) => v,
                None => return false,
            };
            match chan2.try_send(v) {
                Ok(()) => true,
                Err(val2) => { val = Some(val2); false }
            }
        });
        match self.choices.entry(key) {
            Entry::Occupied(mut choice) => {
                choice.get_mut().try = boxed_try;
            }
            Entry::Vacant(spot) => {
                let (chan2, chan3) = (chan.clone(), chan.clone());
                let condvar = self.cond.clone();
                spot.insert(Choice {
                    subscribe: MaybeSubscribed::No(Box::new(move || {
                        chan2.subscribe(condvar.clone())
                    })),
                    unsubscribe: Box::new(move |key| chan3.unsubscribe(key)),
                    try: boxed_try,
                });
            }
        }
        SelectSendHandle { chan: chan }
    }

    pub fn recv<R, T>(
        &mut self,
        chan: R,
    ) -> SelectRecvHandle<R, T>
    where R: Receiver<Item=T> + Clone + 'static, T: 'static {
        let key = ChoiceKey::Receiver(chan.id());
        let recv_val = Rc::new(RefCell::new(None));
        let recv_val2 = recv_val.clone();
        let chan2 = chan.clone();
        let boxed_try = Box::new(move || {
            match chan2.try_recv() {
                Ok(val) => { *recv_val2.borrow_mut() = Some(val); true }
                Err(()) => false,
            }
        });
        match self.choices.entry(key) {
            Entry::Occupied(mut choice) => {
                choice.get_mut().try = boxed_try;
            }
            Entry::Vacant(spot) => {
                let (chan2, chan3) = (chan.clone(), chan.clone());
                let condvar = self.cond.clone();
                spot.insert(Choice {
                    subscribe: MaybeSubscribed::No(Box::new(move || {
                        chan2.subscribe(condvar.clone())
                    })),
                    unsubscribe: Box::new(move |key| chan3.unsubscribe(key)),
                    try: boxed_try,
                });
            }
        }
        SelectRecvHandle { chan: chan, val: recv_val }
    }
}

impl Choice {
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

impl MaybeSubscribed {
    fn subscribe(&self) -> MaybeSubscribed {
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

impl Drop for Select {
    fn drop(&mut self) {
        for (_, choice) in &mut self.choices {
            choice.unsubscribe();
        }
    }
}
