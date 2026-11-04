// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one startup warning the Postgres store
//! raises about its own transport: a connection opened with
//! `encrypt-no-verify` is told, once, that it is encrypted but unchecked.
//!
//! ## Owns
//! - `encrypt_no_verify_warning`: the warning text for a connection string, and
//!   `None` for every mode that checks the server certificate
//! - `warn_encrypt_no_verify`: the stderr emission, once per store open
//!
//! ## Does not own
//! - The mode names and what each one means at connect time (`pg_tls`)
//! - Reading `sslmode` and `sslrootcert` out of the string (`pg_url`)
//! - The connector that accepts any certificate (`tls_trust`)
//!
//! ## Boundary
//!
//! Changes to what an operator is told about an unverified connection should
//! normally require changes here. Changes to how a mode is parsed, or to which
//! connector it picks, should not require changes here.
//!
//! Keep this module focused on the warning text and the one place it is emitted.

use crate::pg_tls::SslChoice;

const ENCRYPT_NO_VERIFY_WARNING: &str = "store: sslmode encrypt-no-verify is active: the \
    connection is encrypted but the server certificate is not validated, so any host that can \
    intercept it can impersonate the database; use sslmode=verify-full, with sslrootcert=PATH for \
    a private certificate authority, to validate it";

pub(crate) fn encrypt_no_verify_warning(url: &str) -> Option<&'static str> {
    let spec = crate::pg_url::spec(url).ok()?;
    match spec.ssl {
        SslChoice::EncryptNoVerify => Some(ENCRYPT_NO_VERIFY_WARNING),
        SslChoice::Disable | SslChoice::Prefer | SslChoice::VerifyFull => None,
    }
}

pub(crate) fn warn_encrypt_no_verify(url: &str) {
    if let Some(message) = encrypt_no_verify_warning(url) {
        clio_config::log::warn(
            message,
            &[
                ("sslmode", "encrypt-no-verify"),
                ("certificate_validation", "disabled"),
            ],
        );
    }
}

#[cfg(test)]
#[path = "pg_tls_warning_tests.rs"]
mod pg_tls_warning_tests;
