#![feature(test)]

#[macro_use]
extern crate chan;
extern crate rand;
extern crate test;

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use chan::{Receiver, Sender};
use rand::Rng;
use test::Bencher;


fn get_data() -> Vec<u8> {
    rand::thread_rng().gen_iter().take(1000).collect()
}

fn get_data_sum<I: IntoIterator<Item=u8>>(xs: I) -> u64 {
    xs.into_iter().fold(0, |sum, x| sum + (x as u64))
}

macro_rules! bench_chan_spsc {
    ($name:ident, $cons:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = Arc::new(get_data());
            let sum = get_data_sum(data.iter().cloned());
            b.bytes = data.len() as u64;
            b.iter(|| {
                let data = data.clone();
                let (s, r) = $cons;
                thread::spawn(move || {
                    for &datum in &*data {
                        let _ = s.send(datum);
                    }
                });
                assert_eq!(sum, get_data_sum(r.iter()));
            });
        }
    }
}

bench_chan_spsc!(spsc_chan_sync_unbuffered, chan::sync(0));
bench_chan_spsc!(spsc_chan_sync_buffered, chan::sync(1));
bench_chan_spsc!(spsc_chan_sync_buffered_all, chan::sync(1000));
bench_chan_spsc!(spsc_chan_async, chan::async());
bench_chan_spsc!(std_spsc_chan_sync_unbuffered, mpsc::sync_channel(0));
bench_chan_spsc!(std_spsc_chan_sync_buffered, mpsc::sync_channel(1));
bench_chan_spsc!(std_spsc_chan_sync_buffered_all, mpsc::sync_channel(1000));
bench_chan_spsc!(std_spsc_chan_async, mpsc::channel());

macro_rules! bench_chan_mpsc {
    ($name:ident, $cons:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = Arc::new(get_data());
            let sum = get_data_sum(data.iter().cloned());
            b.bytes = data.len() as u64;
            b.iter(|| {
                let (s, r) = $cons;
                for i in 0..4 {
                    let data = data.clone();
                    let s = s.clone();
                    let start = i * (data.len() / 4);
                    let end = start + (data.len() / 4);
                    thread::spawn(move || {
                        for &datum in &(&*data)[start..end] {
                            let _ = s.send(datum);
                        }
                    });
                }
                drop(s);
                assert_eq!(sum, get_data_sum(r.iter()));
            });
        }
    }
}

bench_chan_mpsc!(mpsc_chan_sync_unbuffered, chan::sync(0));
bench_chan_mpsc!(mpsc_chan_sync_buffered, chan::sync(1));
bench_chan_mpsc!(mpsc_chan_sync_buffered_all, chan::sync(1000));
bench_chan_mpsc!(mpsc_chan_async, chan::async());
bench_chan_mpsc!(std_mpsc_chan_sync_unbuffered, mpsc::sync_channel(0));
bench_chan_mpsc!(std_mpsc_chan_sync_buffered, mpsc::sync_channel(1));
bench_chan_mpsc!(std_mpsc_chan_sync_buffered_all, mpsc::sync_channel(1000));
bench_chan_mpsc!(std_mpsc_chan_async, mpsc::channel());

macro_rules! bench_select_no_init {
    ($name:ident, $cons:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = Arc::new(get_data());
            let sum = get_data_sum(data.iter().cloned());
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rs = vec![];
                for i in 0..4 {
                    let (s, r) = $cons;
                    rs.push(r);
                    let data = data.clone();
                    let start = i * (data.len() / 4);
                    let end = start + (data.len() / 4);
                    thread::spawn(move || {
                        for &datum in &(&*data)[start..end] {
                            let _ = s.send(datum);
                        }
                    });
                }
                let mut recv = vec![];
                let (r0, r1, r2, r3) = (&rs[0], &rs[1], &rs[2], &rs[3]);
                let mut done = &mut [false, false, false, false];
                loop {
                    select_chan! {
                        r0.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[0] = true;
                            }
                        },
                        r1.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[1] = true;
                            }
                        },
                        r2.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[2] = true;
                            }
                        },
                        r3.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[3] = true;
                            }
                        },
                    }
                    if done.iter().all(|&b| b) {
                        break;
                    }
                }
                assert_eq!(sum, get_data_sum(recv.into_iter()));
            });
        }
    }
}

bench_select_no_init!(select_no_init_chan_sync_unbuffered, chan::sync(0));
bench_select_no_init!(select_no_init_chan_sync_buffered, chan::sync(1));
bench_select_no_init!(select_no_init_chan_sync_buffered_all, chan::sync(1000));
bench_select_no_init!(select_no_init_chan_async, chan::async());

macro_rules! bench_select_with_init {
    ($name:ident, $cons:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = Arc::new(get_data());
            let sum = get_data_sum(data.iter().cloned());
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rs = vec![];
                for i in 0..4 {
                    let (s, r) = $cons;
                    rs.push(r);
                    let data = data.clone();
                    let start = i * (data.len() / 4);
                    let end = start + (data.len() / 4);
                    thread::spawn(move || {
                        for &datum in &(&*data)[start..end] {
                            let _ = s.send(datum);
                        }
                    });
                }
                let mut recv = vec![];
                let (r0, r1, r2, r3) = (&rs[0], &rs[1], &rs[2], &rs[3]);
                let mut done = &mut [false, false, false, false];
                let mut sel = chan::Select::new();
                loop {
                    select_chan! {sel,
                        r0.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[0] = true;
                            }
                        },
                        r1.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[1] = true;
                            }
                        },
                        r2.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[2] = true;
                            }
                        },
                        r3.recv() -> d => {
                            if let Some(d) = d {
                                recv.push(d);
                            } else {
                                done[3] = true;
                            }
                        },
                    }
                    if done.iter().all(|&b| b) {
                        break;
                    }
                }
                assert_eq!(sum, get_data_sum(recv.into_iter()));
            });
        }
    }
}

bench_select_with_init!(select_with_init_chan_sync_unbuffered, chan::sync(0));
bench_select_with_init!(select_with_init_chan_sync_buffered, chan::sync(1));
bench_select_with_init!(select_with_init_chan_sync_buffered_all, chan::sync(1000));
bench_select_with_init!(select_with_init_chan_async, chan::async());
