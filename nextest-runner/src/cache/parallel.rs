// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A small work-stealing helper for the cache's parallel binary passes.
//!
//! Hashing test binaries is blocking, CPU/IO-bound work, so this uses
//! `std::thread::scope` rather than the tokio-based `async-scoped` used
//! elsewhere in nextest. Both cache call sites run outside a live runtime (the
//! listing runtime is shut down before the consult pass; the writer runs before
//! the runner starts its own), and `async-scoped`'s scoped `spawn_blocking`
//! requires a multi-threaded runtime context. There is nothing to await here, so
//! plain scoped threads are both simpler and a better fit than borrowing tokio's
//! blocking pool through its unsafe lifetime glue.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

/// Runs `f` over every item in `items` across a bounded thread pool, collecting
/// the `Some` results into a `Vec` in unspecified order.
///
/// This backs both cache passes that hash test binaries: consulting the cache
/// before a run and storing results after it. Hashing a test binary reads every
/// byte of a file that is routinely several gigabytes, so the work is I/O- and
/// CPU-bound and scales well across cores.
///
/// Threads pull the next index from a shared atomic cursor as they finish rather
/// than being handed a fixed slice, so every worker stays busy even when items
/// differ wildly in size. The pool is capped at the smaller of the available
/// parallelism and the number of items — there is no point spawning more threads
/// than there is work or hardware.
///
/// A scoped thread pool lets the workers borrow `items` and `f` without
/// `'static` bounds, because the scope joins every thread before returning;
/// `f` is shared across workers (hence `Sync`) and its output crosses the thread
/// boundary (hence `Send`).
pub(super) fn parallel_filter_map<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> Option<R> + Sync,
{
    if items.is_empty() {
        return Vec::new();
    }

    let parallelism = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let num_threads = parallelism.min(items.len());

    let next = AtomicUsize::new(0);
    let f = &f;
    let mut all = Vec::new();
    thread::scope(|scope| {
        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(idx) else {
                            break;
                        };
                        if let Some(result) = f(item) {
                            local.push(result);
                        }
                    }
                    local
                })
            })
            .collect();
        for handle in handles {
            // A worker only panics if `f` itself panics; propagate it rather
            // than silently dropping the results computed so far.
            all.extend(handle.join().expect("cache worker thread panicked"));
        }
    });
    all
}
