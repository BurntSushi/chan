extern crate chan;
extern crate hyper;

use std::env::args;
use std::error::Error;
use std::io::Read;
use std::thread;

use chan::{Receiver, Sender};
use hyper::client::Client;

fn main() {
    let page_links = vec![
        "http://doc.rust-lang.org/std/sync/atomic/struct.AtomicUsize.html",
        "http://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html",
        "http://doc.rust-lang.org/std/sync/struct.Arc.html",
        "http://doc.rust-lang.org/std/sync/struct.Condvar.html",
        "http://doc.rust-lang.org/std/sync/type.LockResult.html",
        "http://doc.rust-lang.org/std/",
        "http://fooledya.404",
        "http://doc.rust-lang.org/std/macro.select!.html",
        "http://doc.rust-lang.org/std/macro.assert!.html",
        "http://doc.rust-lang.org/std/macro.assert_eq!.html",
        "http://doc.rust-lang.org/std/macro.cfg!.html",
        "http://doc.rust-lang.org/std/macro.column!.html",
        "http://doc.rust-lang.org/std/macro.concat!.html",
        "http://doc.rust-lang.org/std/macro.concat_idents!.html",
        "http://doc.rust-lang.org/std/macro.debug_assert!.html",
        "http://burntsushi.net/stuff/dne.html",
        "http://doc.rust-lang.org/std/macro.debug_assert_eq!.html",
        "http://doc.rust-lang.org/std/macro.env!.html",
        "http://doc.rust-lang.org/std/macro.file!.html",
    ];
    let rpages = {
        let (slinks, rlinks) = chan::sync_channel(0);
        let (spages, rpages) = chan::sync_channel(0);
        thread::spawn(move || {
            for link in page_links {
                slinks.send(link);
            }
        });
        for _ in 0..args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(4) {
            let rlinks = rlinks.clone();
            let spages = spages.clone();
            thread::spawn(move || {
                let cli = Client::new();
                for link in rlinks.iter() {
                    spages.send((link, get_page(&cli, &link)));
                }
            });
        }
        rpages
    };
    for (link, page) in rpages.iter() {
        match page {
            Err(err) => println!("Could not fetch {}: {}", link, err),
            Ok(page) => println!("{}, {}", page.len(), link),
        }
    }
}

fn get_page(cli: &Client, link: &str) -> Result<String, Box<Error+Send+Sync>> {
    let mut resp = try!(cli.get(link).send());
    if !resp.status.is_success() {
        return Err(From::from(resp.status.to_string()));
    }
    let mut page = String::with_capacity(1024);
    try!(resp.read_to_string(&mut page));
    Ok(page)
}
