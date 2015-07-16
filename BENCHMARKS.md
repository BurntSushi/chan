```
test mpsc_chan_async                         ... bench:     356,330 ns/iter (+/- 37,373) = 2 MB/s
test mpsc_chan_sync_buffered                 ... bench:  16,186,014 ns/iter (+/- 962,562)
test mpsc_chan_sync_buffered_all             ... bench:     371,997 ns/iter (+/- 20,529) = 2 MB/s
test mpsc_chan_sync_unbuffered               ... bench:  25,843,510 ns/iter (+/- 3,365,831)
test select_no_init_chan_async               ... bench:   1,221,417 ns/iter (+/- 84,438)
test select_no_init_chan_sync_buffered       ... bench:   5,116,771 ns/iter (+/- 442,794)
test select_no_init_chan_sync_buffered_all   ... bench:   1,238,522 ns/iter (+/- 74,315)
test select_no_init_chan_sync_unbuffered     ... bench:   5,281,292 ns/iter (+/- 611,475)
test select_with_init_chan_async             ... bench:     759,107 ns/iter (+/- 68,027) = 1 MB/s
test select_with_init_chan_sync_buffered     ... bench:   7,015,030 ns/iter (+/- 814,833)
test select_with_init_chan_sync_buffered_all ... bench:     769,810 ns/iter (+/- 66,908) = 1 MB/s
test select_with_init_chan_sync_unbuffered   ... bench:   8,829,461 ns/iter (+/- 938,300)
test spsc_chan_async                         ... bench:     324,128 ns/iter (+/- 10,292) = 3 MB/s
test spsc_chan_sync_buffered                 ... bench:  12,186,476 ns/iter (+/- 1,400,514)
test spsc_chan_sync_buffered_all             ... bench:     345,468 ns/iter (+/- 4,002) = 2 MB/s
test spsc_chan_sync_unbuffered               ... bench:  13,158,969 ns/iter (+/- 607,033)
test std_mpsc_chan_async                     ... bench:     152,193 ns/iter (+/- 17,694) = 6 MB/s
test std_mpsc_chan_sync_buffered             ... bench:  13,696,283 ns/iter (+/- 1,576,489)
test std_mpsc_chan_sync_buffered_all         ... bench:     202,740 ns/iter (+/- 17,262) = 4 MB/s
test std_mpsc_chan_sync_unbuffered           ... bench:  13,980,354 ns/iter (+/- 2,057,872)
test std_spsc_chan_async                     ... bench:     160,074 ns/iter (+/- 19,480) = 6 MB/s
test std_spsc_chan_sync_buffered             ... bench:  13,613,480 ns/iter (+/- 1,937,164)
test std_spsc_chan_sync_buffered_all         ... bench:     259,622 ns/iter (+/- 9,902) = 3 MB/s
test std_spsc_chan_sync_unbuffered           ... bench:  13,609,904 ns/iter (+/- 1,998,175)
```
