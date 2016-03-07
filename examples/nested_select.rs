// This example demonstrates how to use nested selects.
//
// The `select!` macro is not very good, which means you need to name the
// channels differently in nested invocations (blech).
//
// The nested select here is used to "give priority" to the s1/r1 channel.
// If you run this program, you should see lots of receives on r1 near
// the beginning with few receives on r2. Once r1 is exhausted, r2 is finally
// allowed to consume data.

#[macro_use]
extern crate chan;

use std::thread;
use std::time::Duration;

fn main() {
    let (s1, r1) = chan::sync(0);
    let (s2, r2) = chan::sync(0);
    let (s1, s2) = (s1.clone(), s2.clone());
    let wg = chan::WaitGroup::new();
    wg.add(2);
    let (wg1, wg2) = (wg.clone(), wg.clone());
    thread::spawn(move || {
        for i in 0..10 {
            s1.send(i);
            thread::sleep(Duration::from_millis(10));
        }
        wg1.done();
    });
    thread::spawn(move || {
        for i in 0..10 {
            s2.send(i * 5);
            thread::sleep(Duration::from_millis(100));
        }
        wg2.done();
    });
    let done = {
        let (s, r) = chan::sync(0);
        thread::spawn(move || {
            wg.wait();
            s.send(());
        });
        r
    };
    loop {
        let (r1b, r2b) = (&r1, &r2);
        chan_select! {
            default => {
                chan_select! {
                    r1b.recv() -> v1 => match v1 {
                        None => {}
                        Some(v1) => println!("r1: {:?}", v1),
                    },
                    r2b.recv() -> v2 => match v2 {
                        None => {}
                        Some(v2) => println!("r2: {:?}", v2),
                    },
                    done.recv() => break,
                }
            },
            r1.recv() -> v1 => match v1 {
                None => {}
                Some(v1) => println!("r1: {:?}", v1),
            },
        }
    }
}
