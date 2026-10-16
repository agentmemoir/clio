// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! durable subject-request tools (`subject_request_*`), published in the same
//! `tool_schema` pack as the other tool families.
//!
//! ## Owns
//! - One `inputSchema` per subject-request and peer-request tool
//!
//! ## Does not own
//! - Core, portability, or other family definitions (see `schema_defs` and the
//!   sibling `schema_*_defs` modules)
//! - Tool dispatch or request semantics (`subject_request_tools`,
//!   `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to a request tool's parameter contract should normally require
//! changes here. Changes to request semantics should not require changes here.
//!
//! Keep this module focused on subject-request schema definitions.

use crate::schema_defs::{ToolDef, b, bank_actor, en, num, s};

/// All subject-request tool definitions.
#[must_use]
pub fn request_defs() -> Vec<ToolDef> {
    vec![
        start_def(),
        status_def(),
        retry_def(),
        copy_def(),
        exception_def(),
        events_def(),
        close_def(),
        peer_status_def(),
        peer_retry_def(),
        peer_disposition_def(),
    ]
}

/// `subject_request_start`: open or replay one request.
fn start_def() -> ToolDef {
    ToolDef {
        name: "subject_request_start",
        description: "Open one durable subject request, or replay the stored one of the \
                      same id and scope, and report its separated status. The request id \
                      binds immutably to the kind and scope.",
        required: &["request_id", "kind"],
        props: props(vec![
            ("request_id", s("Durable request id; the idempotency key.")),
            ("kind", en("Request kind.", &["copy", "erasure"])),
            (
                "subject_id",
                s("One data subject; omit for a whole-bank scope."),
            ),
        ]),
    }
}

/// `subject_request_status`: report one stored request.
fn status_def() -> ToolDef {
    ToolDef {
        name: "subject_request_status",
        description: "Report one stored request's state, technical completion, pending \
                      operator actions, and outcome as separate fields.",
        required: &["request_id"],
        props: props(vec![("request_id", s("Durable request id."))]),
    }
}

/// `subject_request_retry`: retry a confirmed erasure.
fn retry_def() -> ToolDef {
    ToolDef {
        name: "subject_request_retry",
        description: "Retry a confirmed erasure through its durable request id, then report \
                      the request. Requires a harness or system actor and confirm=true.",
        required: &["request_id", "subject_id", "legal_basis"],
        props: props(vec![
            ("request_id", s("Durable request id.")),
            ("subject_id", s("Data subject whose DEK is destroyed.")),
            ("legal_basis", s("Legal basis for the request.")),
            (
                "reason_code",
                s("Optional machine reason code (default COMPLIANCE_CRYPTO_SHRED)."),
            ),
            (
                "confirm",
                b("Perform the retry; default false (unconfirmed calls never mutate)."),
            ),
        ]),
    }
}

/// `subject_request_copy`: record one copy-inventory disposition.
fn copy_def() -> ToolDef {
    ToolDef {
        name: "subject_request_copy",
        description: "Record one copy-inventory disposition for a request and report it. \
                      Metadata only: no copy content, digest, key, or token.",
        required: &[
            "copy_id",
            "request_id",
            "custody",
            "owner",
            "required_action",
            "state",
        ],
        props: props(vec![
            ("copy_id", s("Stable copy id.")),
            ("request_id", s("Request the copy is attached to.")),
            (
                "custody",
                s("Custody class: clio | operator | caller | peer | review_output | unknown."),
            ),
            ("owner", s("Disposition owner: harness | system.")),
            (
                "required_action",
                s("Required action: erase | remove | delivery | notice."),
            ),
            ("state", s("Copy state, including the final outcome.")),
            (
                "exception_reason",
                s("Why the copy has no request result yet, when it is an open gap."),
            ),
            (
                "updated_at",
                s("Last update time (RFC 3339 UTC; defaults to now)."),
            ),
        ]),
    }
}

/// `subject_request_exception`: record a retention exception.
fn exception_def() -> ToolDef {
    ToolDef {
        name: "subject_request_exception",
        description: "Record a request-level retention exception and report the request; an \
                      approved exception yields a partial final outcome.",
        required: &["request_id", "reason"],
        props: props(vec![
            ("request_id", s("Durable request id.")),
            ("reason", s("The retention exception reason.")),
        ]),
    }
}

/// `subject_request_events`: read one page of a request's events.
fn events_def() -> ToolDef {
    ToolDef {
        name: "subject_request_events",
        description: "Read one content-free page of the local events bound to one request \
                      (sequence, action id, state) in ascending order.",
        required: &["request_id"],
        props: props(vec![
            ("request_id", s("Durable request id whose events are read.")),
            (
                "limit",
                num("Maximum rows to return (default 100, max 1000)."),
            ),
            ("cursor", s("Resume after this decimal sequence position.")),
        ]),
    }
}

/// `subject_request_close`: accept closure for one request.
fn close_def() -> ToolDef {
    ToolDef {
        name: "subject_request_close",
        description: "Accept closure for one request, finalize its outcome, and report the \
                      request. A request with an open required action is refused.",
        required: &["request_id", "product_path"],
        props: props(vec![
            ("request_id", s("Durable request id.")),
            (
                "product_path",
                s("The covered product path whose recorded slot closure reads."),
            ),
        ]),
    }
}

/// `subject_request_peer_status`: the derived peer slots of one request.
fn peer_status_def() -> ToolDef {
    ToolDef {
        name: "subject_request_peer_status",
        description: "Report the derived peer state slots of one request from its durable \
                      per-peer rows.",
        required: &["request_id"],
        props: props(vec![("request_id", s("Durable request id."))]),
    }
}

/// `subject_request_peer_retry`: reconcile pending erasure actions.
fn peer_retry_def() -> ToolDef {
    ToolDef {
        name: "subject_request_peer_retry",
        description: "Reconcile the pending erasure actions a restart left on one request \
                      and report the request; a settled peer action is proven by its durable \
                      evidence, never repeated.",
        required: &["request_id"],
        props: props(vec![
            ("request_id", s("Durable request id.")),
            (
                "legal_basis",
                s("Legal basis recorded on a resumed erasure (default OPERATOR_RETRY)."),
            ),
        ]),
    }
}

/// `subject_request_peer_disposition`: record one peer disposition.
fn peer_disposition_def() -> ToolDef {
    ToolDef {
        name: "subject_request_peer_disposition",
        description: "Record one peer disposition for a request and report the request and \
                      its peer states.",
        required: &["request_id", "peer_id", "disposition"],
        props: props(vec![
            ("request_id", s("Durable request id.")),
            ("peer_id", s("The peer whose row is disposed.")),
            (
                "disposition",
                en(
                    "Peer disposition.",
                    &["removal_evidence", "retention_approved", "unknown"],
                ),
            ),
            (
                "proof_ref",
                s("The peer's durable proof for the recorded state."),
            ),
        ]),
    }
}

/// Append the shared `bank`/`actor` selectors to a tool's own properties.
fn props(
    mut base: Vec<(&'static str, serde_json::Value)>,
) -> Vec<(&'static str, serde_json::Value)> {
    base.extend(bank_actor());
    base
}
