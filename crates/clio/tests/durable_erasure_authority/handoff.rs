// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable denial's life across a stop and
//! a handoff: the denial recorded one step before key loss, its survival of a
//! restart, the handoff into a new-host fixture, the refusal of a new-empty
//! initialization, and the refusal of a known-old copy.
//!
//! ## Owns
//! - The interrupted-erase restart-and-handoff scenario
//! - The existing-deployment initialization refusal and the known-old refusal
//!
//! ## Does not own
//! - The fixtures and probes (see `support`)
//! - The check API's refusal matrix without a store (see `membership`)
//!
//! ## Boundary
//!
//! Changes to which restart and handoff facts are measured should normally
//! require changes here. Changes to the journal's operations should not.
//!
//! Keep this module focused on the denial's endurance, not on the journal.

use crate::support::{
    BANK, DENIED_ITEM, DENIED_SUBJECT, Scratch, denied_hash, file_sha256, journal_path,
    load_current, observe_journal, open_sqlite, prepare, read_error, save_copy, subject_hash,
    write_item,
};
use clio::ErrorCode;
use clio::store::ComplianceStore;
use clio_compliance::{
    ArchiveClaim, AuthorityRefusal, JournalState, MemberFacts, check_archive_authority, handoff,
    initialize_new_empty, record_denial,
};

#[test]
fn denial_recorded_before_key_loss_survives_restart_and_handoff() {
    let scratch = Scratch::new("handoff-restart");
    let journal = journal_path(&scratch, "journal.json");
    let db = scratch.join("a/clio.db");
    let kms = scratch.join("a/clio.kms.json");
    let store = open_sqlite(&db, &kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");

    // The exact production step erase_request runs strictly before key loss:
    // the durable denial is recorded, then the stop happens here — the erase
    // is never invoked, so the subject's key and item stay intact.
    let generation =
        record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");
    assert_eq!(generation, 1, "the first denial lands at generation 1");
    assert_eq!(
        read_error(&store, DENIED_ITEM),
        None,
        "the interrupted erase left the item readable"
    );
    assert!(
        !store
            .subject_dek_destroyed(DENIED_SUBJECT)
            .expect("probe the subject key"),
        "the interrupted erase never destroyed the subject key"
    );
    let before = observe_journal("handoff-restart-source", &journal);
    drop(store);

    // Restart: reopen the store and reload the journal from disk.
    let store = open_sqlite(&db, &kms);
    let restarted = observe_journal("handoff-restart-reloaded", &journal);
    assert_eq!(
        restarted, before,
        "the deployment identity and the denial survive the restart"
    );
    assert_eq!(
        read_error(&store, DENIED_ITEM),
        None,
        "the subject's item survives the restart"
    );
    assert!(
        !store
            .subject_dek_destroyed(DENIED_SUBJECT)
            .expect("probe the subject key"),
        "no key loss happened before or after the restart"
    );

    // Hand the current journal to a new host fixture.
    let new_host = prepare(&scratch, "new-host/erasure-journal.json");
    let handed = handoff(&journal, &new_host).expect("hand the journal to the new host");
    assert_eq!(
        handed, restarted,
        "the handoff keeps the same deployment, generation, and denials"
    );
    let at_new_host = observe_journal("handoff-restart-new-host", &new_host);
    assert_eq!(
        at_new_host, restarted,
        "the new host loads the same authority"
    );
    assert_eq!(
        file_sha256(&new_host),
        file_sha256(&journal),
        "the handed-off authority is byte-identical to the source"
    );
    println!(
        "EVIDENCE handoff-restart deployment={} generation={} denied={} key_intact=true",
        at_new_host.deployment_id,
        at_new_host.generation,
        at_new_host.is_denied(&denied_hash()),
    );
}

#[test]
fn new_empty_initialization_refuses_an_existing_deployment_and_a_known_old_copy_refuses() {
    let scratch = Scratch::new("handoff-initialize");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("seed the deployment");
    let seeded = observe_journal("handoff-initialize-seeded", &journal);
    let seeded_bytes = file_sha256(&journal);
    // The known-old copy: one generation behind the authority handed off
    // below, saved before the second denial is recorded.
    let older = save_copy(&journal, &prepare(&scratch, "older/erasure-journal.json"));

    // New-empty initialization over the existing deployment refuses and
    // leaves the recorded authority untouched.
    let refused =
        initialize_new_empty(&journal).expect_err("init over an existing deployment must refuse");
    assert_eq!(
        refused.code,
        ErrorCode::Forbidden,
        "the init refusal names the occupied path"
    );
    assert_eq!(
        file_sha256(&journal),
        seeded_bytes,
        "the refused init left the journal untouched"
    );

    // Advance the current authority one generation, then hand it off.
    let generation = record_denial(&journal, &subject_hash("subject-later"), &[BANK.to_owned()])
        .expect("record the second denial");
    assert_eq!(generation, 2, "the second denial advances the generation");
    let current = observe_journal("handoff-initialize-current", &journal);
    assert_eq!(current.generation, seeded.generation + 1);
    assert!(current.is_denied(&denied_hash()));
    assert!(current.is_denied(&subject_hash("subject-later")));

    let new_host = prepare(&scratch, "new-host/erasure-journal.json");
    let handed = handoff(&journal, &new_host).expect("hand the current authority to the new host");
    assert_eq!(
        handed, current,
        "the handoff keeps the denial and the generation"
    );
    let at_new_host = observe_journal("handoff-initialize-new-host", &new_host);
    assert_eq!(
        at_new_host, current,
        "the new host holds the current authority"
    );
    assert_eq!(at_new_host.denials.len(), 2, "both denials were handed off");

    // The known-old copy is refused against the handed-off current authority.
    let older_journal = load_current(&older);
    assert_eq!(older_journal.deployment_id, current.deployment_id);
    assert_eq!(
        older_journal.generation + 1,
        current.generation,
        "the saved copy is exactly one generation old"
    );
    let older_generation = older_journal.generation;
    let older_state = JournalState::Current(older_journal);
    let current_state = JournalState::Current(current.clone());
    let claim = ArchiveClaim {
        deployment_id: current.deployment_id.clone(),
        generation: current.generation,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    };
    let known_old = check_archive_authority(
        &current_state,
        Some(&older_state),
        &claim,
        &[MemberFacts::None],
    );
    assert_eq!(
        known_old.expect_err("a known-old copy must refuse"),
        AuthorityRefusal::PresentedOlderGeneration
    );
    println!(
        "EVIDENCE handoff-initialize deployment={} generation={} denials={} known_old_generation={}",
        at_new_host.deployment_id,
        at_new_host.generation,
        at_new_host.denials.len(),
        older_generation,
    );
}
