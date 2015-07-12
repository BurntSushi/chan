/*!
An implementation of a multi-producer, multi-consumer synchronous channel with
a (possible empty) fixed size buffer.
*/

extern crate rand;

use std::collections::VecDeque;
use std::fmt;
use std::ops::Drop;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};

use notifier::Notifier;
pub use select::{Select, SelectRecvHandle, SelectSendHandle};
use tracker::Tracker;
pub use wait_group::WaitGroup;

// This enables us to (in practice) uniquely identify any particular channel.
// A better approach would be to use the pointer's address in memory, but it
// looks like `Arc` doesn't support that (yet?).
//
// Any other ideas? ---AG
//
// N.B. This is combined with ChannelId to distinguish between the sending
// and receiving halves of a channel.
static NEXT_CHANNEL_ID: AtomicUsize = ATOMIC_USIZE_INIT;

mod notifier;
mod select;
mod tracker;
mod wait_group;

pub fn sync<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    let send = Channel::new(size, false);
    let recv = send.clone();
    (send.into_sender(), recv.into_receiver())
}

pub fn async<T>() -> (Sender<T>, Receiver<T>) {
    let send = Channel::new(0, true);
    let recv = send.clone();
    (send.into_sender(), recv.into_receiver())
}

#[doc(hidden)]
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChannelId(ChannelKey);

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
enum ChannelKey {
    Sender(u64),
    Receiver(u64),
}

impl ChannelId {
    fn sender(id: u64) -> ChannelId {
        ChannelId(ChannelKey::Sender(id))
    }

    fn receiver(id: u64) -> ChannelId {
        ChannelId(ChannelKey::Receiver(id))
    }
}

pub struct Iter<T> {
    chan: Receiver<T>,
}

impl<T> Iterator for Iter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> { self.chan.recv() }
}

#[derive(Debug)]
pub struct Sender<T>(Channel<T>);

#[derive(Debug)]
pub struct Receiver<T>(Channel<T>);

#[derive(Debug)]
struct Channel<T>(Arc<Inner<T>>);

#[derive(Clone, Copy, Debug)]
enum ChannelType {
    Async,
    Unbuffered,
    Buffered,
}

struct Inner<T> {
    id: u64,
    notify: Notifier,
    track: Tracker,
    cond: Condvar,
    cap: usize,
    ty: ChannelType,
    data: Mutex<Data<T>>,
}

#[derive(Debug)]
struct Data<T> {
    closed: bool,
    waiting_send: usize,
    waiting_recv: usize,
    user: UserData<T>,
}

#[derive(Debug)]
enum UserData<T> {
    One(Option<T>),
    Ring { queue: Vec<Option<T>>, pos: usize, len: usize },
    Queue(VecDeque<T>),
}

struct SendOp<'a, T: 'a> {
    lock: MutexGuard<'a, Data<T>>,
    kind: SendOpKind<T>,
}

#[derive(Debug)]
enum SendOpKind<T> {
    Ok,
    Closed(T),
    WouldBlock(T),
}

struct RecvOp<'a, T: 'a> {
    lock: MutexGuard<'a, Data<T>>,
    kind: RecvOpKind<T>,
}

#[derive(Debug)]
enum RecvOpKind<T> {
    Ok(T),
    Closed,
    WouldBlock,
}

impl<T> Sender<T> {
    pub fn send(&self, val: T) {
        self.send_op(self.inner().lock(), false, val).unwrap()
    }

    fn try_send(&self, val: T) -> Result<(), T> {
        self.send_op(self.inner().lock(), true, val).into_result()
    }

    fn send_op<'a>(
        &'a self,
        data: MutexGuard<'a, Data<T>>,
        try: bool,
        val: T,
    ) -> SendOp<'a, T> {
        match self.inner().ty {
            ChannelType::Async => self.inner().async_send(data, val),
            ChannelType::Unbuffered => {
                self.inner().unbuffered_send(data, try, val)
            }
            ChannelType::Buffered => {
                self.inner().buffered_send(data, try, val)
            }
        }
    }

    fn inner(&self) -> &Inner<T> {
        &(self.0).0
    }

    fn id(&self) -> ChannelId {
        ChannelId::sender(self.inner().id)
    }
}

impl<T> Receiver<T> {
    pub fn recv(&self) -> Option<T> {
        self.recv_op(self.inner().lock(), false).unwrap()
    }

    fn try_recv(&self) -> Result<Option<T>, ()> {
        self.recv_op(self.inner().lock(), true).into_result()
    }

    fn recv_op<'a>(
        &'a self,
        data: MutexGuard<'a, Data<T>>,
        try: bool,
    ) -> RecvOp<'a, T> {
        match self.inner().ty {
            ChannelType::Async => self.inner().async_recv(data, try),
            ChannelType::Unbuffered => self.inner().unbuffered_recv(data, try),
            ChannelType::Buffered => self.inner().buffered_recv(data, try),
        }
    }

    pub fn iter(&self) -> Iter<T> { Iter { chan: self.clone() } }

    fn inner(&self) -> &Inner<T> {
        &(self.0).0
    }

    fn id(&self) -> ChannelId {
        ChannelId::receiver(self.inner().id)
    }
}

impl<T> Channel<T> {
    fn new(size: usize, async: bool) -> Channel<T> {
        let (user, ty) = if async {
            (
                UserData::Queue(VecDeque::with_capacity(1024)),
                ChannelType::Async,
            )
        } else if size == 0 {
            (UserData::One(None), ChannelType::Unbuffered)
        } else {
            let mut queue = Vec::with_capacity(size);
            for _ in 0..size { queue.push(None); }
            (
                UserData::Ring { queue: queue, pos: 0, len: 0 },
                ChannelType::Buffered,
            )
        };
        Channel(Arc::new(Inner {
            id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst) as u64,
            notify: Notifier::new(),
            track: Tracker::new(),
            cond: Condvar::new(),
            cap: size,
            ty: ty,
            data: Mutex::new(Data {
                closed: false,
                waiting_send: 0,
                waiting_recv: 0,
                user: user,
            }),
        }))
    }

    fn into_sender(self) -> Sender<T> {
        self.0.track.add_sender();
        Sender(self)
    }

    fn into_receiver(self) -> Receiver<T> {
        self.0.track.add_receiver();
        Receiver(self)
    }
}

impl<T> Inner<T> {
    fn lock(&self) -> MutexGuard<Data<T>> {
        self.data.lock().unwrap()
    }

    fn close(&self) {
        let mut data = self.lock();
        data.closed = true;
        self.notify();
    }

    fn notify(&self) {
        self.cond.notify_all();
        self.notify.notify();
    }

    fn buffered_send<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
        val: T,
    ) -> SendOp<'a, T> {
        while data.user.len() == self.cap {
            if data.closed {
                return SendOp::closed(data, val);
            }
            if try {
                return SendOp::blocked(data, val);
            }
            data = self.cond.wait(data).unwrap();
        }
        if data.closed {
            return SendOp::closed(data, val);
        }
        data.user.push(val);
        self.notify();
        SendOp::ok(data)
    }

    fn buffered_recv<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
    ) -> RecvOp<'a, T> {
        while data.user.len() == 0 {
            if data.closed {
                return RecvOp::closed(data);
            }
            if try {
                return RecvOp::blocked(data);
            }
            data = self.cond.wait(data).unwrap();
        }
        self.notify();
        let val = data.user.pop();
        RecvOp::ok(data, val)
    }

    fn unbuffered_send<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
        val: T,
    ) -> SendOp<'a, T> {
        while data.waiting_send == 1 || data.user.len() == 1 {
            if try {
                return SendOp::blocked(data, val);
            }
            data = self.cond.wait(data).unwrap();
        }
        if data.closed {
            return SendOp::closed(data, val);
        }
        if try && data.waiting_recv == 0 {
            return SendOp::blocked(data, val);
        }
        data.user.push(val);
        self.notify();
        while data.user.len() == 1 {
            data.waiting_send += 1;
            data = self.cond.wait(data).unwrap();
            data.waiting_send -= 1;
        }
        self.notify();
        SendOp::ok(data)
    }

    fn unbuffered_recv<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
    ) -> RecvOp<'a, T> {
        while data.user.len() == 0 {
            if data.closed {
                return RecvOp::closed(data);
            }
            if try {
                return RecvOp::blocked(data);
            }
            self.notify();
            data.waiting_recv += 1;
            data = self.cond.wait(data).unwrap();
            data.waiting_recv -= 1;
        }
        let val = data.user.pop();
        self.notify();
        RecvOp::ok(data, val)
    }

    fn async_send<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        val: T,
    ) -> SendOp<'a, T> {
        data.user.push(val);
        self.notify();
        SendOp::ok(data)
    }

    fn async_recv<'a>(
        &'a self,
        mut data: MutexGuard<'a, Data<T>>,
        try: bool,
    ) -> RecvOp<'a, T> {
        while data.user.len() == 0 {
            if data.closed {
                return RecvOp::closed(data);
            }
            if try {
                return RecvOp::blocked(data);
            }
            data = self.cond.wait(data).unwrap();
        }
        let val = data.user.pop();
        self.notify();
        RecvOp::ok(data, val)
    }
}

impl<T> UserData<T> {
    fn push(&mut self, val: T) {
        match *self {
            UserData::One(ref mut val_loc) => *val_loc = Some(val),
            UserData::Ring { ref mut queue, pos, ref mut len } => {
                let cap = queue.len();
                assert!(*len < cap);
                queue[(pos + *len) % cap] = Some(val);
                *len += 1;
            }
            UserData::Queue(ref mut deque) => deque.push_back(val),
        }
    }

    fn pop(&mut self) -> T {
        match *self {
            UserData::One(ref mut val) => val.take().unwrap(),
            UserData::Ring { ref mut queue, ref mut pos, ref mut len } => {
                let cap = queue.len();
                assert!(*len <= cap);
                assert!(*len > 0);
                let val = queue[*pos].take().expect("non-null item in queue");
                *pos = (*pos + 1) % cap;
                *len -= 1;
                val
            }
            UserData::Queue(ref mut deque) => deque.pop_front().unwrap(),
        }
    }

    fn len(&self) -> usize {
        match *self {
            UserData::One(ref val) => if val.is_some() { 1 } else { 0 },
            UserData::Ring { len, .. } => len,
            UserData::Queue(ref deque) => deque.len(),
        }
    }
}

impl<'a, T> SendOp<'a, T> {
    fn ok(lock: MutexGuard<'a, Data<T>>) -> SendOp<'a, T> {
        SendOp { lock: lock, kind: SendOpKind::Ok }
    }

    fn closed(lock: MutexGuard<'a, Data<T>>, val: T) -> SendOp<'a, T> {
        SendOp { lock: lock, kind: SendOpKind::Closed(val) }
    }

    fn blocked(lock: MutexGuard<'a, Data<T>>, val: T) -> SendOp<'a, T> {
        SendOp { lock: lock, kind: SendOpKind::WouldBlock(val) }
    }

    fn unwrap(self) {
        self.into_result().ok().unwrap();
    }

    fn into_result(self) -> Result<(), T> {
        self.into_result_lock().1
    }

    fn into_result_lock(self) -> (MutexGuard<'a, Data<T>>, Result<(), T>) {
        match self.kind {
            SendOpKind::Ok => (self.lock, Ok(())),
            SendOpKind::WouldBlock(val) => (self.lock, Err(val)),
            SendOpKind::Closed(_) => {
                // I think this case cannot happen.
                drop(self.lock);
                panic!("cannot send on a closed channel");
            }
        }
    }
}

impl<'a, T> RecvOp<'a, T> {
    fn ok(lock: MutexGuard<'a, Data<T>>, val: T) -> RecvOp<'a, T> {
        RecvOp { lock: lock, kind: RecvOpKind::Ok(val) }
    }

    fn closed(lock: MutexGuard<'a, Data<T>>) -> RecvOp<'a, T> {
        RecvOp { lock: lock, kind: RecvOpKind::Closed }
    }

    fn blocked(lock: MutexGuard<'a, Data<T>>) -> RecvOp<'a, T> {
        RecvOp { lock: lock, kind: RecvOpKind::WouldBlock }
    }

    fn unwrap(self) -> Option<T> {
        self.into_result().ok().unwrap()
    }

    fn into_result(self) -> Result<Option<T>, ()> {
        self.into_result_lock().1
    }

    fn into_result_lock(self)
                       -> (MutexGuard<'a, Data<T>>, Result<Option<T>, ()>) {
        (self.lock, match self.kind {
            RecvOpKind::Ok(val) => Ok(Some(val)),
            RecvOpKind::WouldBlock => Err(()),
            RecvOpKind::Closed => Ok(None),
        })
    }
}

impl<T> Clone for Channel<T> {
    fn clone(&self) -> Channel<T> {
        Channel(self.0.clone())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        self.0.clone().into_sender()
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Receiver<T> {
        self.0.clone().into_receiver()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.inner().track.remove_sender(|| self.inner().close());
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.inner().track.remove_receiver(|| ());
    }
}

impl<T: fmt::Debug> fmt::Debug for Inner<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data = self.data.lock().unwrap();
        try!(writeln!(f, "SyncInner {{"));
        try!(writeln!(f, "    id: {:?},", self.id));
        try!(writeln!(f, "    cap: {:?},", self.cap));
        try!(writeln!(f, "    notify: {:?},", self.notify));
        try!(writeln!(f, "    data: {:?},", &*data));
        write!(f, "}}")
    }
}

#[macro_export]
macro_rules! chan_select {
    ($select:ident, default => $default:expr, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr,
    )+) => {
        chan_select!(
            $select,
            default => $default,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };
    ($select:ident, default => $default:expr, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+) => {{
        let mut sel = &mut $select;
        $(let $chan = sel.$meth(&$chan $(, $send)*);)+
        let which = sel.try_select();
        $(if which == Some($chan.id()) {
            $(let $name = $chan.into_value();)*
            $code
        } else)+
        { $default }
    }};
    ($select:ident, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr,
    )+) => {
        chan_select!(
            $select,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };
    ($select:ident, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+) => {{
        let mut sel = &mut $select;
        $(let $chan = sel.$meth(&$chan $(, $send)*);)+
        let which = sel.select();
        $(if which == $chan.id() {
            $(let $name = $chan.into_value();)*
            $code
        } else)+
        { unreachable!() }
    }};
    (default => $default:expr) => {{ $default }};
    (default => $default:expr,) => {{ $default }};
    ($select:ident, default => $default:expr) => {{ $default }};
    ($select:ident, default => $default:expr,) => {{ $default }};
    ($select:ident) => {{
        let mut sel = &mut $select;
        sel.select(); // blocks forever
    }};
    () => {{
        let mut sel = $crate::Select::new();
        chan_select!(sel);
    }};
    ($($tt:tt)*) => {{
        let mut sel = $crate::Select::new();
        chan_select!(sel, $($tt)*);
    }};
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::{WaitGroup, async, sync};

    #[test]
    fn simple() {
        let (send, recv) = sync(1);
        send.send(5);
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_unbuffered() {
        let (send, recv) = sync(0);
        thread::spawn(move || send.send(5));
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_async() {
        let (send, recv) = async();
        send.send(5);
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_iter() {
        let (send, recv) = sync(1);
        thread::spawn(move || {
            for i in 0..100 {
                send.send(i);
            }
        });
        let recvd: Vec<i32> = recv.iter().collect();
        assert_eq!(recvd, (0..100).collect::<Vec<i32>>());
    }

    #[test]
    fn simple_iter_unbuffered() {
        let (send, recv) = sync(0);
        thread::spawn(move || {
            for i in 0..100 {
                send.send(i);
            }
        });
        let recvd: Vec<i32> = recv.iter().collect();
        assert_eq!(recvd, (0..100).collect::<Vec<i32>>());
    }

    #[test]
    fn simple_iter_async() {
        let (send, recv) = async();
        thread::spawn(move || {
            for i in 0..100 {
                send.send(i);
            }
        });
        let recvd: Vec<i32> = recv.iter().collect();
        assert_eq!(recvd, (0..100).collect::<Vec<i32>>());
    }

    #[test]
    fn simple_try() {
        let (send, recv) = sync(1);
        send.try_send(5).is_err();
        recv.try_recv().is_err();
    }

    #[test]
    fn simple_try_unbuffered() {
        let (send, recv) = sync(0);
        send.try_send(5).is_err();
        recv.try_recv().is_err();
    }

    #[test]
    fn simple_try_async() {
        let (send, recv) = async();
        recv.try_recv().is_err();
        send.try_send(5).is_ok();
    }

    /*
    #[test]
    fn select_manual() {
        let (s1, r1) = sync(1);
        let (s2, r2) = sync(1);
        s1.send(1);
        s2.send(2);

        let mut sel = ::Select::new();
        // let mut sel = &mut select;
        let c1 = sel.recv(&r1);
        let c2 = sel.recv(&r2);
        let which = sel.select();
        if which == c1.id() {
            println!("r1");
        } else if which == c2.id() {
            println!("r2");
        } else {
            unreachable!();
        }
    }
    */

    #[test]
    fn select() {
        let (sticka, rticka) = sync(1);
        let (stickb, rtickb) = sync(1);
        let (stickc, rtickc) = sync(1);
        let (send, recv) = sync(0);
        thread::spawn(move || {
            loop {
                sticka.send("ticka");
                thread::sleep_ms(100);
                println!("RECV: {:?}", recv.recv());
            }
        });
        thread::spawn(move || {
            loop {
                stickb.send("tickb");
                thread::sleep_ms(50);
            }
        });
        thread::spawn(move || {
            thread::sleep_ms(1000);
            stickc.send(());
        });

        loop {
            let mut stop = false;
            chan_select! {
                rticka.recv() -> val => println!("{:?}", val),
                rtickb.recv() -> val => println!("{:?}", val),
                rtickc.recv() => stop = true,
                send.send("fubar".to_owned()) => println!("SENT!"),
            }
            if stop {
                break;
            }
        }
        println!("select done!");
    }

    #[test]
    fn mpmc() {
        let (send, recv) = sync(1);
        for i in 0..4 {
            let send = send.clone();
            thread::spawn(move || {
                for work in vec!['a', 'b', 'c'] {
                    send.send((i, work));
                }
            });
        }
        let wg_done = WaitGroup::new();
        for i in 0..4 {
            wg_done.add(1);
            let wg_done = wg_done.clone();
            let recv = recv.clone();
            thread::spawn(move || {
                for (sent_from, work) in recv.iter() {
                    println!("sent from {} to {}, work: {}",
                             sent_from, i, work);
                }
                println!("worker {} done!", i);
                wg_done.done();
            });
        }
        drop(send);
        wg_done.wait();
        println!("mpmc done!");
    }
}
