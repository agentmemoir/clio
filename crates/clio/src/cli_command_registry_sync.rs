// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the seven `sync`
//! commands: `sync status`, `sync push`, `sync pull`, `sync serve`,
//! `sync keygen`, `sync key export`, and `sync key status`.
//!
//! ## Owns
//! - The seven `CommandSpec` records and the tranche that lists them in
//!   capture order, each carrying the bytes its authored usage row and catalog
//!   row held
//! - The key-custody notes: the once-only keygen print, the deliberate key
//!   export exposure, and the status view that never shows a key byte
//! - The `sync serve` cleartext-relay warning and the single-host omission
//!   explanation, as the notes the usage lines carry
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - Sync transfer behavior or key resolution (see `sync_cli`,
//!   `sync_key_cli`, and `clio-sync`)
//!
//! ## Boundary
//!
//! Changes to one of these seven commands' flags, usage text, or summary
//! should normally require changes here. Changes to sync behavior or key
//! policy should not.
//!
//! Keep this module focused on the seven sync records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the notes are
// the prose the command's own `--help` printed, so both rendered surfaces and
// the frozen golden capture keep their exact output.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

/// The globals a sync leaf accepts without printing: the shared parser merges
/// every declared global, so the leaves that print only their own flags and
/// `--output` list the other six here.
const SYNC_UNPRINTED: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The required `--remote HOST:PORT` selector of `push` and `pull`: the usage
/// line writes it bare because the transfer refuses to run without it.
const REMOTE: Flag = Flag {
    required: true,
    ..Flag::text(
        "remote",
        "HOST:PORT",
        "Names the peer `sync_cli::transfer_args` sends to, as `host:port`; the transfer refuses to run without it.",
    )
};

/// The `status` omission note: what a single-host deployment prints instead.
const STATUS_OMIT_NOTE: &str =
    "on a single-host deployment (sync.omit=true) this explains the omission and exits 0";

/// The `status` rotation note: how an unrefreshed peer shows up in the report.
const STATUS_ROTATION_NOTE: &str = "after a key rotation, a peer that has not been given the new key shows up here as open dead-letters and a pull cursor that stops advancing; conflicts stay logged and visible in this report";

/// The `push`/`pull` note: where the transfer key and the peer credential come
/// from. The cipher key is never a CLI argument.
const TRANSFER_NOTE: &str = "the key is read from CLIO_SYNC_KEY_FILE when set, otherwise from the store `clio sync keygen` recorded; --token falls back to CLIO_SYNC_TOKEN";

/// The `serve` credential note: the shared credential store, its start-time
/// load, and the secret-on-every-request rule.
const SERVE_CREDENTIAL_NOTE: &str = "runs in the foreground for the resolved TTL (default 1800s); --token-file loads the shared credential store (each secret reaches only its record's banks) once at start, so revocation takes effect at the next start; a configured credential source (--token-file) requires a secret on every request, dev mode included";

/// The `serve` cleartext note: everything a non-loopback relay can see, what it
/// cannot, and the reverse-proxy rule for a public bind.
const SERVE_CLEARTEXT_NOTE: &str = "a bind to a non-loopback address warns on stderr, in every mode including --dev, and the bind proceeds: bank scope (which bank a row belongs to), record kind (which shape the peer opens), operation (what the peer applies), transaction time (the order rows win in), and device id (which node wrote a row, and the tie-break when timestamps are equal) cross the network in the clear; the record identifier and the content digest cross in the clear too, in a form derived under a key the relay does not hold, so a relay can neither read them nor confirm a guessed value against them, though it does hold one stable value per row, and an identifier the machine generated is sent unchanged rather than as such a form; a relay also sees the record-kind mix and the device-id stability across time, which are routing-necessary rather than harmless; only the content and the admission score are sealed; this describes rows pushed from a sealing-capable build onward, and rows already on a relay keep their older cleartext identifiers until they are naturally re-pushed; put a TLS-terminating reverse proxy in front of a public bind";

/// The `keygen` note: the once-only print, the rotation order, and what a peer
/// without the new key does.
const KEYGEN_NOTE: &str = "creates the key in the recorded store (the recommended file store when none is recorded), records the store choice, and prints the key exactly once; it cannot be recovered except with `clio sync key export`; the key never leaves the node except through that export; generating a new key on a node that already holds one rotates: the node seals with the newest key and opens with any key it holds; a rotation is not done until every peer holds the new key — give it to each peer, confirm the fingerprint matches with `clio sync key status` on both sides, and only then rely on it; a peer that has not been given the new key dead-letters every inbound row and its pull cursor freezes until it gets one, while content sealed under a key the peer still holds stays readable";

/// The `key export` note: the one deliberate key exposure and how a rotation
/// uses it.
const KEY_EXPORT_NOTE: &str = "prints the stored key on purpose; a shell flag or environment variable leaves it in process listings, so a shared deployment should pass CLIO_SYNC_KEY_FILE pointing at an owner-only key file; this one deliberate exposure is how a rotation reaches each peer: copy the new key to every peer, confirm the fingerprints match, and rely on the rotation only then";

/// The `key status` note: what the report shows and what it never shows.
const KEY_STATUS_NOTE: &str = "reports the store, fingerprint, creation time, and key count; never the key or a prefix of it; the fingerprint is what two nodes compare to confirm they hold the same key, and the key count shows the superseded keys a node keeps so that content sealed under them stays readable";

/// `clio sync status`: masked local status, with the single-host explanation.
pub(crate) const SYNC_STATUS: CommandSpec = CommandSpec {
    name: "sync status",
    aliases: &[],
    summary: "explain what sync would do, and omit when disabled",
    notes: &[STATUS_OMIT_NOTE, STATUS_ROTATION_NOTE],
    positionals: &[],
    documented: &[
        Flag::text(
            "banks",
            "a,b",
            "Selects the banks the status report covers; `sync_cli::banks_arg` splits the comma list and refuses one that names no bank.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: SYNC_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio sync status",
            comment: Some("Explain what sync would do."),
        },
        Example {
            command: "clio sync status --banks default",
            comment: Some("Report one bank."),
        },
        Example {
            command: "clio sync status -o json",
            comment: Some("Emit the status report as JSON."),
        },
    ],
    tool: Some("sync_status"),
    section: Section::Read,
};

/// `clio sync push`: send local changes to a sync server.
pub(crate) const SYNC_PUSH: CommandSpec = CommandSpec {
    name: "sync push",
    aliases: &[],
    summary: "send local changes to a sync server",
    notes: &[TRANSFER_NOTE],
    positionals: &[],
    documented: &[
        REMOTE,
        Flag {
            default: Some("push"),
            ..Flag::choice(
                "mode",
                "push|pull|bidirectional",
                "Selects the operation `sync_cli::transfer_args` sends: `push`, `pull`, or `bidirectional`; absent, the sync tool uses `push`.",
                &["push", "pull", "bidirectional"],
            )
        },
        Flag::text(
            "banks",
            "a,b",
            "Limits the transfer to the named banks; `sync_cli::banks_arg` splits the comma list and refuses one that names no bank.",
        ),
        Flag::text(
            "token",
            "T",
            "Bearer token for the peer `sync_cli::secret_flag` sends; absent, it falls back to `CLIO_SYNC_TOKEN`.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: SYNC_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio sync push --remote 127.0.0.1:8788",
            comment: Some("Push local changes to a peer."),
        },
        Example {
            command: "clio sync push --remote 127.0.0.1:8788 --banks default",
            comment: Some("Push one bank."),
        },
        Example {
            command: "clio sync push --remote 127.0.0.1:8788 --mode bidirectional",
            comment: Some("Push and pull in one transfer."),
        },
    ],
    tool: Some("sync_push"),
    section: Section::Write,
};

/// `clio sync pull`: take remote changes from a sync server.
pub(crate) const SYNC_PULL: CommandSpec = CommandSpec {
    name: "sync pull",
    aliases: &[],
    summary: "take remote changes from a sync server",
    notes: &[TRANSFER_NOTE],
    positionals: &[],
    documented: &[
        REMOTE,
        Flag {
            default: Some("pull"),
            ..Flag::choice(
                "mode",
                "push|pull|bidirectional",
                "Selects the operation `sync_cli::transfer_args` sends: `push`, `pull`, or `bidirectional`; absent, the sync tool uses `pull`.",
                &["push", "pull", "bidirectional"],
            )
        },
        Flag::text(
            "banks",
            "a,b",
            "Limits the transfer to the named banks; `sync_cli::banks_arg` splits the comma list and refuses one that names no bank.",
        ),
        Flag::text(
            "token",
            "T",
            "Bearer token for the peer `sync_cli::secret_flag` sends; absent, it falls back to `CLIO_SYNC_TOKEN`.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: SYNC_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio sync pull --remote 127.0.0.1:8788",
            comment: Some("Take remote changes from a peer."),
        },
        Example {
            command: "clio sync pull --remote 127.0.0.1:8788 --banks default",
            comment: Some("Pull one bank."),
        },
        Example {
            command: "clio sync pull --remote 127.0.0.1:8788 -o json",
            comment: Some("Print the transfer report as JSON."),
        },
    ],
    tool: Some("sync_pull"),
    section: Section::Read,
};

/// `clio sync serve`: run the sync server in the foreground.
pub(crate) const SYNC_SERVE: CommandSpec = CommandSpec {
    name: "sync serve",
    aliases: &[],
    summary: "run the sync server in the foreground",
    notes: &[SERVE_CREDENTIAL_NOTE, SERVE_CLEARTEXT_NOTE],
    positionals: &[],
    documented: &[
        Flag {
            default: Some("127.0.0.1:0"),
            ..Flag::text(
                "bind",
                "ADDR",
                "Sets the address `sync serve` binds, as `host:port`; port 0 picks an ephemeral port.",
            )
        },
        Flag::text(
            "banks",
            "a,b",
            "Limits the served banks; `sync_cli::banks_arg` splits the comma list and refuses one that names no bank.",
        ),
        Flag::text(
            "token-file",
            "PATH",
            "Points at the shared credential store `sync serve` loads once at start; each secret reaches only its record's banks.",
        ),
        Flag {
            default: Some("1800"),
            ..Flag::uint(
                "ttl-secs",
                "N",
                "Sets the foreground auto-stop TTL in seconds that `sync serve` applies; `sync_cli::serve_args` requires a whole number.",
            )
        },
        Flag::toggle(
            "dev",
            "Allows a token-less anonymous bind; `sync serve` refuses one without this outside dev mode.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: SYNC_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio sync serve --token-file ./tokens.json",
            comment: Some("Serve on an ephemeral loopback port with the shared credential store."),
        },
        Example {
            command: "clio sync serve --bind 127.0.0.1:8788 --token-file ./tokens.json",
            comment: Some("Serve on a fixed loopback port with the shared credential store."),
        },
        Example {
            command: "clio sync serve --bind 127.0.0.1:8788 --dev",
            comment: Some("Allow a token-less bind for local development."),
        },
    ],
    tool: Some("sync_serve"),
    section: Section::Write,
};

/// `clio sync keygen`: create or supersede the key, printed exactly once.
pub(crate) const SYNC_KEYGEN: CommandSpec = CommandSpec {
    name: "sync keygen",
    aliases: &[],
    summary: "Create the sync key in the recorded store and record the choice; the key is printed exactly once and cannot be recovered except with `clio sync key export`",
    notes: &[KEYGEN_NOTE],
    positionals: &[],
    documented: &[
        Flag::choice(
            "store",
            "file|keystore",
            "Chooses where `sync_key_cli::keygen` writes the key via `KeySource::parse`: `file` or `keystore`; absent, the recorded choice or the recommended `file` store decides.",
            &["file", "keystore"],
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: SYNC_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio sync keygen",
            comment: Some("Create the key and print it exactly once."),
        },
        Example {
            command: "clio sync keygen --store file",
            comment: Some("Create the key in the file store."),
        },
        Example {
            command: "clio sync keygen --store file -o json",
            comment: Some("Create the key in the file store and report it as JSON."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// `clio sync key export`: print the stored key, the one deliberate exposure.
pub(crate) const SYNC_KEY_EXPORT: CommandSpec = CommandSpec {
    name: "sync key export",
    aliases: &[],
    summary: "Print the stored sync key; a shell flag or environment variable leaves the key in process listings, so a shared deployment should point CLIO_SYNC_KEY_FILE at an owner-only key file",
    notes: &[KEY_EXPORT_NOTE],
    positionals: &[],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: SYNC_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio sync key export",
            comment: Some("Print the stored key for a controlled copy."),
        },
        Example {
            command: "clio sync key export -o json",
            comment: Some("Print the key as JSON for the same controlled copy."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// `clio sync key status`: the key's custody facts, never a key byte.
pub(crate) const SYNC_KEY_STATUS: CommandSpec = CommandSpec {
    name: "sync key status",
    aliases: &[],
    summary: "Show the sync key's store, fingerprint, creation time, and count; never the key",
    notes: &[KEY_STATUS_NOTE],
    positionals: &[],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: SYNC_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio sync key status",
            comment: Some("Show the store, fingerprint, creation time, and count."),
        },
        Example {
            command: "clio sync key status -o json",
            comment: Some("Emit the same facts as JSON."),
        },
    ],
    tool: Some("sync_key_status"),
    section: Section::Read,
};

/// The seven sync records, in capture order.
pub(crate) static SYNC_LEAVES: &[CommandSpec] = &[
    SYNC_STATUS,
    SYNC_PUSH,
    SYNC_PULL,
    SYNC_SERVE,
    SYNC_KEYGEN,
    SYNC_KEY_EXPORT,
    SYNC_KEY_STATUS,
];

#[cfg(test)]
#[path = "cli_command_registry_sync_tests.rs"]
mod tests;
