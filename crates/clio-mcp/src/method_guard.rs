// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one HTTP verb decision the Streamable HTTP
//! transport makes: `POST` is served and no other verb is. A refused verb is
//! answered with status 405 and an `Allow: POST` header, and carries no body, so
//! no protocol result is ever produced for it.
//!
//! ## Owns
//! - The one accepted verb, the `Allow` header name, and the 405 status
//! - The refusal shape a refused verb is answered with: the status, the headers, and the empty body
//! - The decision over one HTTP verb
//!
//! ## Does not own
//! - Reading a request and parsing its verb off the request line (see `http_read`)
//! - The route and its response writing (see `http`); this module only names the status and the headers a refused verb is answered with
//! - The JSON-RPC envelope and any result payload (see `protocol`); a refusal here never reaches that layer
//!
//! ## Boundary
//!
//! Changes to which verb the endpoint accepts or to the shape of the refusal
//! should normally require changes here. Changes to routing, request reading, or
//! a tool should not require changes here.
//!
//! Keep this module focused on the one verb decision and its refusal shape.

/// The one HTTP verb this transport accepts.
pub(crate) const ACCEPTED_METHOD: &str = "POST";

/// Wire name of the header that names the verbs an endpoint accepts.
pub(crate) const ALLOW_HEADER: &str = "Allow";

/// The status a refused verb is answered with.
pub(crate) const METHOD_NOT_ALLOWED: u16 = 405;

/// The refusal one HTTP verb draws: the status, the headers, and the body.
///
/// The body is always empty, and the headers are the extra headers a route writes before the shared length and close headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// The HTTP status the route answers with.
    pub(crate) status: u16,
    /// The headers the route writes, in the order it writes them.
    pub(crate) headers: [(&'static str, &'static str); 1],
    /// The response body, which is always empty.
    pub(crate) body: &'static str,
}

/// The one refusal this module answers: 405, `Allow: POST`, and no body.
pub(crate) const REFUSAL: Refusal = Refusal {
    status: METHOD_NOT_ALLOWED,
    headers: [(ALLOW_HEADER, ACCEPTED_METHOD)],
    body: "",
};

/// Decide one HTTP verb: `Ok(())` when the transport accepts it.
///
/// The verb is the only input, so the decision is reached from the request line alone and no body or protocol parser runs before it.
/// The caller reads the verb from that line and asks this guard first, so a request it refuses never reaches the JSON-RPC layer.
/// The request reader upper-cases the verb it parses, so the comparison is exact.
///
/// # Errors
/// The verb is not [`ACCEPTED_METHOD`], which answers [`REFUSAL`].
pub(crate) fn check_method(method: &str) -> Result<(), Refusal> {
    if method == ACCEPTED_METHOD {
        Ok(())
    } else {
        Err(REFUSAL)
    }
}

/// The refusal for one HTTP verb, or `None` for the accepted verb.
///
/// `None` is a request the route goes on to serve, so a caller can answer a refused verb with the status and headers it carries and nothing else.
#[must_use]
pub(crate) fn refuse(method: &str) -> Option<Refusal> {
    check_method(method).err()
}

#[cfg(test)]
#[path = "method_guard_tests.rs"]
mod method_guard_tests;
