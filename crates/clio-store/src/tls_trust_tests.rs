// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the trust a TLS connection rests on:
//! that a supplied root is added to the bundled roots, that a bad file is
//! refused with a message an operator can act on, and that a certificate signed
//! by the supplied root verifies while the same certificate does not verify
//! without it.
//!
//! ## Owns
//! - The bundled-roots-plus-supplied composition
//! - The refusals for a missing or non-certificate file
//! - Certificate verification against the composed roots
//!
//! ## Does not own
//! - Reaching a real server (`pg_tls_db_tests` owns that)
//! - Reading `sslrootcert` out of a URL (`pg_url` owns that)
//!
//! ## Boundary
//!
//! Changes to how roots are composed or checked should normally require changes
//! here. Changes to the mode names should not.
//!
//! The two certificates under `tests/fixtures` are a throwaway private CA and a
//! leaf it signed, both for `localhost`. Only the certificates are kept; the
//! keys were discarded after signing.

use super::*;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A path to one of the checked-in certificate fixtures.
fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// The leaf certificate the private CA signed.
fn leaf() -> CertificateDer<'static> {
    read_pem(&fixture("server.pem"))
        .expect("the fixture leaf reads")
        .into_iter()
        .next()
        .expect("the fixture leaf holds one certificate")
}

/// A verifier over the roots built from `supplied`.
fn verifier(supplied: Option<&str>) -> Arc<WebPkiServerVerifier> {
    WebPkiServerVerifier::builder_with_provider(
        Arc::new(trust_roots(supplied).expect("the roots build")),
        Arc::new(default_provider()),
    )
    .build()
    .expect("a verifier builds from a non-empty root store")
}

/// A fixed instant inside the fixtures' validity, so the test does not read the
/// wall clock.
fn now() -> UnixTime {
    UnixTime::since_unix_epoch(Duration::from_secs(1_800_000_000))
}

#[test]
fn the_bundled_roots_are_always_present() {
    let roots = trust_roots(None).expect("the bundled roots build");
    assert!(
        !roots.roots.is_empty(),
        "the bundled public roots are the default trust"
    );
}

#[test]
fn a_supplied_root_is_added_alongside_the_bundled_ones() {
    let bundled = trust_roots(None).expect("bundled").roots.len();
    let with_ca = trust_roots(Some(&fixture("private_ca.pem"))).expect("a supplied root builds");
    assert_eq!(
        with_ca.roots.len(),
        bundled + 1,
        "the supplied root is added, never substituted"
    );
}

#[test]
fn a_missing_root_file_names_the_path() {
    let missing = fixture("no_such_ca.pem");
    let err = trust_roots(Some(&missing)).expect_err("a missing file is refused");
    assert!(err.message.contains(&missing), "{}", err.message);
}

#[test]
fn a_file_that_is_not_a_certificate_is_refused() {
    let path = std::env::temp_dir().join(format!("clio-tls-trust-{}.pem", std::process::id()));
    std::fs::write(&path, b"this is not a certificate\n").expect("write the scratch file");
    let err = trust_roots(Some(&path.to_string_lossy())).expect_err("junk is refused");
    let _ = std::fs::remove_file(&path);
    assert!(
        err.message.contains("no usable certificate"),
        "{}",
        err.message
    );
}

#[test]
fn a_certificate_signed_by_the_supplied_root_verifies() {
    let ca = fixture("private_ca.pem");
    let outcome = verifier(Some(&ca)).verify_server_cert(
        &leaf(),
        &[],
        &ServerName::try_from("localhost").expect("a dns name"),
        &[],
        now(),
    );
    assert!(
        outcome.is_ok(),
        "the leaf chains to the supplied root: {outcome:?}"
    );
}

#[test]
fn the_same_certificate_fails_without_the_supplied_root() {
    let outcome = verifier(None).verify_server_cert(
        &leaf(),
        &[],
        &ServerName::try_from("localhost").expect("a dns name"),
        &[],
        now(),
    );
    assert!(
        outcome.is_err(),
        "a private root is not one of the bundled public roots"
    );
}
