use std::collections::hash_map::{HashMap, Entry};
use std::ops::Drop;
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

pub struct Select<'s> {
    cond: Arc<Condvar>,
    cond_mutex: Mutex<()>,
    choices: Vec<Choice<'s>>,
    senders: HashMap<u64, usize>,
    receivers: HashMap<u64, usize>,
}

struct Choice<'s> {
    subscribe: MaybeSubscribed<'s>,
    unsubscribe: Box<Fn(u64) + 's>,
}

enum MaybeSubscribed<'s> {
    No(Box<Fn() -> u64 + 's>),
    Yes(u64),
}

pub struct SelectHandle<'r, 's: 'r, 'run> {
    select: &'r mut Select<'s>,
    runs: HashMap<usize, Box<FnMut() -> bool + 'run>>,
    default: Option<Box<FnMut() + 'run>>,
}

impl<'s> Select<'s> {
    pub fn new() -> Select<'s> {
        Select {
            cond: Arc::new(Condvar::new()),
            cond_mutex: Mutex::new(()),
            choices: vec![],
            senders: HashMap::new(),
            receivers: HashMap::new(),
        }
    }

    fn is_subscribed(&self) -> bool {
        self.choices.is_empty() || self.choices[0].is_subscribed()
    }

    pub fn handle<'r, 'run>(&'r mut self) -> SelectHandle<'r, 's, 'run> {
        let runs = HashMap::with_capacity(self.choices.len());
        SelectHandle {
            select: self,
            runs: runs,
            default: None,
        }
    }
}

impl<'r, 's, 'run> SelectHandle<'r, 's, 'run> {
    pub fn select(mut self) {
        let mut first = true;
        while !self.try() {
            if first {
                match self.default {
                    Some(ref mut default) => { default(); return; }
                    _ => {}
                }
                // At this point, we've tried to pick one of the
                // synchronization events without initiating a subscription,
                // but nothing succeeded. Before we sit and wait, we need to
                // tell all of our channels to notify us when something
                // changes.
                if !self.select.is_subscribed() {
                    for choice in &mut self.select.choices {
                        choice.subscribe();
                    }
                }
            }
            first = false;
            let _ = self.select.cond.wait(
                self.select.cond_mutex.lock().unwrap()).unwrap();
        }
    }

    fn try(&mut self) -> bool {
        for (i, _) in self.select.choices.iter_mut().enumerate() {
            if let Some(run) = self.runs.get_mut(&i) {
                if run() {
                    return true;
                }
            }
        }
        false
    }

    pub fn default<F>(
        mut self,
        run: F,
    ) -> SelectHandle<'r, 's, 'run> where F: FnMut() + 'run {
        self.default = Some(Box::new(run));
        self
    }

    pub fn send<'c: 's + 'run, C, T, F>(
        mut self,
        chan: C,
        val: T,
        mut run: F,
    ) -> SelectHandle<'r, 's, 'run>
    where C: Sender<Item=T> + Clone + 'c,
          T: 'run,
          F: FnMut() + 'run {
        let mut val = Some(val);
        let chan2 = chan.clone();
        let boxed_run = Box::new(move || {
            let v = match val.take() {
                Some(v) => v,
                None => return false,
            };
            match chan.try_send(v) {
                Ok(()) => { run(); true }
                Err(val2) => { val = Some(val2); false }
            }
        });
        match self.select.senders.entry(chan2.id()) {
            Entry::Occupied(idx) => {
                self.runs.insert(*idx.get(), boxed_run);
            }
            Entry::Vacant(spot) => {
                let chan3 = chan2.clone();
                let condvar = self.select.cond.clone();
                spot.insert(self.select.choices.len());
                self.runs.insert(self.select.choices.len(), boxed_run);
                self.select.choices.push(Choice {
                    subscribe: MaybeSubscribed::No(Box::new(move || {
                        chan2.subscribe(condvar.clone())
                    })),
                    unsubscribe: Box::new(move |key| chan3.unsubscribe(key)),
                });
            }
        }
        self
    }

    pub fn recv<'c: 's + 'run, C, T, F>(
        mut self,
        chan: C,
        mut run: F,
    ) -> SelectHandle<'r, 's, 'run>
    where C: Receiver<Item=T> + Clone + 'c,
          F: FnMut(Option<T>) + 'run {
        let chan2 = chan.clone();
        let boxed_run = Box::new(move || {
            match chan.try_recv() {
                Ok(val) => { run(val); true }
                Err(()) => false,
            }
        });
        match self.select.receivers.entry(chan2.id()) {
            Entry::Occupied(idx) => {
                self.runs.insert(*idx.get(), boxed_run);
            }
            Entry::Vacant(spot) => {
                let chan3 = chan2.clone();
                let condvar = self.select.cond.clone();
                spot.insert(self.select.choices.len());
                self.runs.insert(self.select.choices.len(), boxed_run);
                self.select.choices.push(Choice {
                    subscribe: MaybeSubscribed::No(Box::new(move || {
                        chan2.subscribe(condvar.clone())
                    })),
                    unsubscribe: Box::new(move |key| chan3.unsubscribe(key)),
                });
            }
        }
        self
    }
}

impl<'a> Choice<'a> {
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

impl<'a> MaybeSubscribed<'a> {
    fn subscribe(&self) -> MaybeSubscribed<'a> {
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

impl<'a> Drop for Select<'a> {
    fn drop(&mut self) {
        for choice in &mut self.choices {
            choice.unsubscribe();
        }
    }
}
