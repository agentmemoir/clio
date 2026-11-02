// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one test in this crate that talks to a real Secret Service daemon: it installs the store this build ships, writes a credential through the production wrapper, reads it back, deletes it, and checks that the delete removed it.
//!
//! ## Owns
//! - The Linux round trip against a real Secret Service, behind the `linux-secret-service-test` feature
//! - The unique service and account the credential is filed under
//! - The bounded wait that turns a hung bus into a named failure
//! - The guard that deletes the credential even when a step fails
//!
//! ## Does not own
//! - Which store is installed, and when (see `secret_key_store_platform`)
//! - The entry lifecycle over an in-memory credential (see `secret_key_store_tests`)
//! - The install outcomes a host with no daemon produces (see `secret_key_store_platform_tests`)
//!
//! ## Boundary
//!
//! This is the only test in the crate that reaches a real OS store, so it is off by default: the module is gated to Linux, to test builds, and to the `linux-secret-service-test` feature, and `cargo test -p clio-config` compiles and runs none of it. Turn it on only where a session bus carries a Secret Service daemon, which is what the `linux-secret-service` CI job provides; on a host without one the install fails and this test fails on purpose rather than skipping, because a skip is how this arm stayed unverified.
//!
//! # Why this exists, and why it needs a real daemon
//!
//! Every other test of the platform store stops short of the store itself. The install seam is driven with a store that has no backend behind it, the entry lifecycle is driven over an in-memory credential, and the Linux test of the install itself accepts either outcome. All of that can pass while the shipped store writes nothing and reads nothing: none of it makes a D-Bus call with a credential in hand.
//!
//! This test makes those calls. It needs an `org.freedesktop.secrets` service on the session bus, which no developer machine is obliged to have, so it is not part of the suite's normal run. It is also the reason the `linux-secret-service` job exists: a green Linux run of the ordinary suite says the crate compiles and the failure path speaks, and says nothing about whether the store works.

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::os_entry;
use crate::secret_key_store::{ACCOUNT, KeyringStore, SecretStore, display_name};

/// What the Secret Service store's `CredentialStoreApi::vendor` begins with.
///
/// Compared against the store installed in this process, so a store some other test in this binary installed is refused instead of answering the round trip in the real one's place.
const SECRET_SERVICE_VENDOR: &str = "Secret Service store";

/// The bytes the round trip writes. Not a key and not shaped like one, so a run that leaks leaves nothing usable behind.
const SECRET: &[u8] = b"clio Secret Service round trip";

/// How long the whole sequence is given before the test reports a hung store.
///
/// The store's connect and its calls are blocking and carry no timeout of their own, so the wait is bounded here: a daemon that stops answering fails this test by name instead of hanging the job until the runner kills it.
const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(60);

#[test]
fn a_secret_round_trips_through_the_secret_service_store() {
    // The whole real-store sequence runs on its own thread, which is what makes the wait bounded: a channel with a deadline is the only way to time out a call the store does not bound itself. The install is inside that sequence on purpose, because connecting to the bus is the step most likely to hang when a daemon is unhealthy.
    let (sender, receiver) = channel();
    let _worker = std::thread::spawn(move || {
        let outcome = live_round_trip();
        let _ = sender.send(outcome);
    });
    match receiver.recv_timeout(ROUND_TRIP_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => panic!("{reason}"),
        Err(RecvTimeoutError::Timeout) => panic!(
            "the Secret Service did not finish the round trip in {ROUND_TRIP_TIMEOUT:?}, so the \
             install or a call is hung"
        ),
        Err(RecvTimeoutError::Disconnected) => {
            panic!("the round-trip thread ended without a result, so it panicked before reporting")
        }
    }
}

/// The whole sequence the test asserts: install, open the entry, write, read, delete, confirm the delete took.
///
/// The failures are `String` rather than `AmError` because nothing panics inside the timed thread, and each one names the step, so a red run says which call the store refused.
fn live_round_trip() -> Result<(), String> {
    // The production install and the production entry: the same two calls a run of Clio makes, so a host that cannot reach a Secret Service reports the reason an operator would see. Failing here is the point — the job that runs this test provides the daemon, and a skip would restore exactly the gap the test was added to close.
    os_entry(ACCOUNT).map_err(|err| {
        format!(
            "{} store did not install, so no real round trip can run: {}. This test needs a \
             session bus carrying an `org.freedesktop.secrets` daemon; the `linux-secret-service` \
             CI job runs it under `dbus-run-session` with `gnome-keyring-daemon`.",
            display_name(),
            err.message
        )
    })?;

    // The entry above installed the store, so the default is the one this build ships. Confirm it: a sibling test that installed a store with no backend behind it would answer every call below, and the round trip would pass over nothing.
    let store = keyring_core::get_default_store()
        .ok_or("the entry above reported success but left no store installed")?;
    let vendor = store.vendor();
    if !vendor.starts_with(SECRET_SERVICE_VENDOR) {
        return Err(format!(
            "the installed store is not the Secret Service store ({vendor}), so another test in \
             this binary installed its own; run this test on its own"
        ));
    }

    // A service and account of this run's own: the credential cannot collide with a concurrent run, and it is nowhere near the `clio` entry an operator's real key lives in, which this test must never read or delete.
    let service = unique("service");
    let account = unique("account");
    let test_key = KeyringStore::over(account, "the round-trip test key", move |account| {
        store.build(service, account, None)
    });

    write_read_and_delete(&test_key)
}

/// Write the credential, read it back, delete it, and confirm the delete took.
fn write_read_and_delete(test_key: &KeyringStore) -> Result<(), String> {
    // Cleanup is a guard rather than a call at the end: a failure below leaves this function through `?` and through an early return, and neither may leave a credential in the store.
    let _cleanup = DeletesOnDrop(test_key);

    test_key
        .set(SECRET)
        .map_err(|err| format!("cannot write the test credential: {}", err.message))?;

    let read = test_key
        .get()
        .map_err(|err| format!("cannot read the test credential back: {}", err.message))?;
    match read {
        Some(bytes) if bytes == SECRET => {}
        Some(bytes) => {
            return Err(format!(
                "read back {} bytes, not the {} that were written",
                bytes.len(),
                SECRET.len()
            ));
        }
        None => {
            return Err("the credential written a moment earlier is not in the store".to_owned());
        }
    }

    test_key
        .delete()
        .map_err(|err| format!("cannot delete the test credential: {}", err.message))?;
    let after = test_key.get().map_err(|err| {
        format!(
            "cannot read the credential after the delete: {}",
            err.message
        )
    })?;
    if after.is_some() {
        return Err(
            "the store still holds the credential after the delete reported success".to_owned(),
        );
    }

    Ok(())
}

/// Deletes the test credential when the sequence ends, whichever way it ends.
///
/// Deleting an absent entry is the state the caller asked for, so the happy path's second delete costs one call and a failing path leaves nothing behind.
struct DeletesOnDrop<'a>(&'a KeyringStore);

impl Drop for DeletesOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self.0.delete();
    }
}

/// A service or account name unique to this run, as the `&'static str` the entry factory takes.
///
/// The process id keeps two processes apart and the nanosecond clock keeps two runs inside one process apart, so parallel and repeated runs cannot see each other's credential.
fn unique(kind: &str) -> &'static str {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    Box::leak(format!("clio-test-{kind}-{}-{nanos}", std::process::id()).into_boxed_str())
}
