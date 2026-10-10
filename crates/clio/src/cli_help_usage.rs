// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-command CLI usage strings shown by
//! `clio <verb> --help` and by the help catalog.
//!
//! ## Owns
//! - The command → usage-line table (`VERB_USAGE`) and its lookup
//! - `usage_line`, the one flag list every command's `--help` and the catalog read
//!
//! The commands that bind no MCP tool keep their lines in `cli_help_surfaces`, and
//! `usage_line` reads both so every caller has one lookup.
//!
//! ## Does not own
//! - The command → MCP tool bindings or the `help --json` catalog (see `cli_help`)
//! - Argument parsing (see `cli_args`) or help rendering (see `cli_output`)
//! - The catalog's description text (see `command_catalog`)
//!
//! ## Boundary
//!
//! Changes to a command's usage line should normally require changes here.
//! Changes to a tool's schema should not require changes here.
//!
//! Keep this module focused on the CLI usage strings.

/// Per-command usage lines (command, usage); `cli_help::verb_help` looks one up.
///
/// The lines keep the canonical long `--output` spelling. The `-o` alias is a
/// global flag documented once in the top-level help catalog, not repeated here.
///
/// **One entry per verb, enforced by a test.** `usage_for` returns the first
/// match, so a second entry under the same key is text no command can print. A
/// maintainer who rewrites it sees no change and no diagnostic. A duplicate is
/// merged into the entry that is reachable rather than left to rot.
const VERB_USAGE: &[(&str, &str)] = &[
    (
        "recall",
        "usage: clio recall QUERY [--bank BANK] [--limit N] [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--min-semantic N] [--min-keyword N] [--min-reranker N] [--min-final N] [--no-entities] [--entity-summary] [--output json|text]\n       a score floor is a relative filter over this query's own scores, not a calibrated relevance threshold; --min-semantic and --min-keyword prune one retrieval arm each, and --no-entities cannot be combined with --entity-summary; --query-anchor must be RFC 3339 with an explicit UTC offset, and a relative phrase without an anchor contributes nothing (no server-time fallback)",
    ),
    (
        "get",
        "usage: clio get ID [--snapshot|--gist] [--output json|text]",
    ),
    (
        "show",
        "usage: clio show ID [--snapshot|--gist] [--output json|text]",
    ),
    (
        "inspect",
        "usage: clio inspect [--filter JSON] [--category C] [--epistemic-kind K] [--validity V] [--time-start TS] [--time-end TS] [--limit N] [--offset N] [--include-discarded] [--output json|text]",
    ),
    (
        "stats",
        "usage: clio stats [--bank BANK] [--output json|text]",
    ),
    (
        "task get",
        "usage: clio task get TASK_ID [--explain] [--output json|text]",
    ),
    (
        "task history",
        "usage: clio task history TASK_ID [--limit N] [--explain] [--output json|text]",
    ),
    (
        "failure list",
        "usage: clio failure list TASK_ID_OR_QUERY [--limit N] [--explain] [--output json|text]",
    ),
    (
        "memtree query",
        "usage: clio memtree query [--time-start TS] [--time-end TS] [--task-id ID] [--depth N] [--limit N] [--explain] [--output json|text]",
    ),
    (
        "memtree get",
        "usage: clio memtree get NODE_ID [--explain] [--output json|text]",
    ),
    (
        "history temporal",
        "usage: clio history temporal TARGET [--as-of TS] [--time-axis valid|transaction] [--explain] [--output json|text]",
    ),
    (
        "triple query",
        "usage: clio triple query [SUBJECT [PREDICATE [OBJECT]]] [--subject S] [--predicate P] [--object O] [--as-of TS] [--time-axis valid|transaction] [--explain] [--output json|text]",
    ),
    (
        "belief history",
        "usage: clio belief history BELIEF_ID_OR_PROPOSITION [--explain] [--output json|text]",
    ),
    (
        "graph query",
        "usage: clio graph query SEED_ID [--max-hops N] [--edge-type coactivation|explicit] [--min-weight W] [--explain] [--output json|text]",
    ),
    (
        "associations",
        "usage: clio associations ITEM_ID [--min-weight W] [--explain] [--output json|text]",
    ),
    (
        "persona get",
        "usage: clio persona get [--budget-tokens N] [--explain] [--output json|text]",
    ),
    (
        "maintenance status",
        "usage: clio maintenance status [--item-id ID] [--explain] [--output json|text]",
    ),
    (
        "intent gate",
        "usage: clio intent gate TURN_TEXT [--explain] [--output json|text]",
    ),
    (
        "compose-context",
        "usage: clio compose-context [QUERY] --budget-tokens N [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--recall-scope full|consolidated_only] [--expand-graph] [--prefer-consolidated] [--explain] [--output json|text]",
    ),
    (
        "audit trail",
        "usage: clio audit trail ITEM_ID [--explain] [--output json|text]",
    ),
    (
        "persona stable",
        "usage: clio persona stable KEY VALUE [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--context TEXT] [--dry-run] [--output json|text]",
    ),
    (
        "persona observe",
        "usage: clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--output json|text]",
    ),
    (
        "task upsert",
        "usage: clio task upsert TASK_ID --definition D --status S [--steps a,b] [--evidence-ref E] [--context TEXT] [--dry-run] [--output json|text]",
    ),
    (
        "failure record",
        "usage: clio failure record TASK_ID --attempt-n N --what-failed W --lesson L [--evidence-ref E] [--context TEXT] [--dry-run] [--output json|text]",
    ),
    (
        "scratchpad write",
        "usage: clio scratchpad write KEY CONTENT [--session S] [--bank B] [--actor A] [--output json|text]",
    ),
    (
        "scratchpad read",
        "usage: clio scratchpad read [KEY] [--session S] [--bank B] [--actor A] [--output json|text]",
    ),
    (
        "scratchpad clear",
        "usage: clio scratchpad clear [KEY] [--session S] [--bank B] [--actor A] [--output json|text]",
    ),
    (
        "canonical put",
        "usage: clio canonical put KEY VALUE --category C --epistemic-kind K [--source-type T] [--confidence N] [--source-text TEXT] [--evidence-ref E] [--context C] [--dry-run] [--output json|text]",
    ),
    (
        "canonical get",
        "usage: clio canonical get KEY [--as-of TS] [--output json|text]",
    ),
    (
        "shared store",
        "usage: clio shared store TEXT --epistemic-kind K [--category C] [--episodic-type T] [--source-type T] [--confidence N] [--evidence-ref E] [--source-text TEXT] --actor A --shared-bank [--output json|text]",
    ),
    (
        "shared retrieve",
        "usage: clio shared retrieve QUERY --actor A --shared-bank [--output json|text]",
    ),
    (
        "shared discard",
        "usage: clio shared discard ITEM_ID --reason R [--confirm] --actor A --shared-bank [--output json|text]",
    ),
    (
        "validate",
        "usage: clio validate ITEM_ID --action attest|update|invalidate|delete [--note N] [--new-content C] [--bank B] [--actor A] [--output json|text]",
    ),
    (
        "batch",
        "usage: clio batch --file ops.json [--dry-run] [--bank B] [--actor A] [--output json|text]\n       or: clio batch --file - (reads operations JSON from stdin)",
    ),
    (
        "remember",
        "usage: clio remember TEXT --category C [--bank BANK] [--actor ACTOR] [--epistemic-kind fact|belief] [--confidence N] [--source-type T] [--evidence-ref E] [--context TEXT] [--source-text TEXT] [--allow-duplicate] [--dry-run] [--output json|text]\n       --context records where the memory came from, such as a conversation type or workflow stage. It is not searchable and recall does not match on it, so put anything you want to find later in TEXT; it is at most 4096 UTF-8 bytes and is rejected when empty or oversized, never truncated",
    ),
    (
        "admit",
        "usage: clio admit TEXT --category C [--bank BANK] [--actor ACTOR] [--epistemic-kind fact|belief] [--confidence N] [--source-type T] [--evidence-ref E] [--context TEXT] [--output json|text]\n       or: clio admit --file ops.json [--bank BANK] [--actor ACTOR]\n       --context records where the memory came from, such as a conversation type or workflow stage. It is not searchable and recall does not match on it, so put anything you want to find later in TEXT; it is at most 4096 UTF-8 bytes and is rejected when empty or oversized, never truncated",
    ),
    (
        "summarize",
        "usage: clio summarize --scope S [--dry-run] [--output json|text]",
    ),
    (
        "consolidate",
        "usage: clio consolidate [--bank BANK] [--force] [--dry-run] [--output json|text]\n       --bank selects the target bank for the maintenance wave; --force runs the full wave: refresh every dirty ancestor, then reconcile the bank's consolidated units",
    ),
    (
        "update",
        "usage: clio update ID VALUE --rule discrete|continuous [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
    ),
    (
        "invalidate",
        "usage: clio invalidate ID [--replacement ID] [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
    ),
    (
        "discard",
        "usage: clio discard ID --reason R --confirm [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
    ),
    (
        "correct",
        "usage: clio correct ID VALUE --reason R [--context TEXT] [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
    ),
    (
        "triple add",
        "usage: clio triple add S P O [--epistemic-kind fact|belief] [--source-type T] [--confidence N] [--context TEXT] [--valid-from TS] [--valid-until TS] [--supersede true|false] [--subject-id ID] [--update-rule discrete] [--dry-run] [--output json|text]",
    ),
    (
        "triple end",
        "usage: clio triple end S P [--object O] --valid-until TS [--update-rule discrete] [--dry-run] [--output json|text]",
    ),
    (
        "belief observe",
        "usage: clio belief observe PROP --confidence C --evidence E --source S [--as-of TS] [--subject-id ID] [--context TEXT] [--dry-run] [--output json|text]",
    ),
    (
        "graph link",
        "usage: clio graph link A B --relationship R [--weight W] [--dry-run] [--output json|text]",
    ),
    (
        "hygiene audit",
        "usage: clio hygiene audit [--min-score N] [--limit N] [--offset N] [--bank BANK] [--output json|text]",
    ),
    (
        "hygiene clean",
        "usage: clio hygiene clean --file candidates.json [--action flag|archive|discard|keep] [--confirm] [--dry-run] [--reason R] [--note N] [--bank BANK] [--output json|text]\n       or: clio hygiene clean --file - (reads the candidates JSON from stdin)\n       without --confirm (or with --dry-run) the call is a zero-write dry-run: counts only",
    ),
    (
        "hygiene log",
        "usage: clio hygiene log [--limit N] [--offset N] [--bank BANK] [--output json|text]",
    ),
    (
        "export",
        "usage: clio export --out FILE [--content-mode dsar_plaintext|ciphertext_backup] [--category C] [--time-start TS] [--time-end TS] [--include-archived true|false] [--include-discarded true|false] [--bank BANK] [--actor ACTOR] [--output json|text]",
    ),
    (
        "import",
        "usage: clio import --source FILE [--dry-run] [--force] [--reason R] [--bank BANK] [--output json|text]",
    ),
    (
        "erase",
        "usage: clio erase --subject S --basis B --request R [--reason-code C] --confirm [--actor harness|system] [--bank BANK] [--output json|text]\n       compliance erasure is IRREVERSIBLE (DEK destruction) and is distinct from `discard` and hygiene cleanup",
    ),
    (
        "config get",
        "usage: clio config get [PATH] [--output json|text]",
    ),
    (
        "config set",
        "usage: clio config set PATH VALUE [--scope session|profile|deployment] [--output json|text]",
    ),
    (
        "config profiles",
        "usage: clio config profiles [--output json|text]",
    ),
    (
        "config use",
        "usage: clio config use NAME [--output json|text]",
    ),
    (
        "ranking get",
        "usage: clio ranking get [--output json|text]",
    ),
    (
        "ranking set",
        "usage: clio ranking set --patch JSON [--patch-file FILE] [--dry-run] [--output json|text]",
    ),
    (
        "sync status",
        "usage: clio sync status [--banks a,b] [--output json|text]\n       on a single-host deployment (sync.omit=true) this explains the omission and exits 0\n       after a key rotation, a peer that has not been given the new key shows up here as open dead-letters and a pull cursor that stops advancing; conflicts stay logged and visible in this report",
    ),
    (
        "sync push",
        "usage: clio sync push --remote HOST:PORT [--mode push|pull|bidirectional] [--banks a,b] [--token T] [--output json|text]\n       the key is read from CLIO_SYNC_KEY_FILE when set, otherwise from the store `clio sync keygen` recorded; --token falls back to CLIO_SYNC_TOKEN",
    ),
    (
        "sync pull",
        "usage: clio sync pull --remote HOST:PORT [--mode push|pull|bidirectional] [--banks a,b] [--token T] [--output json|text]\n       the key is read from CLIO_SYNC_KEY_FILE when set, otherwise from the store `clio sync keygen` recorded; --token falls back to CLIO_SYNC_TOKEN",
    ),
    (
        "sync serve",
        "usage: clio sync serve [--bind ADDR] [--banks a,b] [--token-file PATH] [--ttl-secs N] [--dev] [--output json|text]\n       runs in the foreground for the resolved TTL (default 1800s); --token-file loads the shared credential store (each secret reaches only its record's banks) once at start, so revocation takes effect at the next start; a configured credential source (--token-file) requires a secret on every request, dev mode included\n       a bind to a non-loopback address warns on stderr, in every mode including --dev, and the bind proceeds: bank scope (which bank a row belongs to), record kind (which shape the peer opens), operation (what the peer applies), transaction time (the order rows win in), and device id (which node wrote a row, and the tie-break when timestamps are equal) cross the network in the clear; the record identifier and the content digest cross in the clear too, in a form derived under a key the relay does not hold, so a relay can neither read them nor confirm a guessed value against them, though it does hold one stable value per row, and an identifier the machine generated is sent unchanged rather than as such a form; a relay also sees the record-kind mix and the device-id stability across time, which are routing-necessary rather than harmless; only the content and the admission score are sealed; this describes rows pushed from a sealing-capable build onward, and rows already on a relay keep their older cleartext identifiers until they are naturally re-pushed; put a TLS-terminating reverse proxy in front of a public bind",
    ),
    (
        "sync keygen",
        "usage: clio sync keygen [--store file|keystore] [--output json|text]\n       creates the key in the recorded store (the recommended file store when none is recorded), records the store choice, and prints the key exactly once; it cannot be recovered except with `clio sync key export`; the key never leaves the node except through that export; generating a new key on a node that already holds one rotates: the node seals with the newest key and opens with any key it holds; a rotation is not done until every peer holds the new key — give it to each peer, confirm the fingerprint matches with `clio sync key status` on both sides, and only then rely on it; a peer that has not been given the new key dead-letters every inbound row and its pull cursor freezes until it gets one, while content sealed under a key the peer still holds stays readable",
    ),
    (
        "sync key export",
        "usage: clio sync key export [--output json|text]\n       prints the stored key on purpose; a shell flag or environment variable leaves it in process listings, so a shared deployment should pass CLIO_SYNC_KEY_FILE pointing at an owner-only key file; this one deliberate exposure is how a rotation reaches each peer: copy the new key to every peer, confirm the fingerprints match, and rely on the rotation only then",
    ),
    (
        "sync key status",
        "usage: clio sync key status [--output json|text]\n       reports the store, fingerprint, creation time, and key count; never the key or a prefix of it; the fingerprint is what two nodes compare to confirm they hold the same key, and the key count shows the superseded keys a node keeps so that content sealed under them stays readable",
    ),
    ("backup dir", "usage: clio backup dir [--output json|text]"),
    (
        "backup create",
        "usage: clio backup create [--db PATH] [--backend sqlite|postgres] [--label TEXT] [--note TEXT] [--keep N] [--confirm] [--dry-run] [--output json|text]\n       a SQLite store is read once into a compacted, transactionally consistent copy (one VACUUM INTO, logically equivalent, never a byte-for-byte clone, and it does not retain deleted pages); a Postgres store is read in one repeatable-read transaction into a row-level dump in COPY text format, whose schema is owned by this build, so restoring it needs a target this build created. Either way the archive holds key material when a key-store file exists, so keep it owner-only. --dry-run writes nothing. --keep N then runs `clio backup prune --keep N` over the backup directory, with the same rule: without --confirm it reports what it would delete and deletes nothing",
    ),
    (
        "backup list",
        "usage: clio backup list [--limit N] [--output json|text]\n       reads each archive's manifest only: no payload, no database, so it works with the store down. Every row states its backend. --db and --backend are refused because they cannot change the listing",
    ),
    (
        "backup prune",
        "usage: clio backup prune --keep N [--confirm] [--dry-run] [--output json|text]\n       keeps the newest N snapshots by manifest time and deletes the rest; the newest snapshot is never a candidate and --keep 0 is refused. An archive whose manifest cannot be read is reported as unreadable and never deleted, and a symlinked archive is reported and skipped, never followed. Without --confirm (or with --dry-run) nothing is removed and the run is a report that exits 0. Exit codes: 0 report or complete prune, 2 usage refusal, 3 a confirmed prune that could not remove every candidate",
    ),
    (
        "backup restore",
        "usage: clio backup restore ID | clio backup restore --latest [--yes] [--no-input] [--no-pre-backup] [--allow-missing-keys] [--dry-run] [--output json|text]\n       replaces the selected store with the archive's, never a merge: the archive's database and key store overwrite the live ones, a SQLite restore swaps the archived database file in, and a Postgres restore replaces every row of the target in one transaction, so a failure halfway leaves the target untouched. A pre-restore snapshot of the current store is taken first by default; --no-pre-backup skips it and says so. The swap of a SQLite store clears the target's write-ahead log before it renames the new store in, so a crash in that window loses whatever the log had not yet written into the store; a Postgres restore replaces the target in one transaction and has no such window; that snapshot is the only thing that covers it, so --no-pre-backup accepts the window rather than removing it. The archive holds key material when it carries a key member, so keep it owner-only; --allow-missing-keys accepts an archive without one and the restored content is then permanently unreadable. On a terminal the snapshot id must be typed exactly; off one, --yes is required, and --no-input forces the non-interactive path",
    ),
    (
        "backup verify",
        "usage: clio backup verify ID | clio backup verify --latest [--output json|text]\n       reads the archive, not a store, so --db and --backend are refused because they cannot change the verdict; it re-reads the payload and the key member, compares both with the manifest, and asks each backend's own payload check whether the payload is valid",
    ),
    (
        "auth token add",
        "usage: clio auth token add --label LABEL --actor ACTOR --bank BANK [--bank BANK] [--output json|text]\n       generates a credential, prints its secret exactly once, and stores only the digest in the owner-only token store; the secret cannot be recovered — save it when you see it; the store file is held under a lock for the whole write, so a run that cannot take the lock refuses without printing a secret, and --db, --backend, and --bank are refused because they cannot change what is written",
    ),
    (
        "auth token list",
        "usage: clio auth token list [--output json|text]\n       prints labels, actors, banks, and creation times; never a secret and never a digest; --db, --backend, --actor, and --bank are refused because they cannot change the listing",
    ),
    (
        "auth token revoke",
        "usage: clio auth token revoke --label LABEL [--output json|text]\n       removes the record; servers load the store at start, so the change takes effect at the next server start; the write holds the store lock, so it never drops a record a concurrent add wrote, and --db, --backend, --actor, and --bank are refused",
    ),
];

pub(crate) fn usage_for(verb: &str) -> Option<&'static str> {
    VERB_USAGE
        .iter()
        .find(|(name, _)| *name == verb)
        .map(|(_, usage)| *usage)
}

/// The canonical usage line for any command, with no prose and no `usage: `
/// prefix, for the surfaces that render it on their own.
///
/// Every command the catalog lists resolves here, so the catalog and each
/// command's own `--help` read the same string. When the two were separate
/// hand-written texts, a flag added to one was absent from the other, and a
/// belief write needed `--source-type` that the catalog never showed.
///
/// A command with no entry here returns `None`, which the catalog renders as an
/// explicit "no usage line registered" marker rather than as a silent omission.
pub(crate) fn usage_line(command: &str) -> Option<&'static str> {
    if let Some(line) = crate::cli_help_surfaces::surface_usage_line(command) {
        return Some(line);
    }
    let usage = usage_for(command)?;
    let first = usage.lines().next()?;
    first.strip_prefix("usage: ")
}

#[cfg(test)]
#[path = "cli_help_usage_tests.rs"]
mod tests;
