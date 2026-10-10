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
//! - The fixtures and scratch directories that consume the generated names
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Why this exists: the 31 per-module generators this replaced built a name
//! from the wall clock plus the process id. Inside one process the pid is
//! constant, so two calls that landed in the same clock tick returned the same
//! name. In this crate that is not cosmetic — the name is a bank id or item id
//! in a Postgres instance shared by every other suite, and a scratch path on
//! disk — so a repeat means one test reads another's rows, or two tests write
//! one file. The counter makes every call distinct no matter how coarse the
//! clock is.

use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process. Never wraps in any real test run.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A name that no other call in this process, and no other concurrent process,
/// will produce.
///
/// The process id separates concurrent test binaries, the counter separates
/// calls inside one binary, and the clock keeps a re-run from reusing a name a
/// previous run left behind in the shared database or on disk.
pub(crate) fn unique(prefix: &str) -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{nanos}-{}-{sequence}", std::process::id())
}

/// A scratch directory path that no other call will produce, created eagerly.
///
/// The seven per-file `Scratch::new` guards this replaces built their path from
/// the clock and the pid only, and several of them shared a `clio-restore-*`
/// prefix. Two guards that pick the same tag in the same tick then returned the
/// same directory, and because the tests then write files into it, one test
/// reads the other's contents. `create_dir_all` makes this idempotent, so
/// callers keep the same "the directory exists when this returns" guarantee.
pub(crate) fn scratch_dir(prefix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(unique(prefix));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
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
        // The old generators read the clock twice and nothing else, so two
        // threads that reached them together got the same name. Release eight
        // threads from one barrier so their calls land as close to
        // simultaneous as the scheduler allows, then require distinct names.
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
        assert!(unique("bank-a").starts_with("bank-a-"));
    }
}
