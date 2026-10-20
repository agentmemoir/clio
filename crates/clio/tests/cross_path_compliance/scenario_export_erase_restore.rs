// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the export-then-erase-then-restore
//! cross-path claim for one subject: that every family is exported, the
//! delivered copy is recorded, the erasure completes with retry, every
//! pre-erasure archive restore route refuses, and the other subject stays
//! readable.
//!
//! ## Owns
//! - The export, delivered-copy, erase, and restore-route steps of this claim
//! - The assertions that the denied content never returns and the control
//!   subject is unchanged
//!
//! ## Does not own
//! - The archive assembly and read-back probes, and the Postgres gate (see
//!   `support_test_support` and `support_pg_test_support`)
//! - The shared refusal assertions (see `refusals_test_support` and `cli_test_support`)
//!
//! ## Boundary
//!
//! Changes to which routes this claim proves should normally require changes
//! here. Changes to an archive or restore primitive should not.
//!
//! Keep this module focused on the one export-erase-restore claim.

use crate::case_fixtures::{
    self, BANK, CONTROL_ITEM, CONTROL_MARKER, CONTROL_SUBJECT, DENIED_MARKER, DENIED_SUBJECT,
    unique,
};
use crate::cli_test_support::{DENIED_SUBJECT_REFUSAL, assert_refusal, clio_command, run_captured};
use crate::family_fixtures;
use crate::family_test_support::{self, FamilyFixture};
use crate::refusals_test_support::{assert_open_copy_refusal, seed_done_path};
use crate::support_pg_test_support::{
    archive_postgres_with_authority, lock_pg, pg_url, postgres_reachable, require_postgres,
};
use crate::support_test_support::{self, Readback};
use clio::store::{ComplianceStore, DurableRequestStore, ScopedSchema, Store};
use clio_backup::manifest::ManifestAuthority;
use clio_compliance::{JournalState, initialize_new_empty, load_journal};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The control-only target every arm seeds.
const CONTROL_ONLY: &[(&str, &str, &str)] = &[(CONTROL_ITEM, CONTROL_SUBJECT, CONTROL_MARKER)];

/// One finished `clio` run: its exit code and both captured streams.
struct CliRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Every store read this claim needs, on either backend.
trait Live: Store + ComplianceStore + DurableRequestStore {}
impl<T: Store + ComplianceStore + DurableRequestStore + ?Sized> Live for T {}

/// One backend's live endpoints and the fixture seeded into them.
enum Backend {
    Sqlite {
        source: PathBuf,
        target: PathBuf,
        kms: PathBuf,
    },
    Pg {
        source: String,
        target: String,
        kms: PathBuf,
    },
}

impl Backend {
    /// Open the source store for one read; it drops before any child runs.
    fn open(&self) -> Box<dyn Live> {
        match self {
            Self::Sqlite { source, kms, .. } => Box::new(case_fixtures::open_sqlite(source, kms)),
            Self::Pg { source, kms, .. } => Box::new(case_fixtures::open_pg(source, kms)),
        }
    }

    /// The `--db` value of every source command.
    fn source(&self) -> &str {
        match self {
            Self::Sqlite { source, .. } => source.to_str().expect("the source path is UTF-8"),
            Self::Pg { source, .. } => source,
        }
    }

    /// The `--db` value of every restore command.
    fn target(&self) -> &str {
        match self {
            Self::Sqlite { target, .. } => target.to_str().expect("the target path is UTF-8"),
            Self::Pg { target, .. } => target,
        }
    }

    /// The key store every `clio` child and read-back opens.
    fn kms(&self) -> &Path {
        match self {
            Self::Sqlite { kms, .. } | Self::Pg { kms, .. } => kms,
        }
    }

    /// Seed the two-subject source and the control-only target.
    fn seed(&self) -> FamilyFixture {
        match self {
            Self::Sqlite { .. } => {
                let fixture = family_fixtures::seed_sqlite(Path::new(self.source()), self.kms());
                case_fixtures::seed_sqlite(Path::new(self.target()), self.kms(), CONTROL_ONLY);
                fixture
            }
            Self::Pg { .. } => {
                let fixture = family_fixtures::seed_pg(self.source(), self.kms());
                case_fixtures::seed_pg(self.target(), self.kms(), CONTROL_ONLY);
                fixture
            }
        }
    }

    /// Cut the pre-erasure archive and return its snapshot id.
    fn archive(&self, backups: &Path, journal: &Path, slug: &str) -> String {
        let authority = match load_journal(journal) {
            JournalState::Current(found) => Some(ManifestAuthority {
                deployment_id: found.deployment_id,
                generation: found.generation,
                salt_fingerprint: found.salt_fingerprint,
            }),
            other => panic!("expected a current journal: {other:?}"),
        };
        match self {
            Self::Sqlite { .. } => {
                support_test_support::archive_sqlite_with_authority(
                    backups,
                    Path::new(self.source()),
                    self.kms(),
                    slug,
                    authority,
                )
                .id
            }
            Self::Pg { .. } => {
                archive_postgres_with_authority(backups, self.source(), self.kms(), slug, authority)
                    .id
            }
        }
    }
}

/// The common `clio` frame: the deployment database, the command's own
/// whitespace-separated arguments, and the harness actor, bank, and JSON
/// output every step shares. Fixture paths and ids carry no spaces.
fn clio_args(source: &str, verb: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["--db".to_owned(), source.to_owned()];
    args.extend(verb.split_whitespace().map(str::to_owned));
    args.extend(["--actor", "harness", "--bank", BANK, "--output", "json"].map(str::to_owned));
    args
}

/// One `clio backup restore` selector: the snapshot id (or `--latest`), then
/// the confirmation and target the restore driver always takes.
fn restore_args(target: &str, selector: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["backup".to_owned(), "restore".to_owned()];
    args.extend(selector.split_whitespace().map(str::to_owned));
    args.extend(["--yes", "--db", target, "--output", "json"].map(str::to_owned));
    args
}

/// The Clio encryption key the in-process fixtures and the `clio` children
/// share, so a key store one writer seals the other can open.
const KEY: [u8; 32] = [0x5a; 32];

/// The key file every `clio` child opens, a sibling of the erasure journal.
fn key_path(journal: &Path) -> PathBuf {
    journal
        .parent()
        .expect("the journal has a parent")
        .join("clio.key")
}

/// The thread-local key this scenario installs, cleared when the guard drops.
///
/// The key is a thread-local, so a scenario that leaves it installed hands it
/// to the next test the harness runs on the same thread.
struct ThreadKeyGuard;

impl Drop for ThreadKeyGuard {
    fn drop(&mut self) {
        clio_config::secret_seal::test_key::clear_thread_key();
    }
}

/// Write this process's key file and install the same key on this thread.
///
/// The returned guard clears the thread-local key when it drops, so the key
/// never outlives the scenario that installed it.
fn install_key(journal: &Path) -> ThreadKeyGuard {
    clio_config::secret_seal::test_key::use_key(KEY);
    std::fs::write(key_path(journal), KEY).expect("write the scenario key file");
    ThreadKeyGuard
}

/// Run one real `clio` command, draining both streams while it waits.
///
/// The wait is bounded: the shared driver kills and reaps a child that does
/// not exit before its deadline, so a hung command fails the run instead of
/// stalling it.
fn run(journal: &Path, kms: &Path, backups: &Path, args: &[String]) -> CliRun {
    let mut command = clio_command(journal, kms, backups);
    command
        .env("CLIO_KEY_FILE", key_path(journal))
        .env("CLIO_KEY_FILE_ONLY", "1")
        .args(args);
    let out = run_captured(&mut command);
    assert!(
        out.status.code().is_some(),
        "clio {} was killed by a signal",
        args.join(" ")
    );
    CliRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// One per-backend surface the composed claim drives.
struct Claim {
    name: &'static str,
    backend: Backend,
    journal: PathBuf,
    backups: PathBuf,
    export_path: PathBuf,
    archive_id: String,
    fixture: FamilyFixture,
}

impl Claim {
    /// Run one framed `clio` command without requiring success.
    fn raw(&self, verb: &str) -> CliRun {
        run(
            &self.journal,
            self.backend.kms(),
            &self.backups,
            &clio_args(self.backend.source(), verb),
        )
    }

    /// Run one framed `clio` command, require success, and parse its one-line
    /// JSON report.
    fn ok(&self, verb: &str) -> Value {
        let out = self.raw(verb);
        assert_eq!(out.code, Some(0), "`clio {verb}` failed: {}", out.stderr);
        serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|error| panic!("parse the report of `clio {verb}`: {error}"))
    }

    /// Record one copy disposition on the request.
    fn copy(&self, request: &str, id: &str, custody: &str, action: &str, state: &str) -> Value {
        let verb = format!(
            "compliance request-copy --copy-id {id} --request-id {request} --custody {custody} \
             --owner harness --required-action {action} --state {state}"
        );
        self.ok(&verb)
    }

    /// Attempt one supported pre-erasure archive restore route.
    fn restore(&self, selector: &str) -> CliRun {
        run(
            &self.journal,
            self.backend.kms(),
            &self.backups,
            &restore_args(self.backend.target(), selector),
        )
    }
}

/// Drive the one composed claim against an already seeded and archived source.
fn drive_claim(claim: &Claim) {
    let request = unique("request");
    let delivered = unique("copy-delivered");
    let open_copy = unique("copy-open");
    let export = claim.export_path.to_str().expect("UTF-8 export path");

    // 1. Export one subject: the manifest must carry every family the ledger
    //    names, with the counts the every-family fixture seeded and the
    //    ledger's own omissions, and the other subject's marker must be absent
    //    from the delivered file.
    let exported = claim.ok(&format!(
        "export --out {export} --subject {DENIED_SUBJECT} --content-mode dsar_plaintext"
    ));
    let manifest = &exported["manifest"];
    family_test_support::assert_every_family(manifest, &claim.fixture);
    let subject = &manifest["filter"]["subject_id"];
    assert_eq!(subject, &json!(DENIED_SUBJECT), "{manifest}");
    let omissions = manifest["omissions"].as_array().map_or(0, Vec::len);
    let complete = manifest["complete"].as_bool();
    assert_eq!(complete, Some(omissions == 0), "{manifest}");
    let bundle = std::fs::read_to_string(&claim.export_path).expect("read the bundle");
    assert!(bundle.contains(DENIED_MARKER), "denied marker missing");
    assert!(!bundle.contains(CONTROL_MARKER), "control marker leaked");

    // 2. Open the durable erasure request and record the delivered copy, then a
    //    still-open required action that must gate closure.
    claim.ok(&format!(
        "compliance request-start --request-id {request} --kind erasure --subject-id \
         {DENIED_SUBJECT}"
    ));
    let copied = claim.copy(&request, &delivered, "clio", "delivery", "done");
    assert_eq!(copied["request_id"], json!(request), "{copied}");
    let opened = claim.copy(&request, &open_copy, "caller", "erase", "pending");
    let pending = opened["pending_operator_actions"]
        .as_array()
        .expect("pending actions");
    let open_pending = pending.iter().any(|a| a["action_id"] == json!(open_copy));
    let delivered_pending = pending.iter().any(|a| a["action_id"] == json!(delivered));
    assert!(open_pending && !delivered_pending, "pending: {opened}");

    // 3. Erase through the real public route, then retry: the retry is
    //    idempotent and never adds a second effect.
    let erased = claim.ok(&format!(
        "erase --subject {DENIED_SUBJECT} --basis gdpr-art-17 --request {request} --confirm"
    ));
    let effects = erased["effects_after"].as_u64().unwrap_or(0);
    assert!(effects >= 1, "erase effects: {effects}");
    let retried = claim.ok(&format!(
        "compliance request-retry --request-id {request} --subject-id {DENIED_SUBJECT} \
         --legal-basis GDPR_ART_17 --confirm"
    ));
    let before = retried["effects_before"].as_u64().unwrap_or(0);
    let after = retried["effects_after"].as_u64().unwrap_or(0);
    assert!(before >= 1, "one durable effect: {retried}");
    assert_eq!(before, after, "retry repeats: {retried}");

    // 4. The request closes only after its required actions: the open copy
    //    refuses the close; recording it done lets the close land.
    seed_done_path(&*claim.backend.open(), &request, "erasure");
    let refused = claim.raw(&format!(
        "compliance request-close --request-id {request} --product-path erasure"
    ));
    assert_open_copy_refusal(refused.code, &refused.stderr);
    claim.copy(&request, &open_copy, "caller", "erase", "done");
    let closed = claim.ok(&format!(
        "compliance request-close --request-id {request} --product-path erasure"
    ));
    assert_eq!(closed["request_outcome"], json!("completed"), "{closed}");

    // 5. Every supported pre-erasure archive restore route refuses, and the
    //    other subject stays readable in the target each time.
    for selector in [
        claim.archive_id.clone(),
        format!("{} --no-pre-backup", claim.archive_id),
        format!("{} --dry-run", claim.archive_id),
        "--latest".to_owned(),
    ] {
        let out = claim.restore(&selector);
        assert_refusal(out.code, &out.stderr, &DENIED_SUBJECT_REFUSAL);
        assert_eq!(
            support_test_support::readback(&*claim.backend.open(), CONTROL_ITEM),
            Readback::Readable,
            "the control subject stays readable after restore {selector:?}"
        );
    }

    println!(
        "REFUSED {} routes=4 archive={} request={request} copy={delivered}",
        claim.name, claim.archive_id
    );
}

/// Seed one backend, cut its pre-erasure archive, and drive the claim.
fn run_backend(backend: Backend, scratch: &crate::common::Scratch, name: &'static str, tag: &str) {
    let backups = case_fixtures::backup_dir(scratch);
    let journal = case_fixtures::journal_file(scratch, "journal/erasure-journal.json");
    let _key_guard = install_key(&journal);
    let fixture = backend.seed();
    initialize_new_empty(&journal).expect("initialize the deployment journal");
    let archive_id = backend.archive(&backups, &journal, &unique(tag));
    let export_path = scratch.join("export/bundle.json");
    std::fs::create_dir_all(export_path.parent().expect("the export path has a parent"))
        .expect("create the export directory");
    drive_claim(&Claim {
        name,
        backend,
        journal,
        backups,
        export_path,
        archive_id,
        fixture,
    });
}

/// The SQLite arm: a real export, erase, and retry, then every restore route.
#[test]
fn sqlite_export_erase_then_every_restore_route_refuses() {
    let scratch = case_fixtures::scratch("scenario-export-erase-sqlite");
    let backend = Backend::Sqlite {
        source: scratch.join("source/clio.db"),
        target: scratch.join("target/clio.db"),
        kms: scratch.join("clio.kms.json"),
    };
    run_backend(backend, &scratch, "sqlite", "scenario-export-erase-sqlite");
}

/// The loopback-Postgres arm: the same claim against isolated schemas.
#[test]
fn postgres_export_erase_then_every_restore_route_refuses() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let source = ScopedSchema::new(&base);
    let target = ScopedSchema::new(&base);
    let scratch = case_fixtures::scratch("scenario-export-erase-postgres");
    let backend = Backend::Pg {
        source: source.url().to_owned(),
        target: target.url().to_owned(),
        kms: scratch.join("clio.kms.json"),
    };
    run_backend(
        backend,
        &scratch,
        "postgres",
        "scenario-export-erase-postgres",
    );
}
