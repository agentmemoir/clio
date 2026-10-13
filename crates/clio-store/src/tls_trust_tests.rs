// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the trust a TLS connection rests on:
//! that a supplied root is added to the bundled roots, that a bad file is
//! refused with a message an operator can act on, that a certificate signed by
//! the supplied root verifies while the same certificate does not verify without
//! it, and that a real handshake built from the store's own config accepts the
//! supplied root and rejects the same server without it.
//!
//! ## Owns
//! - The bundled-roots-plus-supplied composition
//! - The refusals for a missing or non-certificate file
//! - Certificate verification against the composed roots
//! - A real TLS handshake through [`verified_config`] and [`unverified_config`]
//!
//! ## Does not own
//! - The Postgres protocol handshake (`pg_tls_db_tests` owns that)
//! - Reading `sslrootcert` out of a URL (`pg_url` owns that)
//!
//! ## Boundary
//!
//! Changes to how roots are composed or checked should normally require changes
//! here. Changes to the mode names should not.
//!
//! The certificates under `tests/fixtures` are a throwaway private CA and a leaf
//! it signed, both for `localhost`. `server.key.pem` is the leaf's test-only key:
//! it authenticates nothing but the in-process server these tests start, and it
//! is not a credential for any real service.

use super::*;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConnection, ServerConfig, ServerConnection, StreamOwned};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
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
    UnixTime::since_unix_epoch(Duration::from_hours(500_000))
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
    assert!(err.message.contains(&missing));
}

#[test]
fn a_file_that_is_not_a_certificate_is_refused() {
    let path = std::env::temp_dir().join(format!("clio-tls-trust-{}.pem", std::process::id()));
    std::fs::write(&path, b"this is not a certificate\n").expect("write the scratch file");
    let err = trust_roots(Some(&path.to_string_lossy())).expect_err("junk is refused");
    let _ = std::fs::remove_file(&path);
    assert!(err.message.contains("no usable certificate"));
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
    assert!(outcome.is_ok(), "the leaf chains to the supplied root");
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

#[test]
fn both_connectors_build_for_the_store() {
    assert!(
        verified_connector(None).is_ok(),
        "the bundled-root connector builds"
    );
    assert!(
        verified_connector(Some(&fixture("private_ca.pem"))).is_ok(),
        "the private-root connector builds"
    );
    assert!(
        unverified_connector().is_ok(),
        "the no-verify connector builds"
    );
}

/// A rustls server config from the fixture leaf and its test-only key.
fn server_config() -> Arc<ServerConfig> {
    let certs = read_pem(&fixture("server.pem")).expect("the fixture leaf reads");
    let key = {
        let file = std::fs::File::open(fixture("server.key.pem")).expect("the fixture key opens");
        let mut reader = std::io::BufReader::new(file);
        rustls_pemfile::private_key(&mut reader)
            .expect("the fixture key reads")
            .expect("the file holds one key")
    };
    Arc::new(
        ServerConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .expect("safe protocol versions")
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("a server config builds from the fixture"),
    )
}

/// Run one TLS handshake against an in-process server built from the fixture,
/// and report the client's result. A certificate the client refuses fails its
/// first write, which is the same failure the store would see.
///
/// The server reads one byte and echoes it, so the handshake completes on both
/// sides. A five-second read timeout bounds either side, so a logic error fails
/// a test instead of hanging the run.
fn handshake(client: Arc<ClientConfig>) -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let addr = listener.local_addr().expect("the bound address");
    let server = std::thread::spawn(move || -> Result<(), String> {
        let (tcp, _) = listener.accept().map_err(|e| e.to_string())?;
        tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let conn = ServerConnection::new(server_config()).map_err(|e| e.to_string())?;
        let mut tls = StreamOwned::new(conn, tcp);
        let mut byte = [0u8; 1];
        tls.read_exact(&mut byte).map_err(|e| e.to_string())?;
        tls.write_all(&byte).map_err(|e| e.to_string())?;
        tls.flush().map_err(|e| e.to_string())
    });
    let tcp = TcpStream::connect(addr).expect("connect to the in-process server");
    tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let conn = ClientConnection::new(
        client,
        ServerName::try_from("localhost").expect("a dns name"),
    )
    .map_err(|e| e.to_string())?;
    let mut tls = StreamOwned::new(conn, tcp);
    let outcome = (|| -> Result<(), String> {
        tls.write_all(b"x").map_err(|e| e.to_string())?;
        tls.flush().map_err(|e| e.to_string())?;
        let mut byte = [0u8; 1];
        tls.read_exact(&mut byte).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = server.join();
    outcome
}

#[test]
fn a_real_handshake_succeeds_with_the_supplied_root() {
    let config = verified_config(Some(&fixture("private_ca.pem"))).expect("the config builds");
    assert!(
        handshake(config).is_ok(),
        "the supplied root must authenticate the fixture server"
    );
}

#[test]
fn a_real_handshake_fails_without_the_supplied_root() {
    let config = verified_config(None).expect("the config builds");
    assert!(
        handshake(config).is_err(),
        "a private root is not one of the bundled public roots"
    );
}

#[test]
fn a_real_handshake_succeeds_without_verification() {
    let config = unverified_config().expect("the config builds");
    assert!(
        handshake(config).is_ok(),
        "encrypt-no-verify must accept the certificate it does not check"
    );
}
