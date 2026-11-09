// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for a non-activated candidate guard that decides which HTTP verb the candidate endpoint accepts: `POST` and no other verb.
//! A verb it does not accept is refused with status 405 and an `Allow: POST` header, and the refusal carries no body, so no protocol result is ever produced for it.
//!
//! It also holds the candidate endpoint that runs the guard before the existing POST routing: the routing arrives as a continuation, which only an accepted verb calls, and whose result is returned unchanged.
//!
//! ## Owns
//! - The one accepted verb, the `Allow` header name, and the 405 status
//! - The refusal shape a refused verb is answered with: the status, the headers, and the empty body
//! - The decision over one HTTP verb
//! - The candidate endpoint that guards a request before the POST continuation runs, and the result shape it answers with
//!
//! ## Does not own
//! - Reading a request and parsing its verb off the request line (see `http_read`)
//! - The live route and its response writing (see `http`); this module only names the status and the headers a refused verb is answered with
//! - The POST routing itself: it arrives as the endpoint's continuation and is never copied here
//! - The JSON-RPC envelope and any result payload (see `protocol`); a refusal here never reaches that layer
//! - Activation: only this crate's tests compile and reach this module
//!
//! ## Boundary
//!
//! Changes to which verb the candidate endpoint accepts, to the shape of the refusal, or to the order of the guard and the POST continuation should normally require changes here.
//! Changes to routing, request reading, or a tool should not require changes here.
//!
//! Keep this module focused on the one verb decision, its refusal shape, and the endpoint that composes them with the POST continuation.

/// The one HTTP verb the candidate endpoint accepts.
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

/// Decide one HTTP verb: `Ok(())` when the candidate endpoint accepts it.
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

/// The candidate refusal for one HTTP verb, or `None` for the accepted verb.
///
/// `None` is a request the route goes on to serve, so a caller can answer a refused verb with the status and headers it carries and nothing else.
#[must_use]
pub(crate) fn refuse(method: &str) -> Option<Refusal> {
    check_method(method).err()
}

/// The result one route answers with: the status, the body, and the extra headers.
///
/// This is the live route's own result tuple, so the endpoint can pass a
/// continuation's result through untouched.
pub(crate) type Response = (u16, String, Vec<(String, String)>);

/// Answer one request: refuse every verb but the accepted one, and pass the
/// accepted verb through to the POST continuation.
///
/// `post` is the existing POST routing, borrowed as the continuation the
/// endpoint calls; it runs only for [`ACCEPTED_METHOD`], exactly once, and the
/// [`Response`] it answers is returned unchanged.
/// Every other verb is answered with [`REFUSAL`] converted to a [`Response`],
/// so its body is empty, its only header is `Allow: POST`, and no protocol
/// parser has run.
#[must_use]
pub(crate) fn endpoint<F>(method: &str, post: F) -> Response
where
    F: FnOnce() -> Response,
{
    let Some(refusal) = refuse(method) else {
        return post();
    };
    let headers = refusal
        .headers
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    (refusal.status, refusal.body.to_owned(), headers)
}

#[cfg(test)]
#[path = "method_guard_tests.rs"]
mod method_guard_tests;
