/*!
An implementation of a multi-producer, multi-consumer synchronous channel with
a (possible empty) fixed size buffer.
*/

extern crate rand;

use std::sync::{Arc, Condvar};

pub use async::AsyncChannel;
pub use sync::{SyncSender, SyncReceiver, sync_channel};
pub use select::{Choose, Select, ChoiceKey, SelectSendHandle, SelectRecvHandle};
pub use wait_group::WaitGroup;

mod async;
mod notifier;
mod select;
mod sync;
mod tracker;
mod wait_group;

pub trait Channel {
    type Item;

    fn id(&self) -> u64;
    fn subscribe(&self, condvar: Arc<Condvar>) -> u64;
    fn unsubscribe(&self, key: u64);
}

pub trait Sender: Channel {
    fn send(&self, val: Self::Item);
    fn try_send(&self, val: Self::Item) -> Result<(), Self::Item>;
    fn close(&self);
}

pub trait Receiver: Channel {
    fn recv(&self) -> Option<Self::Item>;
    fn try_recv(&self) -> Result<Option<Self::Item>, ()>;
    fn iter(self) -> Iter<Self> where Self: Sized { Iter::new(self) }
}

impl<'a, T: Channel> Channel for &'a T {
    type Item = T::Item;

    fn id(&self) -> u64 { (*self).id() }

    fn subscribe(&self, condvar: Arc<Condvar>) -> u64 {
        (*self).subscribe(condvar)
    }

    fn unsubscribe(&self, key: u64) { (*self).unsubscribe(key) }
}

impl<'a, T: Sender> Sender for &'a T {
    fn send(&self, val: Self::Item) { (*self).send(val); }

    fn try_send(&self, val: Self::Item) -> Result<(), Self::Item> {
        (*self).try_send(val)
    }

    fn close(&self) { (*self).close() }
}

impl<'a, T: Receiver> Receiver for &'a T {
    fn recv(&self) -> Option<Self::Item> { (*self).recv() }

    fn try_recv(&self) -> Result<Option<Self::Item>, ()> { (*self).try_recv() }
}

pub struct Iter<C> {
    chan: C,
}

impl<C: Receiver> Iter<C> {
    pub fn new(c: C) -> Iter<C> { Iter { chan: c } }
}

impl<C: Receiver> Iterator for Iter<C> {
    type Item = C::Item;
    fn next(&mut self) -> Option<C::Item> { self.chan.recv() }
}

#[macro_export]
macro_rules! select_chan {
    ($select:ident, default => $default:expr, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr,
    )+) => {
        select_chan!(
            $select,
            default => $default,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };
    ($select:ident, default => $default:expr, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+) => {{
        let mut sel = &mut $select;
        $(let $chan = sel.$meth($chan.clone() $(, $send)*);)+
        let which = sel.select(true);
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
        select_chan!(
            $select,
            $($chan.$meth($($send)*) $(-> $name)* => $code),+);
    };
    ($select:ident, $(
        $chan:ident.$meth:ident($($send:expr)*)
        $(-> $name:pat)* => $code:expr
    ),+) => {{
        let mut sel = &mut $select;
        $(let $chan = sel.$meth($chan.clone() $(, $send)*);)+
        let which = sel.select(false).unwrap();
        $(if which == $chan.id() {
            $(let $name = $chan.into_value();)*
            $code
        } else)+
        { unreachable!() }
    }};
    ($($tt:tt)*) => {{
        let mut sel = Select::new();
        select_chan!(sel, $($tt)*);
    }};
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::{
        Sender, Receiver,
        Select, Choose, AsyncChannel, WaitGroup,
        sync_channel,
    };

    #[test]
    fn simple() {
        let (send, recv) = sync_channel(1);
        send.send(5);
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_unbuffered() {
        let (send, recv) = sync_channel(0);
        thread::spawn(move || send.send(5));
        assert_eq!(recv.recv(), Some(5));
    }

    #[test]
    fn simple_async() {
        let chan = AsyncChannel::new();
        chan.send(5);
        assert_eq!(chan.recv(), Some(5));
    }

    #[test]
    fn simple_iter() {
        let (send, recv) = sync_channel(1);
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
        let (send, recv) = sync_channel(1);
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
        let chan = AsyncChannel::new();
        let chan2 = chan.clone();
        thread::spawn(move || {
            for i in 0..100 {
                chan2.send(i);
            }
            chan2.close();
        });
        let recvd: Vec<i32> = chan.iter().collect();
        assert_eq!(recvd, (0..100).collect::<Vec<i32>>());
    }

    #[test]
    fn simple_try() {
        let (send, recv) = sync_channel(1);
        send.try_send(5).is_err();
        recv.try_recv().is_err();
    }

    #[test]
    fn simple_try_unbuffered() {
        let (send, recv) = sync_channel(0);
        send.try_send(5).is_err();
        recv.try_recv().is_err();
    }

    #[test]
    fn simple_try_async() {
        let chan = AsyncChannel::new();
        chan.try_recv().is_err();
        chan.try_send(5).is_ok();
    }

    #[test]
    fn select() {
        let (sticka, rticka) = sync_channel(1);
        let (stickb, rtickb) = sync_channel(1);
        let (stickc, rtickc) = sync_channel(1);
        let (send, recv) = sync_channel::<()>(0);
        thread::spawn(move || {
            loop {
                sticka.send("ticka");
                thread::sleep_ms(100);
                recv.recv();
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
            select_chan! {
                rticka.recv() -> val => println!("{:?}", val),
                rtickb.recv() -> val => println!("{:?}", val),
                rtickc.recv() => stop = true,
                send.send(()) => println!("SENT!"),
            }
            if stop {
                break;
            }
        }
        println!("done!");
    }

    #[test]
    fn choose() {
        #[derive(Debug)]
        enum Message { Foo, Bar, Fubar }
        let (s1, r1) = sync_channel(1);
        let (s2, r2) = sync_channel(1);
        let (s3, r3) = sync_channel(1);
        thread::spawn(move || loop {
            thread::sleep_ms(50);
            s1.send(Message::Foo);
        });
        thread::spawn(move || loop {
            thread::sleep_ms(70);
            s2.send(Message::Bar);
        });
        thread::spawn(move || loop {
            thread::sleep_ms(500);
            s3.send(Message::Fubar);
        });
        let mut chooser = Choose::new().recv(r1).recv(r2).recv(r3);
        loop {
            match chooser.choose().unwrap() {
                Message::Foo => println!("foo"),
                Message::Bar => println!("bar"),
                Message::Fubar => break,
            }
        }
        println!("done!");
    }

    #[test]
    fn mpmc() {
        let (send, recv) = sync_channel(1);
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
        println!("done!");
    }
}
