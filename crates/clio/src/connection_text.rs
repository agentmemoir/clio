// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the plain-text facts the CLI prints about a
//! connection address: one sentence saying whether the address is encrypted and
//! whether it stays on this machine.
//!
//! ## Owns
//! - The shared encryption/locality sentence used by the applied summary and
//!   `clio status`
//!
//! ## Does not own
//! - Parsing, validating, or connecting with the address (see `clio-store`)
//! - Redacting a password out of printed text (see `clio_ops::redact_credentials`)
//!
//! ## Boundary
//!
//! Changes to how an address is described in plain words should normally require
//! changes here. Changes to how it is parsed should not.
//!
//! Keep this module focused on the sentence the CLI prints about a store.

/// The one sentence that says how a stored address is protected: whether the
/// connection is encrypted, whether the certificate is checked, and whether it
/// stays on this machine.
///
/// `clio setup`'s applied summary and `clio status` both print this, so the two
/// can never describe the same store in two different ways, and an operator can
/// tell afterwards whether the connection was verified. A Postgres address
/// is read with the store's own parser, so the SSL mode and the locality are
/// the ones the driver will use. An address that parser refuses is a SQLite
/// file or `sqlite::memory:`, which is not encrypted and is on this machine.
#[must_use]
pub(crate) fn database_encryption_locality(address: &str) -> String {
    match clio_store::parse_connection(address) {
        Ok(parsed) => {
            let encryption = match parsed.ssl {
                clio_store::SslChoice::Disable => "not encrypted",
                clio_store::SslChoice::Prefer => "encrypted and verified when offered",
                clio_store::SslChoice::VerifyFull => "encrypted and verified",
                clio_store::SslChoice::EncryptNoVerify => "encrypted, not verified",
            };
            let locality = if parsed.is_local() {
                "this machine only"
            } else {
                "remote"
            };
            format!("{encryption}, {locality}")
        }
        // A path is a SQLite file: not encrypted, and on this machine. A
        // Postgres address that does not parse is neither, and describing it as
        // unencrypted and local would say the opposite of the truth about a
        // store that might be remote and encrypted. Say only what is known.
        Err(_) if names_postgres_scheme(address) => "encryption could not be read".to_owned(),
        Err(_) => "not encrypted, this machine only".to_owned(),
    }
}

/// Whether an address names the Postgres scheme this store opens.
///
/// The parser refuses an address for two different reasons: it is not a
/// Postgres address at all (a SQLite path), or it is a Postgres address this
/// store cannot read. The two must not read the same way.
fn names_postgres_scheme(address: &str) -> bool {
    let address = address.trim_start();
    address.starts_with("postgres://") || address.starts_with("postgresql://")
}

#[cfg(test)]
#[path = "connection_text_tests.rs"]
mod tests;
