// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the ten `compliance`
//! subject-request and peer-request verbs: `request-start`, `request-status`,
//! `request-retry`, `request-copy`, `request-exception`, `request-events`,
//! `request-close`, `peer-status`, `peer-retry`, and `peer-disposition`.
//!
//! It is the sibling the `cli_command_registry_compliance` module declares and
//! re-exports, so `COMPLIANCE_LEAVES` can name these records while each half of
//! the tranche stays within the size cap.
//!
//! ## Owns
//! - The ten `CommandSpec` records and the flags only they use
//! - The description, value kind, and default of each documented flag
//!
//! ## Does not own
//! - The `compliance inventory` and `compliance review-export` records and the
//!   tranche list (see `cli_command_registry_compliance`)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The request dispatch, decoding, and views (see `compliance_request_cli`)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags or summary text should normally
//! require changes here. Changes to the command implementations should not.
//!
//! Keep this module focused on the compliance request records.

use crate::cli_command_registry::{GLOBAL_ACTOR, GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

// The request records' flags: each maps one CLI flag to one tool argument.
const REQUEST_UNPRINTED: &[&str] = &["db", "backend", "no-input", "yes"];
const REQUEST_NOTE: &str = "drives one durable subject request through the in-process subject-request tool, so the CLI and the MCP surface share one request path";
const REQUEST_ID: Flag = Flag {
    required: true,
    ..Flag::text("request-id", "ID", "The durable request id.")
};
const REQUEST_KIND: Flag = Flag {
    required: true,
    ..Flag::choice(
        "kind",
        "copy|erasure",
        "The request kind the id binds to.",
        &["copy", "erasure"],
    )
};
const REQUEST_SUBJECT: Flag = Flag::text(
    "subject-id",
    "SUBJECT",
    "The covered data subject; omit for a whole bank.",
);
const REQUEST_BASIS: Flag =
    Flag::text("legal-basis", "BASIS", "The legal basis a request records.");
const RETRY_SUBJECT: Flag = Flag {
    required: true,
    ..Flag::text("subject-id", "SUBJECT", "The covered data subject.")
};
const RETRY_BASIS: Flag = Flag {
    required: true,
    ..Flag::text("legal-basis", "BASIS", "The legal basis a retry records.")
};
const REQUEST_REASON_CODE: Flag = Flag::text("reason-code", "CODE", "The machine reason code.");
const REQUEST_CONFIRM: Flag = Flag::toggle(
    "confirm",
    "Perform the retry; without it the tool refuses and never mutates.",
);
const REQUEST_REASON: Flag = Flag {
    required: true,
    ..Flag::text("reason", "REASON", "The retention exception reason.")
};
const REQUEST_LIMIT: Flag = Flag::uint(
    "limit",
    "N",
    "Bounds the review page; the tool default applies when omitted.",
);
const REQUEST_CURSOR: Flag = Flag::text(
    "cursor",
    "C",
    "Resumes the review after this decimal sequence position.",
);
const REQUEST_PRODUCT: Flag = Flag {
    required: true,
    ..Flag::text("product-path", "PATH", "The covered product path.")
};
const COPY_ID: Flag = Flag {
    required: true,
    ..Flag::text("copy-id", "ID", "The stable copy id.")
};
const COPY_CUSTODY: Flag = Flag {
    required: true,
    ..Flag::choice(
        "custody",
        "CLASS",
        "The copy custody class.",
        &[
            "clio",
            "operator",
            "caller",
            "peer",
            "review_output",
            "unknown",
        ],
    )
};
const COPY_OWNER: Flag = Flag {
    required: true,
    ..Flag::choice(
        "owner",
        "harness|system",
        "The copy disposition owner.",
        &["harness", "system"],
    )
};
const COPY_ACTION: Flag = Flag {
    required: true,
    ..Flag::choice(
        "required-action",
        "erase|remove|delivery|notice",
        "The action the copy requires.",
        &["erase", "remove", "delivery", "notice"],
    )
};
const COPY_STATE: Flag = Flag {
    required: true,
    ..Flag::text("state", "STATE", "The copy state, including the outcome.")
};
const COPY_EXCEPTION: Flag = Flag::text(
    "exception-reason",
    "REASON",
    "Why the copy has no request result yet.",
);
const COPY_UPDATED: Flag = Flag::text(
    "updated-at",
    "TIME",
    "The last update time (RFC 3339 UTC; defaults to now).",
);
const PEER_ID: Flag = Flag {
    required: true,
    ..Flag::text("peer-id", "PEER", "The peer whose durable row is disposed.")
};
const PEER_DISPOSITION: Flag = Flag {
    required: true,
    ..Flag::choice(
        "disposition",
        "DISPOSITION",
        "The peer disposition to record.",
        &["removal_evidence", "retention_approved", "unknown"],
    )
};
const PEER_PROOF: Flag = Flag::text("proof-ref", "REF", "The peer's durable proof reference.");

const fn request_spec(
    name: &'static str,
    summary: &'static str,
    documented: &'static [Flag],
    examples: &'static [Example],
    tool: &'static str,
    section: Section,
) -> CommandSpec {
    CommandSpec {
        name,
        aliases: &[],
        summary,
        notes: &[REQUEST_NOTE],
        positionals: &[],
        documented,
        accepted_extra: REQUEST_UNPRINTED,
        flag_aliases: &[],
        usage_override: None,
        usage_alternates: &[],
        examples,
        tool: Some(tool),
        section,
    }
}

pub(crate) const COMPLIANCE_REQUEST_START: CommandSpec = request_spec(
    "compliance request-start",
    "Open one durable subject request, or replay the stored one of the same id, and report its separated status",
    &[
        REQUEST_ID,
        REQUEST_KIND,
        REQUEST_SUBJECT,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance request-start --request-id r1 --kind erasure",
            None,
        ),
        Example::new(
            "clio compliance request-start --request-id r1 --kind copy -o json",
            None,
        ),
    ],
    "subject_request_start",
    Section::Write,
);

pub(crate) const COMPLIANCE_REQUEST_STATUS: CommandSpec = request_spec(
    "compliance request-status",
    "Report one durable request's state, technical completion, pending operator actions, and outcome as separate fields",
    &[REQUEST_ID, GLOBAL_BANK, GLOBAL_ACTOR, GLOBAL_OUTPUT],
    &[
        Example::new("clio compliance request-status --request-id r1", None),
        Example::new(
            "clio compliance request-status --request-id r1 -o json",
            None,
        ),
    ],
    "subject_request_status",
    Section::Read,
);

pub(crate) const COMPLIANCE_REQUEST_RETRY: CommandSpec = request_spec(
    "compliance request-retry",
    "Retry a confirmed erasure through its durable request id and report the pending action and effect counts",
    &[
        REQUEST_ID,
        RETRY_SUBJECT,
        RETRY_BASIS,
        REQUEST_REASON_CODE,
        REQUEST_CONFIRM,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance request-retry --request-id r1 --subject-id s1 --legal-basis gdpr-art-17 --confirm --actor harness",
            None,
        ),
        Example::new(
            "clio compliance request-retry --request-id r1 --subject-id s1 --legal-basis gdpr-art-17 --confirm --actor harness -o json",
            None,
        ),
    ],
    "subject_request_retry",
    Section::Write,
);

pub(crate) const COMPLIANCE_REQUEST_COPY: CommandSpec = request_spec(
    "compliance request-copy",
    "Record one copy-inventory disposition for a request and report the request",
    &[
        COPY_ID,
        REQUEST_ID,
        COPY_CUSTODY,
        COPY_OWNER,
        COPY_ACTION,
        COPY_STATE,
        COPY_EXCEPTION,
        COPY_UPDATED,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance request-copy --copy-id c1 --request-id r1 --custody clio --owner harness --required-action erase --state pending",
            None,
        ),
        Example::new(
            "clio compliance request-copy --copy-id c1 --request-id r1 --custody peer --owner system --required-action notice --state open -o json",
            None,
        ),
    ],
    "subject_request_copy",
    Section::Write,
);

pub(crate) const COMPLIANCE_REQUEST_EXCEPTION: CommandSpec = request_spec(
    "compliance request-exception",
    "Record a request-level retention exception and report the request",
    &[
        REQUEST_ID,
        REQUEST_REASON,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance request-exception --request-id r1 --reason retention-hold",
            None,
        ),
        Example::new(
            "clio compliance request-exception --request-id r1 --reason retention-hold -o json",
            None,
        ),
    ],
    "subject_request_exception",
    Section::Write,
);

pub(crate) const COMPLIANCE_REQUEST_EVENTS: CommandSpec = request_spec(
    "compliance request-events",
    "Read one content-free page of the local events bound to one request",
    &[
        REQUEST_ID,
        REQUEST_LIMIT,
        REQUEST_CURSOR,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance request-events --request-id r1 --limit 50",
            None,
        ),
        Example::new(
            "clio compliance request-events --request-id r1 --cursor 10 -o json",
            None,
        ),
    ],
    "subject_request_events",
    Section::Read,
);

pub(crate) const COMPLIANCE_REQUEST_CLOSE: CommandSpec = request_spec(
    "compliance request-close",
    "Accept closure for one request, finalize its outcome, and report the request",
    &[
        REQUEST_ID,
        REQUEST_PRODUCT,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance request-close --request-id r1 --product-path ./product.db",
            None,
        ),
        Example::new(
            "clio compliance request-close --request-id r1 --product-path ./product.db -o json",
            None,
        ),
    ],
    "subject_request_close",
    Section::Write,
);

pub(crate) const COMPLIANCE_PEER_STATUS: CommandSpec = request_spec(
    "compliance peer-status",
    "Report the derived peer state slots of one request from its durable per-peer rows",
    &[REQUEST_ID, GLOBAL_BANK, GLOBAL_ACTOR, GLOBAL_OUTPUT],
    &[
        Example::new("clio compliance peer-status --request-id r1", None),
        Example::new("clio compliance peer-status --request-id r1 -o json", None),
    ],
    "subject_request_peer_status",
    Section::Read,
);

pub(crate) const COMPLIANCE_PEER_RETRY: CommandSpec = request_spec(
    "compliance peer-retry",
    "Reconcile the pending erasure actions a restart left on one request and report it",
    &[
        REQUEST_ID,
        REQUEST_BASIS,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance peer-retry --request-id r1 --legal-basis OPERATOR_RETRY",
            None,
        ),
        Example::new("clio compliance peer-retry --request-id r1 -o json", None),
    ],
    "subject_request_peer_retry",
    Section::Write,
);

pub(crate) const COMPLIANCE_PEER_DISPOSITION: CommandSpec = request_spec(
    "compliance peer-disposition",
    "Record one peer disposition for a request and report the request and its peer states",
    &[
        REQUEST_ID,
        PEER_ID,
        PEER_DISPOSITION,
        PEER_PROOF,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    &[
        Example::new(
            "clio compliance peer-disposition --request-id r1 --peer-id p1 --disposition removal_evidence --proof-ref ref-1",
            None,
        ),
        Example::new(
            "clio compliance peer-disposition --request-id r1 --peer-id p1 --disposition retention_approved -o json",
            None,
        ),
    ],
    "subject_request_peer_disposition",
    Section::Write,
);
