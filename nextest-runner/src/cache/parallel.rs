// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A small work-stealing helper for hashing test binaries in parallel.
//!
//! Hashing test binaries is blocking, CPU/IO-bound work, so this uses
//! `std::thread::scope` rather than the tokio-based `async-scoped` used
//! elsewhere in nextest. The cache consult runs outside a live runtime (the
//! listing runtime is shut down first), and `async-scoped`'s scoped
//! `spawn_blocking` requires a multi-threaded runtime context. There is nothing
//! to await here, so plain scoped threads are both simpler and a better fit than
//! borrowing tokio's blocking pool through its unsafe lifetime glue.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

/// Runs `f` over every item across a bounded scoped thread pool, collecting the
/// `Some` results in unspecified order.
///
/// Workers pull the next index from a shared cursor rather than taking a fixed
/// slice, so none idles while others hash multi-gigabyte binaries; the pool is
/// capped at `min(parallelism, items.len())`. The scope lets workers borrow
/// `items` and `f` without `'static` bounds.
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
            // Propagate a worker panic rather than dropping results silently.
            all.extend(handle.join().expect("cache worker thread panicked"));
        }
    });
    all
}
