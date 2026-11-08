// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the harness the `version` verb's two test suites share: one
//! scratch home per test, the marker writer, the report line's key list, and a
//! loopback listener that stands in for the release endpoint.
//!
//! ## Owns
//! - `Scratch`, `env`, `package`, and `write_marker`
//! - `KEYS` and `keys_of`, the report line's four keys
//! - `Server` and `Reply`, the listener and what it answers
//!
//! ## Does not own
//! - Any assertion (`version_check_tests`, `version_check_release_tests`)
//! - The implementation under test (`version_check`, `version_check_release`)
//!
//! ## Boundary
//!
//! Test-only: every path is a scratch directory and every endpoint is a listener
//! bound to port 0, so no suite touches the real home directory or the network.
//!
//! Keep this module focused on the shared harness.

use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use crate::skill_marker::{SKILL_MARKER_FILE, SkillMarker};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

/// One test's own directory, removed when the guard drops.
pub(crate) struct Scratch {
    root: PathBuf,
}

impl Scratch {
    pub(crate) fn new(label: &str) -> Scratch {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "clio-version-check-{label}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("the scratch directory can be created");
        Scratch { root }
    }

    pub(crate) fn home(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A Linux-layout environment rooted at `home`, so no test reads the real home.
pub(crate) fn env(home: &Path) -> DirectWriteEnv {
    DirectWriteEnv::with(OsLayout::Linux, home.to_path_buf(), None, None, None)
}

/// The package directory one home-relative root holds.
pub(crate) fn package(home: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part))
        .join(crate::skill_install::SKILL_DIR_NAME)
}

/// Write a marker recording `package_version` and `catalog_hash` into `package`.
pub(crate) fn write_marker(package: &Path, package_version: &str, catalog_hash: &str) {
    std::fs::create_dir_all(package).expect("the package directory can be created");
    let marker = SkillMarker {
        installer: crate::skill_marker::SKILL_INSTALLER.to_owned(),
        package_version: package_version.to_owned(),
        catalog_hash: catalog_hash.to_owned(),
        payload_digest: String::new(),
        files: BTreeMap::new(),
        created_package_dir: false,
        harnesses: BTreeSet::new(),
    };
    std::fs::write(
        package.join(SKILL_MARKER_FILE),
        marker.to_bytes().expect("the marker serializes"),
    )
    .expect("the marker can be written");
}

/// The four keys the report's line carries, in the order it writes them.
pub(crate) const KEYS: [&str; 4] = [
    "\"skillVersion\"",
    "\"catalogHash\"",
    "\"releaseStatus\"",
    "\"latestVersion\"",
];

/// Every key of the line, in the order the line states them.
pub(crate) fn keys_of(line: &str) -> Vec<&'static str> {
    let mut found: Vec<(usize, &'static str)> = KEYS
        .iter()
        .filter_map(|key| line.find(key).map(|at| (at, *key)))
        .collect();
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, key)| key).collect()
}

/// What the loopback listener does with the one request it receives.
pub(crate) enum Reply {
    /// Answer with these exact bytes and close.
    Bytes(Vec<u8>),
    /// Answer nothing, holding the connection open until the test stops it.
    Stall,
}

/// One test's own loopback listener: bound to port 0, stopped and joined when
/// the test ends, so no test shares a port and none leaks a thread.
pub(crate) struct Server {
    address: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    pub(crate) fn start(reply: Reply) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port is free");
        listener
            .set_nonblocking(true)
            .expect("the listener can be non-blocking");
        let address = format!(
            "http://{}",
            listener.local_addr().expect("the bound address")
        );
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let counted = Arc::clone(&requests);
        let stopped = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                // Nothing waiting yet: the test's client is on its way.
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                counted.fetch_add(1, Ordering::Relaxed);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                match &reply {
                    Reply::Bytes(bytes) => {
                        let _ = stream.write_all(bytes);
                    }
                    Reply::Stall => {
                        while !stopped.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            }
        });
        Server {
            address,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    /// The endpoint URL for `path` on this listener.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.address)
    }

    /// Stop the listener, join its thread, and report how many requests it saw.
    pub(crate) fn stop_and_count(mut self) -> usize {
        self.stop.store(true, Ordering::Relaxed);
        let count = self.requests.load(Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        count
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A 200 answer carrying `body` as JSON.
pub(crate) fn json_reply(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A status answer with no body.
pub(crate) fn status_reply(code: u16) -> Vec<u8> {
    format!("HTTP/1.1 {code} Status\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").into_bytes()
}

/// A redirect to a port nothing listens on, so following it would fail loudly.
pub(crate) fn redirect_reply() -> Vec<u8> {
    b"HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/elsewhere\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec()
}

/// A 200 answer one byte past the cap.
pub(crate) fn oversized_reply() -> Vec<u8> {
    let body = vec![b'a'; crate::version_check_release::MAX_RESPONSE_BYTES + 1];
    let mut reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    reply.extend_from_slice(&body);
    reply
}
