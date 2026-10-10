// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the label of the credential that presented a
//! request, and for writing it into an audited detail object.
//!
//! ## Owns
//! - `PrincipalLabel`, the non-secret operator-facing name of one credential
//! - `PRINCIPAL_DETAIL_KEY`, the detail key every audited object uses
//! - `record_principal`, the one place that adds or omits that key
//!
//! ## Does not own
//! - Where a label comes from, or which requests carry one (see the transport
//!   that resolved it)
//! - The audit tables themselves and their SQL
//! - Secret masking of any other detail key
//!
//! ## Boundary
//!
//! Changes to what a principal label is, or to the audited key that names it,
//! should normally require changes here. Changes to which operations are
//! audited should not require changes here.
//!
//! Keep this module focused on the principal label and its audited form.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Audited detail key naming the credential that presented the request.
///
/// Absent means no credential presented it, which is the ordinary case for a
/// local process: a local owner already holds the store, so there is nothing to
/// name. The key is never written with a null or an empty value, so a reader
/// cannot mistake an unbound operation for one whose label is unknown.
pub const PRINCIPAL_DETAIL_KEY: &str = "principal";

/// The non-secret label of one credential.
///
/// This is the operator-facing name an operator chose for a credential record.
/// It is not the secret and not a digest of the secret: it is the name under
/// which the secret is filed, and it is recorded so an audit row can say which
/// credential acted rather than only which role was claimed.
///
/// The inner text stays private so a label can only be built through
/// [`PrincipalLabel::new`], which keeps the audited form a plain string in one
/// place.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalLabel(String);

impl PrincipalLabel {
    /// Wrap an operator-chosen label.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The label text, as recorded.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrincipalLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Add the principal key to an audited detail object, or leave it untouched.
///
/// One call site per audited artifact, so the key name and the "absent, not
/// empty" rule are decided once. A detail that is not a JSON object cannot hold
/// a key and is returned unchanged rather than replaced: silently swapping a
/// caller's payload for a different object would lose whatever it carried.
///
/// The label is recorded as it was resolved. Nothing is derived from it here, so
/// no digest of a secret can reach a row through this path.
pub fn record_principal(detail: &mut Value, principal: Option<&PrincipalLabel>) {
    let Some(label) = principal else {
        return;
    };
    if let serde_json::Value::Object(fields) = detail {
        fields.insert(
            PRINCIPAL_DETAIL_KEY.to_owned(),
            Value::String(label.as_str().to_owned()),
        );
    }
}

#[cfg(test)]
#[path = "audit_principal_tests.rs"]
mod audit_principal_tests;
