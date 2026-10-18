// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the synthetic pressure fixture one open
//! backend is measured over: several hundred stored items, an eligible hub, a
//! stale distilled unit, dirty and refreshing forest nodes, pending and
//! retried index jobs, one known secret text, and one low-utility archive-band
//! item.
//!
//! ## Owns
//! - The fixture constants: bank names, the fixed timestamp, the sizes, and
//!   the archive-band weights
//! - `build`, which seeds a base state and returns a hub-wired `Fixture`
//! - `open_sqlite`, `open_postgres`, and `postgres_url` for the two backends
//! - The local regression test for the shared gate's fail-closed contract
//!
//! ## Does not own
//! - Measurement and the p95 ceilings (`measure`)
//! - The pressure block or its component sources (the runtime crates)
//!
//! ## Boundary
//!
//! Changes to the fixture contents should normally require changes here.
//! Changes to how a component is counted should not require changes here.
//!
//! Keep this module focused on the synthetic pressure fixture.
//
// The fixture is seeded through repository APIs only: `Store::create_memory_item`,
// `Store::reinforce_assoc_edge`, `Store::commit_distilled_unit`, the index
// outbox, and the public forest handle. No SQL and no direct row writes.

use clio_config::hygiene_knobs::HygieneWeights;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_store::{AssocPolicy, DistilledUnitRecord, STALE_SOURCE_CHANGED};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use clio_write::{AttachLeaf, HUB_DISTILL_MARKER, HubDistillEngine};
use std::sync::Arc;

/// Main fixture bank: every pressure component is exercised here.
pub const MAIN_BANK: &str = "pressure-main";
/// Extra banks so the bank-list query reports more than one row.
pub const EXTRA_BANKS: [&str; 2] = ["pressure-extra-1", "pressure-extra-2"];
/// Fixed "now" for the pressure read and the archive scan.
pub const NOW: &str = "2026-10-05T00:00:00Z";
/// Item count in the main bank: several hundred.
pub const MAIN_ITEMS: usize = 300;
/// Main-bank items carrying the known archive-band text.
pub const NOISE_ITEMS: usize = 12;
/// Item whose co-activation degree makes it an eligible hub.
pub const HUB_ITEM: &str = "main-0000";
/// Distilled units committed for the stale count; one carries a stale reason.
pub const DISTILLED_UNITS: [(&str, bool); 2] = [("unit-current", false), ("unit-stale", true)];
/// Items whose index jobs sit in retry.
const RETRY_ITEMS: [&str; 2] = ["main-0001", "main-0002"];
/// Hub degree limit: two edges make one hub eligible.
const HUB_DEGREE_LIMIT: u32 = 1;
/// Leaves attached to the main bank's forest before measuring.
const FOREST_LEAVES: usize = 8;
/// Archive-band text taken from the hygiene scoring fixtures: it hits the
/// secret detector, so the scoring override forces its score to 0.90 or above
/// whatever the weights are, and the block excludes it from the archive count.
const SECRET_TEXT: &str = "api_key: sk-live-abcdefgh1234";
/// The one low-utility archive-band item, seeded past the main bank.
pub const ARCHIVE_BAND_ITEM: &str = "main-archive-band";
/// Weights that park a no-signal item exactly in the archive band, reused from
/// the pressure block's unit fixtures: with no secret, no pattern, and no near
/// duplicate, a zero-admission item scores the utility weight alone (0.72) —
/// above the 0.70 archive floor and below the 0.90 discard floor, whatever its
/// age. The secret text stays a discard-band hit through the scoring override,
/// so the band split keeps working under these weights.
const FIXTURE_WEIGHTS: HygieneWeights = HygieneWeights {
    secret: 0.0,
    pattern: 0.0,
    dup: 0.28,
    utility: 0.72,
    staleness: 0.0,
};

/// A ready-to-measure pressure fixture on one open backend.
pub struct Fixture {
    /// State carrying the fixture banks, the hub engine, and the forest nodes.
    pub state: Arc<McpState>,
    /// Banks that must appear in the store's bank list.
    pub banks: Vec<String>,
    /// Stored item rows across the fixture banks.
    pub item_count: usize,
}

/// Seed the fixture into `base` and return the state the measurement uses.
pub fn build(base: &McpState) -> Fixture {
    seed_main_bank(base);
    seed_extra_banks(base);
    seed_archive_band_item(base);
    for (id, stale) in DISTILLED_UNITS {
        commit_unit(base, id, stale);
    }
    attach_forest(base);
    for id in RETRY_ITEMS {
        base.index
            .fail_index_job(MAIN_BANK, id, "fixture synthetic failure")
            .expect("fail fixture index job");
    }
    let counted = base.ops_store.counts().expect("fixture item counts");
    let mut state = (*base).clone();
    // The archive band [0.70, 0.90) is unreachable under the default hygiene
    // weights (a clean item maxes at 0.65), so the measurement state uses the
    // archive-band weights above, exactly as the pressure block's unit tests do.
    state.env.hygiene = FIXTURE_WEIGHTS;
    state.hub = Some(hub_engine(base));
    let mut banks = Vec::with_capacity(EXTRA_BANKS.len() + 1);
    banks.push(MAIN_BANK.to_owned());
    banks.extend(EXTRA_BANKS.iter().map(|bank| (*bank).to_owned()));
    Fixture {
        state: Arc::new(state),
        banks,
        item_count: usize::try_from(counted.memories).unwrap_or(usize::MAX),
    }
}

/// Open the SQLite in-memory state the fixture runs on.
pub fn open_sqlite() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: MAIN_BANK.into(),
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("open sqlite state")
}

/// The shared `CLIO_DATABASE_URL` when it points at Postgres, else a fail-closed
/// skip through the store's shared gate.
///
/// The contract this crate's Postgres suites run under: with no Postgres
/// `CLIO_DATABASE_URL` configured the gate panics, so a green suite proves the
/// Postgres path actually dialled a database instead of skipping it. The sole
/// opt-out is `CLIO_SKIP_PG_TESTS` set to `1`, `true`, or `yes`, which prints a
/// credential-safe note naming the suite and the server's own refusal and
/// yields `None` so the caller returns early. Any other value, or none, keeps
/// the strict default. The choice is deliberate: this crate takes no Compose
/// fallback, so a run either has a URL or must ask to skip.
pub fn postgres_url() -> Option<String> {
    clio_store::pg_skip::pg_url_or_skip("the read-tools Postgres pressure measurement")
}

/// Open the Postgres state over one private schema URL.
pub fn open_postgres(database_url: &str) -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "postgres".into(),
            database_url: database_url.to_owned(),
            default_bank: MAIN_BANK.into(),
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("open postgres state")
}

/// One main-bank item per index; the first `NOISE_ITEMS` carry the secret text.
fn seed_main_bank(state: &McpState) {
    for i in 0..MAIN_ITEMS {
        let id = format!("main-{i:04}");
        if i < NOISE_ITEMS {
            create_item(
                state,
                &id,
                MAIN_BANK,
                SECRET_TEXT,
                0.5,
                "2026-10-01T00:00:00Z",
            );
        } else {
            let gist = format!(
                "pressure fixture note {i}: service listens on port {}",
                9000 + i
            );
            create_item(state, &id, MAIN_BANK, &gist, 0.9, NOW);
        }
    }
}

/// One item per extra bank, so the bank list has more than the main bank.
fn seed_extra_banks(state: &McpState) {
    for (n, bank) in EXTRA_BANKS.iter().enumerate() {
        let gist = format!("pressure extras bank note {n}");
        create_item(state, &format!("extra-{n}"), bank, &gist, 0.9, NOW);
    }
}

/// The low-utility archive-band item: zero admission and distinct prose with
/// no secret, pattern, or near-duplicate partner, so under the measurement
/// weights it scores the utility weight alone (0.72) and the archive count is
/// a true archive-band candidate, not a secret hit. Created four days before
/// the read, so it is mildly stale too.
fn seed_archive_band_item(state: &McpState) {
    create_item(
        state,
        ARCHIVE_BAND_ITEM,
        MAIN_BANK,
        "quokka ledger notes kept beside the archive band fixture",
        0.0,
        "2026-10-01T00:00:00Z",
    );
}

/// Store one admitted item through the repository's write primitive.
fn create_item(
    state: &McpState,
    id: &str,
    bank: &str,
    gist: &str,
    admission: f64,
    created_at: &str,
) {
    let item = MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(admission),
        source_ref: Some(format!("turn-{id}")),
        snapshot: None,
        gist: Some(gist.to_owned()),
        valid_from: created_at.to_owned(),
        valid_until: None,
        tx_from: created_at.to_owned(),
        tx_until: None,
        created_at: created_at.to_owned(),
        updated_at: created_at.to_owned(),
        context: None,
    };
    state
        .store
        .create_memory_item(&item, "agent")
        .expect("create fixture item");
}

/// Commit one distilled unit; `mark_stale` stores the stale reason the counter
/// reads, so the stale-unit count is nonzero without a distillation wave.
fn commit_unit(state: &McpState, id: &str, mark_stale: bool) {
    let item = MemoryItem {
        id: id.to_owned(),
        bank_id: MAIN_BANK.to_owned(),
        subject_id: id.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.8),
        source_ref: Some(HUB_DISTILL_MARKER.to_owned()),
        snapshot: None,
        gist: Some(format!("distilled summary {id}")),
        valid_from: "2026-10-01T00:00:00Z".to_owned(),
        valid_until: None,
        tx_from: "2026-10-01T00:00:00Z".to_owned(),
        tx_until: None,
        created_at: "2026-10-01T00:00:00Z".to_owned(),
        updated_at: "2026-10-01T00:00:00Z".to_owned(),
        context: None,
    };
    let mut record = DistilledUnitRecord::new(
        MAIN_BANK,
        id,
        HUB_ITEM,
        Vec::<String>::new(),
        HUB_DISTILL_MARKER,
        "2026-10-01T00:00:00Z",
    );
    if mark_stale {
        record.stale_reason = Some(STALE_SOURCE_CHANGED.to_owned());
        record.stale_at = Some("2026-10-02T00:00:00Z".to_owned());
    }
    state
        .store
        .commit_distilled_unit(&item, &record, "agent")
        .expect("commit distilled unit");
}

/// A hub engine with a degree limit of one: two reinforced distinct edges make
/// the hub item eligible, and the fixed zero clock keeps the scan deterministic.
fn hub_engine(state: &McpState) -> Arc<HubDistillEngine> {
    let policy = AssocPolicy {
        hub_degree_limit: HUB_DEGREE_LIMIT,
        w_min: 0.0,
        ..AssocPolicy::default()
    };
    for neighbor in ["hub-neighbor-1", "hub-neighbor-2"] {
        state
            .store
            .reinforce_assoc_edge(MAIN_BANK, HUB_ITEM, neighbor, 0, 1.0)
            .expect("reinforce fixture hub edge");
    }
    Arc::new(HubDistillEngine::with_clock(
        Arc::clone(&state.store),
        state.env.clone(),
        policy,
        0,
    ))
}

/// Attach forest leaves through the public handle, then mark one resulting
/// ancestor refreshing so both maintenance-status halves are nonzero.
fn attach_forest(state: &McpState) {
    let mut forest = state.forest.lock().expect("forest lock");
    let tree = forest.tree_mut(MAIN_BANK);
    for i in 0..FOREST_LEAVES {
        let id = format!("main-{i:04}");
        tree.attach_leaf(AttachLeaf {
            item_id: id,
            bank_id: MAIN_BANK.to_owned(),
            interval_start: format!("{i:08}"),
            interval_end: format!("{i:08}"),
            gist: Some(format!("forest leaf {i}")),
            task_id: None,
        })
        .expect("attach fixture leaf");
    }
    let refreshing = tree
        .dirty_node_ids()
        .into_iter()
        .next()
        .expect("a dirty fixture node exists");
    tree.node_mut(&refreshing)
        .expect("dirty fixture node is mutable")
        .refreshing = true;
}

/// The environment variable that arms the child probe below. The parent sets it
/// on a child `Command` only, so no test in this binary mutates the process
/// environment it shares with every other test in the run.
const PG_GATE_CHILD_ENV: &str = "CLIO_MCP_PG_GATE_CHILD";

/// The child test the parent re-execs, as libtest names it. `#[test]` cannot
/// take a `const` for the function name, so a rename of
/// `postgres_gate_child_probe` must update this string with it.
const POSTGRES_GATE_CHILD_TEST: &str = "fixture::postgres_gate_child_probe";

/// The child half of [`the_postgres_gate_fails_closed_without_a_url_and_skips_only_when_opted_out`].
///
/// Harmless in an ordinary run: it returns before touching the gate unless the
/// parent armed it, so `cargo test -p clio-mcp` never calls the gate here.
// The filter in `POSTGRES_GATE_CHILD_TEST` must name this function and its
// `fixture` module; `#[test]` cannot take a const, so a rename here must
// update that constant.
#[test]
fn postgres_gate_child_probe() {
    if std::env::var_os(PG_GATE_CHILD_ENV).is_none() {
        return;
    }
    let _url = postgres_url();
}

/// The clio-mcp Postgres gate fails closed when `CLIO_DATABASE_URL` is unset and
/// skips only when `CLIO_SKIP_PG_TESTS` asked for it.
///
/// The gate reads the process environment, and every test in this binary shares
/// one process, so this test re-execs its own binary and sets the two variables
/// on the child's `Command` only — the pattern `runtime_index_hardening_tests`
/// already uses. The parent environment, and every peer test, is untouched, so
/// the check is deterministic under a parallel run.
#[test]
fn the_postgres_gate_fails_closed_without_a_url_and_skips_only_when_opted_out() {
    let gate_child = || {
        let mut command = std::process::Command::new(
            std::env::current_exe().expect("locate the read-tools test binary"),
        );
        command
            .args(["--exact", POSTGRES_GATE_CHILD_TEST, "--nocapture"])
            .env(PG_GATE_CHILD_ENV, "1")
            .env_remove("CLIO_DATABASE_URL");
        command
    };

    let strict = gate_child()
        .env_remove("CLIO_SKIP_PG_TESTS")
        .output()
        .expect("run the strict gate child");
    assert!(
        !strict.status.success(),
        "the gate must fail closed with no URL and no opt-out"
    );
    let strict_stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        strict_stderr.contains("CLIO_DATABASE_URL is not set"),
        "the failure must name the missing URL: {strict_stderr}"
    );

    let opted_out = gate_child()
        .env("CLIO_SKIP_PG_TESTS", "1")
        .output()
        .expect("run the opted-out gate child");
    assert!(
        opted_out.status.success(),
        "the opt-out must turn the failure into a skip: {}",
        String::from_utf8_lossy(&opted_out.stderr)
    );
    let opted_out_stderr = String::from_utf8_lossy(&opted_out.stderr);
    assert!(
        opted_out_stderr.contains("skipping the read-tools Postgres pressure measurement"),
        "the skip note must name the suite and stay credential-free: {opted_out_stderr}"
    );
}
