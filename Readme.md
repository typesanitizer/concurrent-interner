# concurrent-interner: Conveniently interning strings from multiple threads

[![Build status](https://github.com/typesanitizer/concurrent-interner/actions/workflows/ci.yml/badge.svg)](https://github.com/typesanitizer/concurrent-interner/actions)
[![crates.io](https://img.shields.io/crates/v/concurrent-interner.svg)](https://crates.io/crates/concurrent-interner)
[![docs.rs](https://img.shields.io/docsrs/concurrent-interner/0.1.0)](https://docs.rs/concurrent-interner/0.1.0/concurrent_interner/)
[![Rust](https://img.shields.io/badge/rust-1.56.1%2B-blue.svg?maxAge=3600)](https://github.com/typesanitizer/concurrent-interner/blob/main/Cargo.toml#L5)

**Request for help**:
I am looking for someone experienced with unsafe Rust
to audit the unsafe code.
It is MIRI clean as far as I can tell,
but MIRI take a very long time to run,
making it unsuitable for CI.

This crate provides an string interner
which is safe to use from multiple threads.
You can think of it as the mash up
of a concurrent hash map
and a single-threaded interner.

Documentation:
- See [Contributing](./docs/Contributing.md)
  before filing issues or submitting a PR.
- See [Design](./docs/Design.md) for the overall
  goals and the code works at a high-level.
  Hopefully, it helps you decide
  when you should (not) use this crate.
  Or you can directly read the source code.
- See [Performance](./docs/Performance.md) to see
  how you can run the accompanying benchmarks,
  as well as results for some workloads.
