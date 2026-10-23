# Local compliance operator runbook

One self-hosted operator runs a subject request to an honest result with local Clio commands only. No cloud account, second operator, or outside service is needed.

## Status

This document covers the full local walk: scope inventory, verification, request start, status, copy custody, subject export and delivery, erasure retry, retention exception, event review, peer requests, archive custody, erasure journal backup and handoff, the guarded restore drill, and closure. Every command and refusal below was run and observed on a fresh local sample database unless the text names shipped tests as the evidence.

## Before you start

Protect the local journal file like a key: whoever holds it can replay your store's authority.

Record where every exported file, captured log, and archive lives, and who holds it. A copy you cannot name is a copy you cannot erase.

Every example uses redacted synthetic data. The commands never print signing keys or raw identity proof, and you must not paste either into a request record.

The software records your identity-verification decision, your deadline, and your legal basis. It never verifies a subject's identity and never selects a legal deadline; those decisions are yours.

## Store connection flags

Store commands accept these connection flags. The archive commands `backup list` and `backup verify` do not: they read archives, not a store, and refuse `--db`.

- `--db PATH` opens this store instead of the configured one.
- `--backend sqlite|postgres` selects the store backend.
- `--bank BANK` scopes the command to one bank.
- `--actor ACTOR` names the acting identity.
- `-o json` or `-o text` selects the output mode.

The `--backend` and `--db` flags work before or after the subcommand.

Five compliance commands need a compliance-privilege actor: `request-copy`, `request-exception`, `peer-disposition`, `request-events`, and `review-export`. Without `--actor harness` or `--actor system` they refuse with `local event review requires a harness or system actor with compliance privilege`. The steps below show `--actor harness` on those commands. Other commands such as `request-start`, `request-status`, `peer-status`, `peer-retry`, and `request-close` run without this gate. `request-retry` and a direct `clio erase` still reach this same authorization gate, and the CLI's default actor is an agent, so pass `--actor harness` or `--actor system` on those commands too.

## Where the evidence lives

Each step leaves evidence in a place you can name:

- Step 1: the inventory rows and the declared-copy rows live in the store you open with `--db`; `clio compliance inventory` reads them back.
- Step 2 and Step 3: the durable request record lives in the same store; `clio compliance request-status` reads it back.
- Step 4: the copy-inventory row and the event-review entry live in the store; `clio compliance request-events --request-id r1 --actor harness` reads the entry back.
- Step 5: the bundle file at your `--out` path, plus the `complete` flag and the content hash the command prints.
- Step 6 and Step 10: refusals and erasure counts print on your terminal with a nonzero exit code, and a retry's counts also land in the store's event rows.
- Step 7 and Step 8: the event rows live in the store; `request-events` reads them, and `review-export` writes the ordered copy to the `--out` file you record in custody.
- Step 9: the peer report prints in the command output, and a recorded disposition lives in the store.
- The archive steps: the archive files under the path `clio backup dir` shows, the per-member digests `backup verify` prints, and the journal file at the path named in the Erasure journal section.

## Step 1: Record the scope and known copies

Read the stored record families and known copies before you open a request:

```sh
clio compliance inventory
```

A row whose link or source is missing stays open and is named. The complete flag stays false while anything is open or unclassified.

Declare each copy you already hold so the run records it without touching it:

```sh
clio compliance inventory --copy ./exports/a.jsonl
```

The one-line help does not list `--copy`, but the flag works: it declares an existing caller file, records it, and never touches the file. A declared path that does not exist is not an error: the command exits 0, and the inventory row names the missing copy with `declared copy not found at <path>`.

## Step 2: Start the request

Open one durable subject request:

```sh
clio compliance request-start --request-id r1 --kind erasure --subject-id s1 --verification verified
```

`--kind` is `copy` or `erasure`. `--subject-id` names one covered subject; omit it for a whole bank. `--verification` records your identity-verification decision as `verified` or `unverified`; omit it when none is recorded. `--deadline` records an operator-supplied deadline when you have one.

With peers disabled the request reports `peer_enablement:"not_applicable"`. See the Peers section.

## Step 3: Watch the request state

Report one request's state, technical completion, pending operator actions, and outcome as separate fields:

```sh
clio compliance request-status --request-id r1
```

Read the fields separately. A technical completion is a software result; it is not your copy delivery and not the final request outcome.

## Step 4: Record copy custody

Record one copy-inventory disposition for the request:

```sh
clio compliance request-copy --copy-id c1 --request-id r1 --custody clio --owner harness --required-action erase --state pending --actor harness
```

This command writes into the request's event review, so it needs a compliance-privilege actor.

`--custody` is one of `clio`, `operator`, `caller`, `peer`, `review_output`, or `unknown`. `--required-action` is one of `erase`, `remove`, `delivery`, or `notice`. `--owner` is `harness` or `system`.

`--exception-reason` records why a copy has no request result yet. Record every copy you find, including the ones you keep.

## Step 5: Export and deliver the subject copy

Deliver the subject's stored content as one bundle file:

```sh
clio export --out ./exports/s1-bundle.json --subject s1 --actor harness
```

The command writes one bundle file plus its completeness manifest, and refuses any actor except `harness` or `system`. `--subject` bounds the bundle to one subject; omit it to export the whole store. `--content-mode dsar_plaintext` is the default, and `--content-mode ciphertext_backup` selects the backup bundle.

The command prints one line with the destination, the content mode, a `complete` flag, the entity count, and the start of the content hash. Check `complete=true` on that line before you deliver the bundle; the manifest behind it also carries the per-type counts and the full hash.

Deliver the bundle through the custody route you recorded in Before you start, then declare the delivered copy so the run records it:

```sh
clio compliance inventory --copy ./exports/s1-bundle.json
```

## Step 6: Run a confirmed erasure retry

Retry a confirmed erasure through its durable request id:

```sh
clio compliance request-retry --request-id r1 --subject-id s1 --legal-basis gdpr-art-17 --confirm --actor harness
```

Without `--confirm` the tool refuses with `subject_request_retry requires confirm=true; unconfirmed calls never mutate` and never mutates. The legal basis is recorded as given; you supply it.

A subject that lives in more than one bank refuses the retry with `the banks sharing the subject key changed after confirmation`, even when nothing changed after the request started. This was observed for the default-bank scope and for a single named bank. A single-bank subject retries cleanly and reports the erasure counts before and after. A future release owns a better wording for this refusal.

## Step 7: Record a retention exception

Record a request-level retention exception when part of the data stays:

```sh
clio compliance request-exception --request-id r1 --reason retention-hold --actor harness
```

This command also writes into the request's event review, so it needs a compliance-privilege actor.

An open or excepted request yields a partial or refused result, never a full closure. Do not treat a failed event as complete.

## Step 8: Review the events

Read one content-free page of the local events bound to the request:

```sh
clio compliance request-events --request-id r1 --limit 50 --actor harness
```

Resume a longer review with `--cursor C` after the last decimal sequence position. A cursor past the end returns empty rows with `has_more:false`.

Write the ordered review events to a file under destination custody:

```sh
clio compliance review-export --out ./review.txt --actor harness
```

Only a `harness` or `system` actor may export. `--out` needs a path with a parent directory: `./review.txt` works, and a bare name like `review.txt` refuses with `destination has no parent directory`. An export to a path whose parent exists writes the file with mode `0600` and one `seq<TAB>action_id<TAB>state` line per row, where `seq` is the row's sequence position, and a second export to an existing destination refuses with `review file export is refused: destination custody is not available`; record that exported file in your custody list.

## Step 9: Reconcile peer requests

Report the derived peer state slots of one request:

```sh
clio compliance peer-status --request-id r1
```

Reconcile the pending erasure actions a restart left on one request:

```sh
clio compliance peer-retry --request-id r1 --legal-basis OPERATOR_RETRY
```

Record one peer disposition with its durable proof reference:

```sh
clio compliance peer-disposition --request-id r1 --peer-id p1 --disposition removal_evidence --proof-ref ref-1 --actor harness
```

`--disposition` is one of `removal_evidence`, `retention_approved`, or `unknown`. A removed peer needs an explicit disposition before the request can be judged. See the Peers section for what a peers-disabled store returns.

## Step 10: Close only when eligible

Attempt closure for one request only when it is eligible:

```sh
clio compliance request-close --request-id r1 --product-path ./product.db
```

As shipped, every close attempt refuses with `request closure refused: UnknownProductPath`, on every sample database and on both backends. No shipped command records a product path into the request, so the closure gate never sees a recorded path. A future release owns the fix. Until then the documented final outcome for closure is this refusal, so keep the request open and keep treating it as open.

## Archive custody

Keep one local archive of the store and record who holds it. Set the backup directory with `CLIO_BACKUP_DIR`.

Show where archives live and whether the directory is writable:

```sh
clio backup dir
```

Estimate the archive before you create it:

```sh
clio backup create --dry-run
```

Then create it for real:

```sh
clio backup create
```

The create prints per-table counts. When the erasure journal is missing or damaged, the create still succeeds but prints this warning: `no erasure authority was recorded: the erasure journal is missing or damaged, and a restore of this archive will refuse until a current journal exists`. Fix the journal and create a fresh archive.

A create deletes nothing, so `clio backup create --confirm` refuses with `clio backup create --confirm has nothing to confirm: a create deletes nothing`; the tool hints to add `--keep N` when you want old archives pruned.

List the archives:

```sh
clio backup list
```

`backup list` reads snapshots, not a store: passing `--db` refuses with `clio backup list reads snapshots, not a store; --db cannot change the listing`.

Check one archive member by member:

```sh
clio backup verify <id>
```

The verify reports a digest per member and `consistency:"point-in-time"`. Passing `--db` to verify refuses with `clio backup verify reads an archive, not a store; --db cannot change the answer`.

## Erasure journal backup and handoff

The erasure journal is the deployment's record of erasure authority. Its path is `CLIO_ERASURE_JOURNAL_FILE` when set, else `<data-dir>/erasure-journal.json` in the store's data directory.

The journal holds hashes only, never subject text. Its shape is a schema version, a deployment id, a generation counter, a salt fingerprint, and one denial entry per erased subject with its subject hash and banks:

```json
{"schema_version":1,"deployment_id":"<hex>","generation":1,"salt_fingerprint":"<hex>","denials":[{"subject_hash":"<hex>","banks":["<bank>"]}]}
```

No CLI command backs up or hands off the journal file. The file copy is yours. Shipped commands do read and write the journal in place: `backup create` reads the live journal to record the archive's erasure authority, every confirmed erasure appends a denial entry to it, and the guarded restore reads it before any content becomes readable.

The live journal file is the erasure authority. If a stale handoff copy is swapped in as the live journal file, a pre-erasure archive can restore and make erased content readable again, so the current journal file must stay in place and handoff copies must never replace it silently. Why this gap is yours to guard: the handoff copy is an owner-held file, and no shipped tool watches the live file or pins it in place. A future release owns stronger pinning.

Each erasure appends a denial entry to the journal and bumps its generation. Copy the journal again after every erasure so the copy matches the store.

Before you make a handoff copy or attempt a restore, record the live journal's `generation` and `salt_fingerprint` from the journal file. Compare them against the handoff copy's values, and compare the generation against the archive's claimed generation, before you go on. Stop on any mismatch instead of restoring. The restore guard already enforces part of this check: it refuses an archive that claims a generation newer than the live authority's, and it refuses an archive whose salt fingerprint is missing or differs from the deployment's. What is still yours alone: no tool compares your handoff copy against the live journal file, and no tool checks that the live file is genuine. A future release owns turning the handoff-copy check into an enforced one.

Before you restore, run the journal check from the Erasure journal backup and handoff section: record the live journal's `generation` and `salt_fingerprint`, compare them against your handoff copy and the archive's claimed generation, and stop on any mismatch instead of restoring.

## Guarded restore drill

The restore guard keeps erased subjects from coming back through an archive. Take an archive while the journal is current, then run the drill:

```sh
clio backup restore --latest --yes
```

The guard refuses before any content becomes readable, in these observed cases:

- An archive that still carries usable key material for a currently denied subject refuses with `an archive member carries usable key material for a currently denied subject`.
- A journal file that is missing refuses with `the erasure journal is missing`.
- A journal copied before a later erasure refuses before old content becomes readable, in two observed ways. If the archive claims a newer generation than the live journal, the refusal is `the archive claims a generation newer than the live authority's`. If the archive is of the same generation and still carries usable key material for a denied subject, the refusal is `an archive member carries usable key material for a currently denied subject`.
- An archive taken before any journal existed refuses with `the archive records no erasure authority`.

The guard refuses on usable key material, not on bare presence. A backup taken from a store after its erasure keeps only content-free records for the denied subject — a destroyed key slot, a tombstone, or a ciphertext row without a usable DEK — and those restore cleanly because no key in the archive can make the content readable again. An archive that still carries the subject's key is refused as a whole. A member the guard cannot classify refuses too. The shipped restore-guard and archive-presence scenarios document both the allowed content-free restore and the refusal.

A refusal is not a defect: do not strip markers, reset the journal, or fabricate new authority to make a restore pass.

The passing control: a second store that shares the same current journal and holds no denied-subject references restores cleanly with `clio backup restore --yes` and exits 0. The restore takes a pre-restore snapshot, replaces the key store, and keeps unrelated subjects readable; a survivor item was still present after the restore when checked with `clio inspect`.

Caveat: supported Clio restore refuses old or missing authority, but an operator-held old archive may still be readable outside supported Clio restore. Keep every archive under recorded custody like any other copy.

## Peers

With peers disabled, the core path still runs. The request reports `peer_enablement:"not_applicable"` at start, `peer-status` returns the bare report `{"ok":true}` because there are no peer slots, and `peer-disposition` for a peer id refuses with ``no peer action row exists for request `r1` peer `p1` `` because the peers-disabled store has no peer rows.

When a request requires enabled peers, the shipped behavior is that an offline peer stays pending, a reconnecting peer is denied before it can read or replay, and a removed peer needs an explicit disposition. The shipped acceptance tests document the delivery and replay parts; the CLI walk below proves the rest on a fresh local sample database.

### Two-peer drill

A two-peer drill is reachable from the shipped CLI. Two shipped commands create the enabled peer records, both run and observed on a fresh local sample database: `clio auth token add --label peer-a --actor user --bank default -o json` once per peer (each credential record becomes one enabled peer, and the verb refuses `--db` because it manages the token file, not a store), and `clio config set sync.omit false --scope deployment -o json` (which brings sync into play, the condition the request opener checks before treating records as peers). Then `clio auth token list -o json` shows both records.

The request then opens with peers required, run and observed on the same sample database: `clio compliance request-start --request-id r1 --kind erasure --subject-id s1 --verification verified --actor harness -o json` returns `"peer_enablement":"required"` with one pending slot per record, `"peers":{"peer-a":"pending","peer-b":"pending"}`, frozen at start.

Per-peer behavior from the same walk, each observed in command output: `clio compliance peer-status --request-id r1 --actor harness -o json` returns `{"ok":true,"peer-a":"pending","peer-b":"pending"}`, the offline default. `clio compliance peer-retry --request-id r1 --legal-basis gdpr-art17 --actor harness -o json` succeeds but changes no CLI-visible slot. `clio compliance peer-disposition --request-id r1 --peer-id peer-a --disposition removal_evidence --actor harness -o json` accepts `removal_evidence`, `retention_approved`, or `unknown` for an existing peer row (the `removed` spelling refuses as a usage error), and an unknown peer id refuses with ``no peer action row exists for request `r1` peer `p9` ``. Both dispositions in this walk recorded no `--proof-ref`, and a `removal_evidence` disposition without a proof reference does not settle the peer slot. A proof-bearing disposition does settle it; the shipped test `crates/clio/tests/cross_path_compliance/scenario_all_bank_peer.rs` walks that case. After both dispositions, `peer-status` and `request-status` still show the slots `pending` and `technical_completion` `pending`, so do not read closure progress from a disposition alone. `clio compliance request-events --request-id r1 --actor harness -o json` returned zero rows for this walk, and `clio compliance request-close --product-path prod/x` still refuses with `request closure refused: UnknownProductPath`, unchanged by the two-peer setup.

Three behaviors stay outside this CLI walk, and the shipped tests in `crates/clio/tests/peer_erasure_acceptance/` carry them: peer delivery and replay denial (no CLI command delivers to a peer or replays a peer result; `scenarios_peer.rs` and `scenarios_replay.rs` cover it), visibility of a recorded disposition in a CLI read (`scenarios_disposition.rs`), and per-peer event rows (none appeared in `request-events` during the walk).

## Known limitations

Each recorded gap names what is missing, why, and who owns the debt.

`clio compliance request-close` always refuses with `request closure refused: UnknownProductPath`. What is missing: no shipped command records a product path into the request, so the closure gate never sees one. Why: the request record opens with no path slots and nothing fills them. A future release owns the fix. Until then, Step 10 documents the observed refusal as the final closure outcome.

A direct confirmed `clio erase` can refuse with `the banks sharing the subject key changed after confirmation` for a subject that lives in more than one bank, even when the bank set has not changed after the request started. What is missing: a refusal that tells the operator what to check next. Why: the preflight reads the live bank list from the store just before the erasure and compares it with the confirmed scope, and the check fails closed on any disagreement, so a stale or separately configured bank list can never widen the erasure. A future release owns a better refusal. Until then, check the subject's banks and retry, as Step 6 records for the retry path.

## What is next

This runbook grows a section only after that section is walked on a fresh local sample database. The walk now covers the full subject request, archive custody, journal backup and handoff, and the guarded restore drill. Where a behavior was documented from the shipped tests instead of a local run, the text says so.