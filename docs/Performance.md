# Performance

Ideas for interesting benchmarks
(or improving existing benchmarks)
are welcome on the issue tracker.
PRs to add numbers for other CPU/OS combinations
are also welcome.

## Workload 1 - parallel "tokenization" + interning for a "large" repo

The workload walks the `benchmarks/data` directory,
finds `.rs` files, splits them up into whitespace,
and interns the substrings.

This is a proxy for an actual compiler frontend
which would do something similar,
but with a proper lexer,
virtual file system etc.

We measure the wall time, both with a cold page cache
(clearing the cache requires `sudo`),
and a warm page cache. We also try both
[AHash](https://crates.io/crates/ahash) and
[FxHash](https://crates.io/crates/rustc-hash).
The latter is used by the Rust compiler and Firefox.

### Running

To run the benchmark:

```
git clone --quiet --depth=1 https://github.com/rust-analyzer/rust-analyzer.git --branch=2021-09-20 benchmarks/data/rust-analyzer
rm -rf benchmarks/data/rust-analyzer/.git
# Run benchmarks with warm caches only (cold cache numbers aren't too different)
cargo bench --all-features --bench interner-speed | tee output.txt
# More complex configuration (requires sudo to clear the page cache)
(export BENCH_NTHREADS="$(seq 1 3 11)"; export BENCH_COLD_CACHE=1; cargo bench --all-features --bench interner-speed | tee output.txt)
```

Depending on the settings,
it may take anywhere between a few minutes to 30+ minutes.
You should not use your computer in the meantime;
if you are clearing the page cache,
you will likely not be able to use it anyways. :sweat_smile:

We have a script which parses the Criterion output and prints a nice table.

```
./dev/criterion-to-table.py output.txt
```

## Results

### M1 MacBook Pro (2020): 4P + 4E cores

|nthreads|cold/ahash|cold/fxhash|warm/ahash|warm/fxhash|
|--------|----------|-----------|----------|-----------|
| n = 1  | 128.56 ms|  129.95 ms| 129.50 ms|  127.29 ms|
| n = 2  | 76.074 ms|  72.834 ms| 74.506 ms|  73.159 ms|
| n = 3  | 51.923 ms|  50.856 ms| 52.440 ms|  51.355 ms|
| n = 4  | 42.607 ms|  42.377 ms| 46.230 ms|  42.607 ms|
| n = 5  | 38.461 ms|  38.640 ms| 39.489 ms|  42.483 ms|
| n = 6  | 38.592 ms|  39.008 ms| 39.095 ms|  38.968 ms|
| n = 7  | 42.205 ms|  45.715 ms| 42.219 ms|  41.501 ms|
| n = 8  | 50.306 ms|  44.320 ms| 46.749 ms|  47.501 ms|

The key point I'd like to highlight is that peak performance is not at n = 8.
This is something I've consistently seen over multiple runs;
the best performance ends up being between n = 4 to n = 6.

Since this was done on a MacBook Pro, without much control over thermals,
you should not take the numbers for AHash vs FxHash comparison too seriously.
For example, FxHash usually performs slightly better on very short strings;
this workload does create many very short strings because it splits on whitespace,
whereas a real lexer would likely not intern any strings for individual
characters like `{` and `}`.

The point is, they're pretty close, and it is easy to swap out one for the other.
