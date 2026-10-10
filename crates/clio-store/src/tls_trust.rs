// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the trust a TLS connection rests on: the
//! bundled public roots plus any caller-supplied root, and the two rustls
//! connectors the store chooses between.
//!
//! ## Owns
//! - Building the root set from the bundled roots plus one supplied PEM file
//! - The rustls client config a verified or unverified connection uses
//! - The verified connector that checks the server certificate
//! - The deliberately unverified connector for `encrypt-no-verify`
//! - Reading a PEM file into DER certificates, with a message an operator can act on
//!
//! ## Does not own
//! - Which connector a mode picks (see `pg_tls`)
//! - The URL the driver connects to (see `pg_url`)
//! - The probe or the retry window (`pg_probe` owns those)
//!
//! ## Boundary
//!
//! Changes to the roots or the connectors should normally require changes here.
//! Changes to the mode names or the connection parse should not.
//!
//! # Why the supplied root is added, never substituted
//!
//! `sslrootcert` in `libpq` names a root to trust *in addition* to the system
//! set, so a public certificate keeps working with no configuration and a
//! private one works with one line. Substituting would break every server whose
//! certificate already verified, which is the opposite of the fix.
//!
//! # Why the unverified connector exists at all
//!
//! It encrypts the connection and authenticates nothing. It is reached only
//! when an operator names `encrypt-no-verify`, never by default, and its whole
//! purpose is to be a deliberate, named choice rather than a silent fallback.

use crate::pg_tls::Failure;
use postgres_rustls::MakeTlsConnector;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::sync::Arc;
use tokio_rustls::TlsConnector;

/// The root set a verified connection trusts: the bundled public roots, plus a
/// supplied root when one is named.
///
/// # Errors
/// The supplied file cannot be opened or read, or holds no certificate this
/// build can parse.
pub(crate) fn trust_roots(supplied: Option<&str>) -> Result<RootCertStore, Failure> {
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    if let Some(path) = supplied {
        let certs = read_pem(path)?;
        let (added, ignored) = roots.add_parsable_certificates(certs);
        if added == 0 {
            return Err(Failure::settled(format!(
                "the root certificate at `{path}` holds no usable certificate ({ignored} entry/ies \
                 ignored): sslrootcert must name a PEM file, the way libpq expects"
            )));
        }
    }
    Ok(roots)
}

/// The rustls client config a verified connection uses: the bundled roots plus
/// any supplied root, and the protocol versions this build supports.
///
/// Kept separate from the connector so a test can drive a real handshake with
/// the same config the store hands to `postgres_rustls`.
///
/// # Errors
/// The supplied root cannot be read, or rustls cannot build the config.
pub(crate) fn verified_config(supplied: Option<&str>) -> Result<Arc<ClientConfig>, Failure> {
    Ok(Arc::new(
        builder()?
            .with_root_certificates(trust_roots(supplied)?)
            .with_no_client_auth(),
    ))
}

/// The rustls client config for `encrypt-no-verify`: TLS, with no check on the
/// certificate.
///
/// # Errors
/// rustls cannot build the config.
pub(crate) fn unverified_config() -> Result<Arc<ClientConfig>, Failure> {
    Ok(Arc::new(
        builder()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
            .with_no_client_auth(),
    ))
}

/// The connector that checks the server certificate against [`trust_roots`].
///
/// # Errors
/// The supplied root cannot be read, or rustls cannot build a client config.
pub(crate) fn verified_connector(supplied: Option<&str>) -> Result<MakeTlsConnector, Failure> {
    Ok(connector(verified_config(supplied)?))
}

/// The connector for `encrypt-no-verify`: TLS, with no check on the certificate.
///
/// # Errors
/// rustls cannot build a client config.
pub(crate) fn unverified_connector() -> Result<MakeTlsConnector, Failure> {
    Ok(connector(unverified_config()?))
}

/// A rustls client builder with the protocol versions this build supports.
fn builder() -> Result<rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier>, Failure> {
    ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| Failure::settled(format!("tls setup: {e}")))
}

/// The store's TLS connector, from a finished client config.
fn connector(config: Arc<ClientConfig>) -> MakeTlsConnector {
    MakeTlsConnector::new(TlsConnector::from(config))
}

/// Read every certificate in a PEM file into DER.
///
/// # Errors
/// The file cannot be opened or read, or it is not a PEM file.
fn read_pem(path: &str) -> Result<Vec<CertificateDer<'static>>, Failure> {
    let file = std::fs::File::open(path).map_err(|e| {
        Failure::settled(format!(
            "the root certificate at `{path}` could not be opened: {e}"
        ))
    })?;
    let mut reader = std::io::BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            Failure::settled(format!(
                "the root certificate at `{path}` could not be read: {e}"
            ))
        })
}

/// Accepts any server certificate, for the `encrypt-no-verify` mode only.
#[derive(Debug)]
struct AcceptAnyCertificate;

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
#[path = "tls_trust_tests.rs"]
mod tls_trust_tests;
