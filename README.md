This crate provides experimental support for multi-producer/multi-consumer
channels. This includes bounded synchronous and asynchronous channels in
addition to an unbounded asynchronous channel.

There is also rudimentary support for "selecting" from one of several
synchronization events (sending or receiving).

[![Build status](https://api.travis-ci.org/BurntSushi/chan.png)](https://travis-ci.org/BurntSushi/chan)
[![](http://meritbadge.herokuapp.com/chan)](https://crates.io/crates/chan)

Dual-licensed under MIT or the [UNLICENSE](http://unlicense.org).


### Documentation

[http://burntsushi.net/rustdoc/chan/](http://burntsushi.net/rustdoc/chan/).


### Caveats

This implementation is *simplistic* to the point that there may be serious
performance issues. However, it is possible that it may be good enough (or
may one day become good enough). In particular, there are no fancy lock-free
algorithms or lightweight threads. Everything is implemented with plain old
condition variables and mutexes. Notably, this crate **uses no `unsafe`
blocks**.

The API presented here also differs substantially from the APIs offered by
`std::sync`. This is at least partly due to providing a
multi-producer/multi-consumer queue (`std::sync` provides
multi-producer/single-consumer), but also to keep the implementation simple.

This has some interesting side effects:

* There is no distinction between "senders" and "receivers." There's just
  channels.
* As a result, clients must call `close` explicitly on a channel when there are
  no more senders remaining. Neglecting to do this may lead to memory leaks
  depending on how receivers behave.
* The implementation of `select` in this crate can handle both send and receive
  synchronization events, but both the API and implementation are questionable
  at best.
* A crippled version of `select` called `choose` is provided which only
  supports receive synchronization events. However, it should be much faster in
  loops than its more general `select`.

Finally, much of the semantics applied in this crate were directly inspired by
Go's channels.

