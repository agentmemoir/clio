# Local compliance operator runbook

One self-hosted operator runs a subject request to an honest result with local Clio commands only. No cloud account, second operator, or outside service is needed.

## Status

This document covers the full local walk: scope inventory, verification, request start, status, copy custody, erasure retry, retention exception, event review, peer requests, archive custody, erasure journal backup and handoff, the guarded restore drill, and closure. Every command and refusal below was run and observed on a fresh local fixture unless the text names shipped tests as the evidence.

## Before you start

Protect the local journal file like a key: whoever holds it can replay your store's authority.

Record where every exported file, captured log, and archive lives, and who holds it. A copy you cannot name is a copy you cannot erase.

Every example uses redacted synthetic data. The commands never print signing keys or raw identity proof, and you must not paste either into a request record.

The software records your identity-verification decision, your deadline, and your legal basis. It never verifies a subject's identity and never selects a legal deadline; those decisions are yours.

## Store connection flags

Every command in this runbook accepts the same connection flags:

- `--db PATH` opens this store instead of the configured one.
- `--backend sqlite|postgres` selects the store backend.
- `--bank BANK` scopes the command to one bank.
- `--actor ACTOR` names the acting identity.
- `-o json` or `-o text` selects the output mode.

The `--backend` and `--db` flags work before or after the subcommand.

Commands that write request records or read the event review need a compliance-privilege actor: without `--actor harness` or `--actor system` they refuse with `local event review requires a harness or system actor with compliance privilege`. The steps below show `--actor harness` where it is required.

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

## Step 5: Run a confirmed erasure retry

Retry a confirmed erasure through its durable request id:

```sh
clio compliance request-retry --request-id r1 --subject-id s1 --legal-basis gdpr-art-17 --confirm --actor harness
```

Without `--confirm` the tool refuses with `subject_request_retry requires confirm=true; unconfirmed calls never mutate` and never mutates. The legal basis is recorded as given; you supply it.

A subject that lives in more than one bank refuses the retry with `the banks sharing the subject key changed after confirmation`, even when nothing changed after the request started. This was observed for the default-bank scope and for a single named bank. A single-bank subject retries cleanly and reports the erasure counts before and after.

## Step 6: Record a retention exception

Record a request-level retention exception when part of the data stays:

```sh
clio compliance request-exception --request-id r1 --reason retention-hold --actor harness
```

This command also writes into the request's event review, so it needs a compliance-privilege actor.

An open or excepted request yields a partial or refused result, never a full closure. Do not treat a failed event as complete.

## Step 7: Review the events

Read one content-free page of the local events bound to the request:

```sh
clio compliance request-events --request-id r1 --limit 50 --actor harness
```

Resume a longer review with `--cursor C` after the last decimal sequence position. A cursor past the end returns empty rows with `has_more:false`.

Write the ordered review events to a file under destination custody:

```sh
clio compliance review-export --out ./review.txt --actor harness
```

Only a `harness` or `system` actor may export. `--out` needs a path with a parent directory: `./review.txt` works, and a bare name like `review.txt` refuses with `destination has no parent directory`. An export to a path whose parent exists writes the file with mode `0600` and one `action_id<TAB>state` line per row, and a second export to an existing destination refuses with `review file export is refused: destination custody is not available`; record that exported file in your custody list.

## Step 8: Reconcile peer requests

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

## Step 9: Close only when eligible

Attempt closure for one request only when it is eligible:

```sh
clio compliance request-close --request-id r1 --product-path ./product.db
```

As shipped, every close attempt refuses with `request closure refused: UnknownProductPath`, on every fixture and on both backends. No shipped command records a product path into the request, so the closure gate never sees a recorded path. A future priced phase owns the fix. Until then the documented final outcome for closure is this refusal, so keep the request open and keep treating it as open.

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

No CLI command reads or writes the journal. Its backup and handoff are an owner-held file copy: copy the file to the custody location yourself and record who holds it.

The live journal file is the erasure authority. If a stale handoff copy is swapped in as the live journal file, a pre-erasure archive can restore and make erased content readable again, so the current journal file must stay in place and handoff copies must never replace it silently. Any stronger pinning is owned by a future priced phase.

Each erasure appends a denial entry to the journal and bumps its generation. Copy the journal again after every erasure so the copy matches the store.

## Guarded restore drill

The restore guard keeps erased subjects from coming back through an archive. Take an archive while the journal is current, then run the drill:

```sh
clio backup restore --latest --yes
```

The guard refuses before any content becomes readable, in these observed cases:

- An archive that references a currently denied subject refuses with `an archive member references a currently denied subject`.
- A journal file that is missing refuses with `the erasure journal is missing`.
- A journal copied before a later erasure refuses before old content becomes readable, in two observed ways. If the archive claims a newer generation than the live journal, the refusal is `the archive claims a generation newer than the live authority's`. If the archive is of the same generation and still holds denied references, the refusal is `an archive member references a currently denied subject`.
- An archive taken before any journal existed refuses with `the archive records no erasure authority`.

The guard applies a strict presence predicate to every archive member. A backup that keeps only a destroyed key slot or a tombstone for a denied subject still refuses. Ciphertext rows without a usable data-encryption key still count as presence. A member the guard cannot classify refuses too. The last two behaviors were not run locally; the shipped restore-guard tests document them.

A refusal is not a defect: do not strip markers, reset the journal, or fabricate new authority to make a restore pass.

The passing control: a second store that shares the same current journal and holds no denied-subject references restores cleanly with `clio backup restore --yes` and exits 0. The restore takes a pre-restore snapshot, replaces the key store, and keeps unrelated subjects readable; a survivor item was still present after the restore when checked with `clio inspect`.

Caveat: supported Clio restore refuses old or missing authority, but an operator-held old archive may still be readable outside supported Clio restore. Keep every archive under recorded custody like any other copy.

## Peers

With peers disabled, the core path still runs. The request reports `peer_enablement:"not_applicable"` at start, `peer-status` returns the bare report `{"ok":true}` because there are no peer slots, and `peer-disposition` for a peer id refuses with ``no peer action row exists for request `r1` peer `p1` `` because the peers-disabled store has no peer rows.

When a request requires enabled peers, the shipped behavior is that an offline peer stays pending, a reconnecting peer is denied before it can read or replay, and a removed peer needs an explicit disposition. The shipped subject-peer fixture tests document these behaviors. No shipped command creates enabled peer records, so a two-peer drill from the CLI alone is not reachable; the repo's own tests carry that drill.

## Known limitations

`clio compliance request-close` always refuses with `request closure refused: UnknownProductPath`. What is missing: no shipped command records a product path into the request, so the closure gate never sees one. Why: the request record opens with no path slots and nothing fills them. A future priced phase owns the fix. Until then, Step 9 documents the observed refusal as the final closure outcome.

## What is next

This runbook grows a section only after that section is walked on a fresh local fixture. The walk now covers the full subject request, archive custody, journal backup and handoff, and the guarded restore drill. Where a behavior was documented from the shipped tests instead of a local run, the text says so.