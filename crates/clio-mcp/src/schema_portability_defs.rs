// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! JSON portability tools (`export`, `import`) and the `import_provider`
//! first-release stub, published in the same `tool_schema` pack.
//!
//! ## Owns
//! - One `inputSchema` per portability tool
//!
//! ## Does not own
//! - Core tool definitions (see `schema_defs`)
//! - Tool dispatch or handler semantics
//!
//! ## Boundary
//!
//! Changes to a portability tool's parameter contract should normally require
//! changes here. Changes to core schemas should not require changes here.
//!
//! Keep this module focused on portability schema definitions.

use crate::schema_defs::{ToolDef, allow_duplicate_prop, b, s};

/// All portability tool definitions.
#[must_use]
pub fn portability_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "export",
            description: "Write one JSON memory_export_bundle with a completeness_manifest \
                          (version, counts, checksums, filter, complete flag). \
                          content_mode defaults to dsar_plaintext (authorized readable export); \
                          ciphertext_backup emits opaque bodies without DEKs. Requires a \
                          harness/system actor; no DEKs, API keys, or sync credentials in the file.",
            required: &["destination"],
            props: vec![
                ("destination", s("File path for the bundle JSON.")),
                (
                    "content_mode",
                    s("dsar_plaintext (default) | ciphertext_backup."),
                ),
                (
                    "category",
                    s("Semantic category filter (narrowed exports mark complete=false)."),
                ),
                ("time_start", s("created_at lower bound (inclusive).")),
                ("time_end", s("created_at upper bound (exclusive).")),
                (
                    "include_archived",
                    b("Include hygiene-archived rows (default true)."),
                ),
                (
                    "include_discarded",
                    b("Include operationally discarded rows (default true)."),
                ),
            ],
        },
        ToolDef {
            name: "import",
            description: "Idempotent, manifest-validated JSON import of a prior export. \
                          Re-importing the same bundle skips everything (would_skip); \
                          force=true overwrites differing rows with an auditable reason. \
                          Every new long-term row passes the admission gates; \
                          dry_run reports counts without writing.",
            required: &["source"],
            props: vec![
                ("source", s("File path of the bundle to import.")),
                (
                    "dry_run",
                    b("Report would-create/skip/reject/overwrite without writing."),
                ),
                (
                    "force",
                    b("Overwrite differing existing rows (requires reason)."),
                ),
                ("reason", s("Audit reason required when force=true.")),
                allow_duplicate_prop(),
            ],
        },
        ToolDef {
            name: "import_provider",
            description: "Provider importer: import memories from external memory providers \
                          (Mem0 supported; other providers return PROVIDER_IMPORT_UNSUPPORTED). \
                          Dry-run report precedes writes. Credentials are masked in all output.",
            required: &["provider"],
            props: vec![
                (
                    "provider",
                    s("Provider name (mem0 | hindsight | mnemosyne | honcho | supermemory)."),
                ),
                ("credentials", {
                    serde_json::json!({
                        "type": "object",
                        "description": "Provider credentials (masked in all logs and reports; never plaintext).",
                        "additionalProperties": true
                    })
                }),
                ("options", {
                    serde_json::json!({
                        "type": "object",
                        "description": "Provider options (api_base_url, user_id, agent_id, page_size, max_items).",
                        "additionalProperties": true
                    })
                }),
                (
                    "dry_run",
                    b("Plan and produce dry-run report with zero writes."),
                ),
                allow_duplicate_prop(),
            ],
        },
    ]
}
