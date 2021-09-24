// This Source Code Form is subject to the terms of the Mozilla Public
// License, v2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{ffi::OsString, num::NonZeroU8, process::Command};

use concurrent_interner::serial;
use concurrent_interner::ConcurrentInterner;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use crossbeam::thread::Scope;
use std::hash::{BuildHasher, BuildHasherDefault};
use walkdir::WalkDir;

fn go<'a, RS: Send + Sync + Clone + BuildHasher>(
    nthreads: u8,
    scope: &&Scope<'a>,
    interner: &'a ConcurrentInterner<RS>,
    path: &str,
) {
    let mut senders = vec![];
    for _ in 0..nthreads {
        let (sender, receiver) = std::sync::mpsc::channel();
        senders.push(sender);
        let _ = scope.spawn(move |_| {
            let mut m = interner.get_member();
            loop {
                match receiver.recv() {
                    Ok(Run::Continue(pb)) => {
                        let buf = std::fs::read_to_string(pb).unwrap();
                        for s in buf.split_whitespace() {
                            m.intern(s);
                        }
                    }
                    Ok(Run::Stop) | Err(_) => break,
                }
            }
        });
    }
    let mut rs = OsString::new();
    rs.push("rs");
    let mut idx = 0;
    for pb in WalkDir::new(path).into_iter().filter_map(|e| {
        let pb = e.ok()?.into_path();
        if pb.extension() == Some(&rs) {
            Some(pb)
        } else {
            None
        }
    }) {
        senders[idx]
            .send(Run::Continue(pb))
            .expect("failed to send continue message");
        idx = (idx + 1) % (nthreads as usize);
    }
    for idx in 0..nthreads {
        senders[idx as usize]
            .send(Run::Stop)
            .expect("failed to send stop message");
    }
}

fn do_stuff_multi<RS: Send + Sync + Clone + Default + BuildHasher>(nthreads: u8, path: &str) {
    let interner =
        ConcurrentInterner::<RS>::new(NonZeroU8::new(nthreads).unwrap(), Default::default());
    let _ = crossbeam::scope(|scope| {
        go(nthreads, &scope, &interner, path);
    });
}

fn do_stuff<RS: Default + BuildHasher>(path: &str) {
    let arena = typed_arena::Arena::new();
    let mut basic_interner = serial::Interner::<RS>::new(&arena);
    find_rs_files(path, &mut basic_interner);
}

fn warm_cache(path: &str) {
    for _ in 0..10 {
        do_stuff::<ahash::RandomState>(path);
    }
}

#[cfg(target_os = "macos")]
#[allow(unused)]
fn clear_cache() {
    Command::new("sudo")
        .arg("sync")
        .output()
        .expect("sync failed");
    Command::new("sudo")
        .arg("purge")
        .output()
        .expect("purge failed");
}

#[cfg(target_os = "linux")]
#[allow(unused)]
fn clear_cache() {
    Command::new("sudo")
        .arg("sync")
        .output()
        .expect("sync failed");
    Command::new("sudo")
        .arg("sysctl")
        .arg("vm.drop_caches=1")
        .output()
        .expect("sysctl vm.drop_caches=1 failed");
}

fn criterion_benchmark(c: &mut Criterion) {
    criterion_benchmark_generic::<ahash::RandomState>(c, "ahash");
    criterion_benchmark_generic::<BuildHasherDefault<rustc_hash::FxHasher>>(c, "fxhash");
}

fn criterion_benchmark_generic<RS: Send + Sync + Clone + Default + BuildHasher>(
    c: &mut Criterion,
    hash_name: &'static str,
) {
    let benchmark_name = |n: u8, cache_state: &'static str| {
        format!(
            "intern {}: ({}/{}, n = {})",
            if n == 1 { "serial" } else { "parallel" },
            cache_state,
            hash_name,
            n,
        )
    };

    let nthreads = match std::env::var("BENCH_NTHREADS") {
        Ok(s) => s
            .split_whitespace()
            .map(|x| x.parse::<u8>())
            .collect::<Result<Vec<_>, _>>()
            .expect("BENCH_NTHREADS should be a whitespace-separated list of u8s"),
        Err(_) => (1..=8).collect(),
    };

    let path = "./benchmarks/data";

    let mut run_benchmark = |name| {
        for n in nthreads.iter() {
            c.bench_function(&benchmark_name(*n, name), |b| {
                b.iter_batched(
                    if name == "warm" { || {} } else { clear_cache },
                    |_| {
                        if *n == 1 {
                            do_stuff::<RS>(path)
                        } else {
                            do_stuff_multi::<RS>(*n, path)
                        }
                    },
                    BatchSize::NumIterations(1),
                );
            });
        }
    };

    match std::env::var("BENCH_COLD_CACHE") {
        Ok(_) => {
            clear_cache();
            run_benchmark("cold");
        }
        Err(_) => {}
    }

    warm_cache(path);
    run_benchmark("warm");
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);

// Benchmark:
//
// A. Walk over nested directories with lots of files:
// - For each file: -> move file to thread
//   -> keep tokenizing and adding to interner
//
// B. Walk over nested directories with lots of files:
// - For each file:
//   -> keep tokenizing and add to simple interner
//   https://www.reddit.com/r/rust/comments/fn1jxf/blog_post_fast_and_simple_rust_interner/fl7xvkt?utm_source=share&utm_medium=web2x&context=3
//
// Combinations:
// (1) With warm disk buffer cache
// (2) With cold disk buffer cache

fn find_rs_files<'a, RS: Default + BuildHasher>(
    path: &str,
    interner: &mut serial::Interner<'a, RS>,
) {
    let mut rs = OsString::new();
    rs.push("rs");
    for pb in WalkDir::new(path).into_iter().filter_map(|e| {
        let pb = e.ok()?.into_path();
        if pb.extension() == Some(&rs) {
            Some(pb)
        } else {
            None
        }
    }) {
        let buf = std::fs::read_to_string(&pb).unwrap();
        for str in buf.split_whitespace() {
            interner.intern(str);
        }
    }
}

enum Run<T> {
    Continue(T),
    Stop,
}
