// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one collision-free name generator shared by
//! every test module in this crate.
//!
//! ## Owns
//! - The crate-wide call counter that makes [`unique`] distinct within a process
//!
//! ## Does not own
//! - The fixtures that consume the generated names (bundles, banks, scratch dirs)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Why this exists: the per-module generators this replaced built a name from
//! the wall clock plus the process id. Inside one process the pid is constant,
//! so two calls that landed in the same clock tick returned the same name. Two
//! import tests then wrote different bundle JSON to one path with a
//! non-atomic `std::fs::write`, and each read back the other's bytes as
//! `trailing characters`. The counter makes every call distinct no matter how
//! coarse the clock is.

use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process. Never wraps in any real test run.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A name that no other call in this process, and no other concurrent process,
/// will produce.
///
/// The process id separates concurrent test binaries, the counter separates
/// calls inside one binary, and the clock keeps a re-run from reusing a name a
/// previous run left behind on disk.
pub(crate) fn unique(prefix: &str) -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{nanos}-{}-{sequence}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::unique;
    use std::sync::Arc;

    #[test]
    fn adjacent_calls_are_distinct() {
        assert_ne!(unique("t"), unique("t"));
    }

    #[test]
    fn distinct_on_threads_that_call_at_the_same_instant() {
        // The old generator read the clock twice and nothing else, so two
        // threads that reached it together got the same name. Hold both threads
        // on a barrier so their calls are as close to simultaneous as the
        // scheduler allows, then require every name to be distinct.
        const THREADS: usize = 8;
        let barrier = Arc::new(std::sync::Barrier::new(THREADS));
        let mut names: Vec<String> = Vec::new();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        unique("t")
                    })
                })
                .collect();
            for handle in handles {
                names.push(handle.join().unwrap());
            }
        });
        names.sort();
        names.dedup();
        assert_eq!(names.len(), THREADS, "duplicate names: {names:?}");
    }

    #[test]
    fn prefix_is_preserved_so_call_sites_stay_readable() {
        assert!(unique("am-import-bundle").starts_with("am-import-bundle-"));
    }
}
