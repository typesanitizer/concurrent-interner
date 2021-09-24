# concurrent-interner: Conveniently interning strings from multiple threads

**At the moment, this crate should not be used for serious work.
I am looking for someone experienced with unsafe Rust to
audit the unsafe code first.**

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
