// From http://golang.org/doc/effective_go.html#leaky_buffer

#[macro_use]
extern crate chan;

use std::thread;
use std::time::Duration;

use chan::{Receiver, Sender};

type Buffer = Vec<u8>;

fn main() {
    let (free_send, free_recv) = chan::sync(100);
    let (serv_send, serv_recv) = chan::sync(0);

    thread::spawn(move || client(free_recv, serv_send));
    thread::spawn(move || server(free_send, serv_recv));
    thread::sleep(Duration::from_millis(500));
}

fn client(free_list: Receiver<Buffer>, server_chan: Sender<Buffer>) {
    loop {
        let buf;
        chan_select! {
            default => buf = Vec::with_capacity(1024),
            free_list.recv() -> b => buf = b.unwrap(),
        }
        server_chan.send(buf);
    }
}

fn server(free_list: Sender<Buffer>, server_chan: Receiver<Buffer>) {
    loop {
        let buf = server_chan.recv().unwrap();
        chan_select! {
            default => {},
            free_list.send(buf) => {},
        }
    }
}
