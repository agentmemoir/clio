// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio help` command catalog: one entry per
//! command, carrying the command's flags and a short description.
//!
//! ## Owns
//! - The catalog's command list and each command's description
//! - The renderer that lays an entry out: wrapped usage line, tool arrow, note
//!
//! ## Does not own
//! - The usage lines themselves (see `cli_help_usage`), which every command's own
//!   `--help` also reads
//! - The help layout around the catalog (see the binary `help_text`)
//! - The machine-readable `help --json` catalog and the command suggestions
//!   (see `cli_help`)
//!
//! ## Boundary
//!
//! Changes to what a command is called or described should normally require changes
//! here. Adding or renaming a flag should not: the flag list is rendered from the
//! usage registry, so a flag added there appears here with no edit.
//!
//! Keep this module focused on the catalog text itself.

/// One catalog entry: the command, and what it is for.
///
/// The flags are deliberately absent. They come from [`crate::cli_help_usage`],
/// which is also where each command's own `--help` reads them. Two hand-written
/// lists of the same flags is how 159 flags came to exist in a command's help and
/// nowhere in the catalog, including `--source-type`, which a belief write needs.
const ENTRIES: &[(&str, &str)] = &[
    ("version", "Print package identity"),
    ("health", "Machine-readable health/version JSON"),
    (
        "mcp schema-export",
        "Print/write the tool_schema pack (alias: --path)",
    ),
    ("mcp stdio", "Run the MCP stdio transport"),
    (
        "mcp http",
        "Run the MCP Streamable HTTP transport; with no flag (or a bare --bind) scan 127.0.0.1:{range} for the first free port; an explicit HOST:PORT binds exactly that address",
    ),
    ("ops diagnose", "Read-only install/database health snapshot"),
    (
        "ops verify",
        "Referential-integrity and index-coverage checks",
    ),
    ("ops doctor", "Compose a repair plan (read-only)"),
    (
        "ops repair",
        "Apply allowlisted repairs (live run requires --confirm)",
    ),
    (
        "ops reindex",
        "Rebuild derived indexes (live run requires --confirm); a lexical rebuild replaces legacy plaintext index rows with keyed term digests. Ops exit codes: 0 healthy or completed, 1 domain/runtime failure or findings, 2 usage or repair refusal, 3 reindex failed",
    ),
    (
        "status",
        "Read-only unified stack health (setup, database, listeners, Compose, diagnose). Exit codes: 0 healthy, 1 degraded, 2 usage",
    ),
    (
        "compose up",
        "Materialize the compose stack and start the local Docker services (postgres, embed, rerank, extract)",
    ),
    (
        "compose down",
        "Stop and remove the services `clio compose up` started; re-runnable",
    ),
    (
        "setup",
        "Configure the stack and write the deployment config the binary reads (see `clio setup --help` for the full flag set)",
    ),
    (
        "retention schema",
        "Print the retention profile JSON Schema",
    ),
    (
        "retention get",
        "Show the effective per-bank retention profile",
    ),
    (
        "retention set",
        "Store a validated retention profile (persists to --profiles/env)",
    ),
    (
        "recall",
        "score floors are relative filters over this query's own scores, not calibrated thresholds",
    ),
    ("get", "fetch one memory by id"),
    ("show", "alias of `get`"),
    ("inspect", "metadata-only listing, no decrypted body"),
    (
        "stats",
        "count memories, entities, and index state in one bank",
    ),
    ("task get", "one task and its current state"),
    ("task history", "a task's recorded transitions"),
    ("failure list", "failures recorded against one task"),
    ("memtree query", "walk the memory tree from a seed"),
    ("memtree get", "one memory-tree node and its lineage"),
    (
        "history temporal",
        "the item history as it stood at a point in time",
    ),
    ("triple query", "match subject/predicate/object patterns"),
    ("belief history", "one belief's confidence history"),
    ("graph query", "neighbours of one item, by edge type"),
    ("associations", "co-activation links for one item"),
    ("persona get", "the persona model, within a token budget"),
    ("persona stable", "set a stable persona key"),
    (
        "persona observe",
        "feed one numeric observation into the EMA",
    ),
    ("task upsert", "create or update one task"),
    (
        "failure record",
        "record one attempt, its failure, and the lesson",
    ),
    (
        "scratchpad write",
        "store an ephemeral key for this session",
    ),
    ("scratchpad read", "read one key, or the whole pad"),
    ("scratchpad clear", "drop one key, or the whole pad"),
    (
        "canonical put",
        "gated single-slot upsert with bi-temporal supersession",
    ),
    ("canonical get", "read one canonical key"),
    ("shared store", "write into a shared bank"),
    ("shared retrieve", "read from a shared bank"),
    ("shared discard", "remove one item from a shared bank"),
    ("validate", "attest, update, invalidate, or delete one item"),
    ("batch", "apply a list of operations in one transaction"),
    (
        "maintenance status",
        "queue depth, failures, and drain progress",
    ),
    ("intent gate", "decide whether a turn should reach memory"),
    (
        "compose-context",
        "assemble a budgeted context block for a turn",
    ),
    (
        "audit trail",
        "masked lifecycle reconstruction for one item",
    ),
    ("hygiene audit", "score stored items and list the weakest"),
    (
        "hygiene clean",
        "(or --file - for stdin; without --confirm, or with --dry-run: zero-write dry-run only)",
    ),
    ("hygiene log", "what hygiene has already done"),
    (
        "export",
        "(manifest-backed JSON bundle; requires --actor harness|system)",
    ),
    ("import", "load a bundle produced by `clio export`"),
    (
        "erase",
        "IRREVERSIBLE compliance erasure (DEK destruction); distinct from `discard` and hygiene cleanup",
    ),
    ("remember", ""),
    (
        "admit",
        "score one write without storing it; `--file ops.json` previews a whole batch instead",
    ),
    ("summarize", "summarize one scope into a consolidated unit"),
    ("consolidate", "run the consolidation wave over a bank"),
    ("update", "set an item's value under a named rule"),
    ("invalidate", "close an item's validity; never deletes"),
    (
        "discard",
        "operational removal; distinct from compliance erasure",
    ),
    ("correct", "replace an item's value with a stated reason"),
    ("triple add", "write one subject/predicate/object fact"),
    ("triple end", "close an open fact without a replacement"),
    ("belief observe", "record evidence and a new confidence"),
    ("graph link", "add an explicit edge between two items"),
    ("config get", "read one configuration path"),
    ("config set", "write one configuration path at a scope"),
    (
        "config profiles",
        "list the available configuration profiles",
    ),
    ("config use", "apply one profile by name"),
    ("ranking get", "read the ranking environment"),
    ("ranking set", "patch the ranking environment"),
    (
        "sync status",
        "explain what sync would do, and omit when disabled",
    ),
    ("sync push", "send local changes to a sync server"),
    ("sync pull", "take remote changes from a sync server"),
    ("sync serve", "run the sync server in the foreground"),
    (
        "sync keygen",
        "Create the sync key in the recorded store and record the choice; the key is printed exactly once and cannot be recovered except with `clio sync key export`",
    ),
    (
        "sync key export",
        "Print the stored sync key; a shell flag or environment variable leaves the key in process listings, so a shared deployment should point CLIO_SYNC_KEY_FILE at an owner-only key file",
    ),
    (
        "sync key status",
        "Show the sync key's store, fingerprint, creation time, and count; never the key",
    ),
    (
        "backup dir",
        "Show and create the backup directory (no database needed)",
    ),
    (
        "backup create",
        "Snapshot the store into one owner-only archive: a SQLite store is a compacted, transactionally consistent copy (one VACUUM INTO, logically equivalent, never a byte-for-byte clone, and it does not retain deleted pages); a Postgres store is a row-level dump in COPY text format, whose schema is owned by this build; --keep N then prunes the backup directory to the newest N (nothing without --confirm); exit 3 means pruning could not remove every candidate",
    ),
    (
        "backup list",
        "List snapshots newest first by manifest time (no database needed); --db and --backend are refused because they cannot change the listing",
    ),
    (
        "backup prune",
        "Delete all but the newest N snapshots by manifest time: the newest snapshot is never deleted, an unreadable archive is reported and kept, and without --confirm nothing is removed. Exit codes: 0 report-only or complete prune, 3 confirmed prune could not remove every candidate",
    ),
    (
        "backup restore",
        "Replace the selected store with the archive (never a merge); a pre-restore snapshot is taken first unless --no-pre-backup; for a SQLite store the swap removes the target's write-ahead log before it renames the new store in, so a crash in that window loses whatever that log had not yet written; a Postgres restore replaces the target in one transaction and has no such window; --no-pre-backup is what leaves that window with no way back. Exit codes: 0 clean restore, 1 operational failure, 2 refused, 3 partial or not-durable restore",
    ),
    (
        "backup verify",
        "Re-check one archive's digests and its payload's validity; --db and --backend are refused because they cannot change the verdict",
    ),
    (
        "auth token add",
        "Generate a network credential, print its secret exactly once, and store only the digest in the owner-only token store; the secret cannot be recovered",
    ),
    (
        "auth token list",
        "Show labels, actors, banks, and created_at; never a secret or a digest",
    ),
    (
        "auth token revoke",
        "Remove the record; takes effect at the next server start",
    ),
    (
        "install",
        "Register the clio MCP server with a supported agent harness (see `clio install --help` for the supported harnesses)",
    ),
    (
        "uninstall",
        "Remove the clio MCP server registration from a supported agent harness (see `clio uninstall --help` for the supported harnesses)",
    ),
    (
        "help",
        "Global flags: --db --backend --bank --actor --output (-o). Output mode: an explicit `--output`/`-o` always wins; otherwise text on a terminal (TTY) and JSON when piped. Forms: `--output json`, `--output=json`, `-o json`, `-o=json`. Empty values: a value flag given an empty value (`--bank ''`) is refused on every command; leave the flag off to take the default. Use -- after the command before positional text that starts with -; keep flags before it. Exit codes: 0 ok, 1 domain or runtime failure, 2 usage error or documented command refusal (ops repair or backup restore), 3 is a command-specific degraded outcome (clio ops reindex, backup prune or backup create --keep, or backup restore). Process stops caused by SIGPIPE, SIGINT, or SIGTERM cannot report a CLI error; Unix shells show these as 141, 130, or 143, outside the command exit codes",
    ),
];

/// Every command the catalog lists, in the order it lists them.
///
/// The help catalog and the tests that check the flag lists agree because both
/// read this one list.
#[cfg(test)]
pub(crate) fn catalog_commands() -> impl Iterator<Item = &'static str> {
    ENTRIES.iter().map(|(command, _)| *command)
}

/// The catalog text `clio help` prints under its `Usage:` heading.
///
/// Every entry's flag list is rendered from the usage registry, so this function
/// and `clio <command> --help` cannot disagree about which flags a command takes.
/// A command with no registered usage line says so rather than printing a line
/// with no flags, because a silently empty list reads as "this command takes no
/// flags" and is believed.
pub(crate) fn catalog_text() -> String {
    let mut out = String::new();
    for (command, note) in ENTRIES {
        let mut usage = crate::cli_help_surfaces::wrapped_usage(command, USAGE_WIDTH, LEAD);
        if let Some(last) = usage.last_mut()
            && let Some(tool) = crate::cli_help::bound_tool_name(command)
        {
            last.push_str(ARROW);
            last.push_str(tool);
        }
        for line in &usage {
            out.push_str(line);
            out.push('\n');
        }
        for line in wrap(note, NOTE_WIDTH - NOTE_INDENT.len()) {
            out.push_str(NOTE_INDENT);
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// The indent every catalog line starts at, so the `clio` words line up.
const LEAD: usize = 2;
/// The column the catalog wraps its usage lines at.
const USAGE_WIDTH: usize = 84;
/// The column the catalog wraps its description lines at, indent included.
const NOTE_WIDTH: usize = 92;
/// The indent a description line carries, aligning it under the command's flags.
const NOTE_INDENT: &str = "       ";
/// The gap between the last flag of a command line and its tool name.
const ARROW: &str = "   -> ";

/// `text` wrapped to `width`, with no indent applied.
///
/// A word longer than `width` is never split: half a flag name names a flag that
/// does not exist.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        if word.is_empty() {
            continue;
        }
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && candidate.len() > width {
            lines.push(std::mem::replace(&mut current, word.to_owned()));
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
#[path = "command_catalog_tests.rs"]
mod tests;
