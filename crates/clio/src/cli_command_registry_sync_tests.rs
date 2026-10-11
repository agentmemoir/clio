// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the seven sync records against the
//! usage bytes and parser flag sets the commands print today, and for pinning
//! the tranche's capture order and the key-custody notes.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   registered usage text
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, and required-flag values
//! - The tranche pin: the seven leaves occur once each, in capture order
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The frozen golden capture (`golden_repeatability_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the seven sync records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::{Section, ValueKind};

/// `sync status` renders today's usage text and builds today's parser sets.
#[test]
fn sync_status_matches_the_registered_usage_and_parser() {
    let spec = record("sync status");
    assert_eq!(
        spec.full_usage(),
        "usage: clio sync status [--banks a,b] [--output json|text]\n       on a single-host deployment (sync.omit=true) this explains the omission and exits 0\n       after a key rotation, a peer that has not been given the new key shows up here as open dead-letters and a pull cursor that stops advancing; conflicts stay logged and visible in this report",
        "sync status full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio sync status [--banks a,b] [--output json|text]",
        "sync status usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "banks"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert!(spec.required_flags().is_empty(), "no bare flag");
    assert_eq!(spec.tool, Some("sync_status"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `sync push` renders today's usage text and builds today's parser sets.
#[test]
fn sync_push_matches_the_registered_usage_and_parser() {
    let spec = record("sync push");
    assert_eq!(
        spec.full_usage(),
        "usage: clio sync push --remote HOST:PORT [--mode push|pull|bidirectional] [--banks a,b] [--token T] [--output json|text]\n       the key is read from CLIO_SYNC_KEY_FILE when set, otherwise from the store `clio sync keygen` recorded; --token falls back to CLIO_SYNC_TOKEN",
        "sync push full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio sync push --remote HOST:PORT [--mode push|pull|bidirectional] [--banks a,b] [--token T] [--output json|text]",
        "sync push usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "remote", "mode", "banks", "token"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["remote"], "the bare selector");
    assert_eq!(spec.tool, Some("sync_push"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `sync pull` renders today's usage text and builds today's parser sets.
#[test]
fn sync_pull_matches_the_registered_usage_and_parser() {
    let spec = record("sync pull");
    assert_eq!(
        spec.full_usage(),
        "usage: clio sync pull --remote HOST:PORT [--mode push|pull|bidirectional] [--banks a,b] [--token T] [--output json|text]\n       the key is read from CLIO_SYNC_KEY_FILE when set, otherwise from the store `clio sync keygen` recorded; --token falls back to CLIO_SYNC_TOKEN",
        "sync pull full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio sync pull --remote HOST:PORT [--mode push|pull|bidirectional] [--banks a,b] [--token T] [--output json|text]",
        "sync pull usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "remote", "mode", "banks", "token"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["remote"], "the bare selector");
    assert_eq!(spec.tool, Some("sync_pull"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `sync serve` renders today's usage text and builds today's parser sets.
#[test]
fn sync_serve_matches_the_registered_usage_and_parser() {
    let spec = record("sync serve");
    assert_eq!(
        spec.full_usage(),
        "usage: clio sync serve [--bind ADDR] [--banks a,b] [--token-file PATH] [--ttl-secs N] [--dev] [--output json|text]\n       runs in the foreground for the resolved TTL (default 1800s); --token-file loads the shared credential store (each secret reaches only its record's banks) once at start, so revocation takes effect at the next start; a configured credential source (--token-file) requires a secret on every request, dev mode included\n       a bind to a non-loopback address warns on stderr, in every mode including --dev, and the bind proceeds: bank scope (which bank a row belongs to), record kind (which shape the peer opens), operation (what the peer applies), transaction time (the order rows win in), and device id (which node wrote a row, and the tie-break when timestamps are equal) cross the network in the clear; the record identifier and the content digest cross in the clear too, in a form derived under a key the relay does not hold, so a relay can neither read them nor confirm a guessed value against them, though it does hold one stable value per row, and an identifier the machine generated is sent unchanged rather than as such a form; a relay also sees the record-kind mix and the device-id stability across time, which are routing-necessary rather than harmless; only the content and the admission score are sealed; this describes rows pushed from a sealing-capable build onward, and rows already on a relay keep their older cleartext identifiers until they are naturally re-pushed; put a TLS-terminating reverse proxy in front of a public bind",
        "sync serve full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio sync serve [--bind ADDR] [--banks a,b] [--token-file PATH] [--ttl-secs N] [--dev] [--output json|text]",
        "sync serve usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "bind",
            "banks",
            "token-file",
            "ttl-secs"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes", "dev"]);
    assert!(spec.required_flags().is_empty(), "no bare flag");
    assert_eq!(spec.tool, Some("sync_serve"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `sync keygen` renders today's usage text and builds today's parser sets.
#[test]
fn sync_keygen_matches_the_registered_usage_and_parser() {
    let spec = record("sync keygen");
    assert_eq!(
        spec.full_usage(),
        "usage: clio sync keygen [--store file|keystore] [--output json|text]\n       creates the key in the recorded store (the recommended file store when none is recorded), records the store choice, and prints the key exactly once; it cannot be recovered except with `clio sync key export`; the key never leaves the node except through that export; generating a new key on a node that already holds one rotates: the node seals with the newest key and opens with any key it holds; a rotation is not done until every peer holds the new key — give it to each peer, confirm the fingerprint matches with `clio sync key status` on both sides, and only then rely on it; a peer that has not been given the new key dead-letters every inbound row and its pull cursor freezes until it gets one, while content sealed under a key the peer still holds stays readable",
        "sync keygen full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio sync keygen [--store file|keystore] [--output json|text]",
        "sync keygen usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "store"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert!(spec.required_flags().is_empty(), "no bare flag");
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `sync key export` renders today's usage text and builds today's parser sets.
#[test]
fn sync_key_export_matches_the_registered_usage_and_parser() {
    let spec = record("sync key export");
    assert_eq!(
        spec.full_usage(),
        "usage: clio sync key export [--output json|text]\n       prints the stored key on purpose; a shell flag or environment variable leaves it in process listings, so a shared deployment should pass CLIO_SYNC_KEY_FILE pointing at an owner-only key file; this one deliberate exposure is how a rotation reaches each peer: copy the new key to every peer, confirm the fingerprints match, and rely on the rotation only then",
        "sync key export full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio sync key export [--output json|text]",
        "sync key export usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert!(spec.required_flags().is_empty(), "no bare flag");
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `sync key status` renders today's usage text and builds today's parser sets.
#[test]
fn sync_key_status_matches_the_registered_usage_and_parser() {
    let spec = record("sync key status");
    assert_eq!(
        spec.full_usage(),
        "usage: clio sync key status [--output json|text]\n       reports the store, fingerprint, creation time, and key count; never the key or a prefix of it; the fingerprint is what two nodes compare to confirm they hold the same key, and the key count shows the superseded keys a node keeps so that content sealed under them stays readable",
        "sync key status full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio sync key status [--output json|text]",
        "sync key status usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert!(spec.required_flags().is_empty(), "no bare flag");
    assert_eq!(spec.tool, Some("sync_key_status"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// The seven leaves occur once each across the whole registry, in the capture
/// order `clio sync --help` and the catalog printed them: the transfer verbs
/// first, then the key verbs.
///
/// The guard covers these seven names across every tranche: one of them
/// missing, one of them registered twice anywhere in the tree, or the seven
/// drifting out of order fails here. A record added under another name, or a
/// duplicate among other names, is not caught by this test.
#[test]
fn the_seven_leaves_occur_once_each_in_capture_order() {
    let order = [
        "sync status",
        "sync push",
        "sync pull",
        "sync serve",
        "sync keygen",
        "sync key export",
        "sync key status",
    ];
    let tranche: Vec<&str> = crate::cli_command_registry::all()
        .map(|spec| spec.name)
        .collect();
    for name in order {
        let hits: Vec<usize> = tranche
            .iter()
            .enumerate()
            .filter(|(_, listed)| **listed == name)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(hits.len(), 1, "`{name}` is registered {hits:?} times");
    }
    let at: Vec<usize> = order
        .iter()
        .map(|name| tranche.iter().position(|listed| listed == name).unwrap())
        .collect();
    assert!(
        at.windows(2).all(|pair| pair[0] < pair[1]),
        "the sync leaves drifted out of capture order: {at:?}"
    );
}

/// The summaries are the authored catalog rows, byte for byte: the catalog
/// resolves record-first, so a reworded summary is a row no surface can print.
#[test]
fn the_summaries_are_the_authored_catalog_rows() {
    for (name, summary) in [
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
    ] {
        assert_eq!(record(name).summary, summary, "{name} summary");
    }
}

/// The seven records bind exactly the tools the transfer and status verbs call,
/// accept exactly the six unprinted globals, and declare no aliases, flag
/// aliases, alternates, or examples.
#[test]
fn the_seven_records_carry_no_extra_surfaces() {
    for (name, tool) in [
        ("sync status", Some("sync_status")),
        ("sync push", Some("sync_push")),
        ("sync pull", Some("sync_pull")),
        ("sync serve", Some("sync_serve")),
        ("sync keygen", None),
        ("sync key export", None),
        ("sync key status", Some("sync_key_status")),
    ] {
        let spec = record(name);
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert!(spec.examples.is_empty(), "{name} examples");
        assert_eq!(spec.tool, tool, "{name} tool");
        assert_eq!(spec.usage_override, None, "{name} derives its line");
        assert_eq!(
            spec.accepted_extra,
            ["db", "backend", "bank", "actor", "no-input", "yes"],
            "{name} accepted globals"
        );
    }
}

/// The real registry satisfies `validate` against the catalog inventory.
#[test]
fn validate_accepts_the_sync_records_against_the_catalog_inventory() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> =
        crate::cli_command_registry::all().collect();
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&specs, &inventory),
        Ok(())
    );
}

/// Every documented flag in the seven sync records carries complete metadata.
#[test]
fn metadata_check_accepts_the_sync_records() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> =
        crate::cli_command_registry_sync::SYNC_LEAVES
            .iter()
            .collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// Each sync value flag carries the kind its named consumer parses, and the
/// flags with a fixed code default record it: the per-verb mode, the serve
/// bind, and the serve TTL. The token and the keygen store are config- and
/// file-derived, so they stay `None` and name their source.
#[test]
fn the_sync_value_flag_kinds_and_defaults_match_their_consumers() {
    let flag = |name: &str, long: &str| {
        record(name)
            .documented
            .iter()
            .find(|candidate| candidate.long == long)
            .unwrap_or_else(|| panic!("`{name}` documents `--{long}`"))
    };
    for (name, long) in [
        ("sync status", "banks"),
        ("sync push", "remote"),
        ("sync push", "banks"),
        ("sync push", "token"),
        ("sync serve", "bind"),
        ("sync serve", "banks"),
        ("sync serve", "token-file"),
    ] {
        assert_eq!(
            flag(name, long).value.expect("a value flag").kind,
            ValueKind::Text,
            "`{name} --{long}`"
        );
    }
    assert_eq!(
        flag("sync push", "mode").value.expect("a value flag").kind,
        ValueKind::Enum(&["push", "pull", "bidirectional"])
    );
    assert_eq!(
        flag("sync serve", "ttl-secs")
            .value
            .expect("a value flag")
            .kind,
        ValueKind::Uint
    );
    assert_eq!(
        flag("sync keygen", "store")
            .value
            .expect("a value flag")
            .kind,
        ValueKind::Enum(&["file", "keystore"])
    );
    assert_eq!(flag("sync push", "mode").default, Some("push"));
    assert_eq!(flag("sync pull", "mode").default, Some("pull"));
    assert_eq!(flag("sync serve", "bind").default, Some("127.0.0.1:0"));
    assert_eq!(flag("sync serve", "ttl-secs").default, Some("1800"));
    assert_eq!(flag("sync push", "token").default, None);
    assert!(
        flag("sync push", "token")
            .description
            .contains("CLIO_SYNC_TOKEN")
    );
    assert_eq!(flag("sync keygen", "store").default, None);
    assert!(
        flag("sync keygen", "store")
            .description
            .contains("recorded choice")
    );
}
