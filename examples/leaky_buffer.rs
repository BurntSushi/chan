// From http://golang.org/doc/effective_go.html#leaky_buffer

#[macro_use]
extern crate chan;

use std::thread;

use chan::{Receiver, Sender, SyncReceiver, SyncSender};

type Buffer = Vec<u8>;

fn main() {
    let (free_send, free_recv) = chan::sync(100);
    let (serv_send, serv_recv) = chan::sync(0);

    thread::spawn(move || client(free_recv, serv_send));
    thread::spawn(move || server(free_send, serv_recv));
    thread::sleep_ms(500);
}

fn client(free_list: SyncReceiver<Buffer>, server_chan: SyncSender<Buffer>) {
    loop {
        let buf;
        select_chan! {
            default => buf = Vec::with_capacity(1024),
            free_list.recv() -> b => buf = b.unwrap(),
        }
        server_chan.send(buf);
    }
}

fn server(free_list: SyncSender<Buffer>, server_chan: SyncReceiver<Buffer>) {
    loop {
        let buf = server_chan.recv().unwrap();
        select_chan! {
            default => {},
            free_list.send(buf) => {},
        }
    }
}
