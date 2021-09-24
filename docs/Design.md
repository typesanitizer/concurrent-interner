# Design

<!-- [NOTE: interner-design] -->

## Goals

The interner's design tries to achieve the following:
- Interned strings should be small.

  **Reason:** Reducing memory usage for data types
  containing interned strings
  is valuable.
- Interner should be shareable across threads with minimal locking.

  **Reason:** It lets us parse things in parallel (good for throughput)
  while de-duping strings across files, which is good for memory usage
  and allows cheap string comparison across threads.

  For example, batch compilers commonly use coarse-grained concurrency,
  with the degree of concurrency staying constant
  over the program's duration.
  This makes it OK to use thread-local non-concurrent hash tables,
  since there is no cross-thread data transfer.
  Using a concurrent hash table allows you to change the
  degree of concurrency at different stages of the compiler pipeline,
  without having to resort to hash-consing for fast string comparison
  (which would incur higher memory usage).

- Try to keep memory fragmentation down.

  **Reason:** Memory fragmentation bad.

## High-Level Design

- The core hash table is a thread-safe one: `DashMap`.
- The storage is divided into disjoint growable arenas.
- When a thread needs to intern many strings,
  it asks the interner for a member using `get_member`.
- Each thread interns strings using the member it gets.
- Members have a 1:1 correspondence to arenas;
  a member doesn't need to take any locks
  when allocating into its own arena,
  and it can't allocate into other members' arenas.
- An arena is a monotonically growing vector of fixed-size chunks
  (huge strings introduce some wrinkles but nothing major).
- A string is allocated (into an arena)
  when a member tries to intern a string
  that no thread has recorded as "interned" so far.
  (The wording is a mouthful here because another thread may have observed
  the same "miss" and may be allocating or about to update the shared hash table
  (i.e. a [TOCTOU problem][]), but that's alright,
  apart from a little bit of wasted memory.)
  <!-- [NOTE: interner-overallocate] -->
- The member stores a "composite index"
  (a triple identifying the chunk, start and len inside it)
  into a separate, monotonically growing composite index vector.
- The index at which the new composite index is stored in the vector is
  combined with the arena's id to create an interned string.

[TOCTOU problem]: https://en.wikipedia.org/wiki/Time-of-check_to_time-of-use

So we do achieve the three main goals: (assuming the implementation is sound)
- [X] Interned strings should be small.
      
  **Status:** Interned strings are 32-bits.
- [X] Interner should be shareable across threads with minimal locking.
   
  **Status:** The member API, which borrows (heh) heavily
  from `bumpalo-herd`'s `Herd` type
  makes it possible to use the interner from multiple threads.
  One situation in which we may over-allocate
  is when multiple threads try to intern a string for a first time.
  However, I suspect this is unlikely to be an issue in practice.
- [X] Try to keep memory fragmentation down.

  **Status:** Permanent storage for strings
  is allocated out of contiguous chunks,
  instead of a simple design
  where the storage is `Vec<String>`
  with many small allocations for the `String`s.

## Design Drawbacks

- The implementation and API are more complex
  than that for a serial interner.
- You need to lug around the interner
  to wherever you want to access the string
  which may be cumbersome, especially for debugging.
  (For debugging specifically,
  you could stick the interner into a global variable...)
- Interning a string is more expensive
  compared to using thread-local non-concurrent hash tables.
  This crate makes the assumption that
  the additional insertion overhead
  and poorer scaling under high contention
  for a concurrent interner
  doesn't make a meaningful difference to lexing+parsing throughput,
  so it makes sense to prioritize reducing memory usage.
  Alternately, you can view this crate
  as a mostly plug-and-play option
  to test the aforementioned assumption.
