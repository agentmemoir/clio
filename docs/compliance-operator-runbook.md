# Local compliance operator runbook

One self-hosted operator runs a subject request to an honest result with local Clio commands only. No cloud account, second operator, or outside service is needed.

## Status

This draft covers the subject-request walk: scope, verification, request start, status, copy custody, erasure retry, retention exception, event review, peer requests, and closure. The archive custody, journal backup and handoff, and the guarded restore drill are separate walks and are not yet in this document.

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

## Step 2: Start the request

Open one durable subject request:

```sh
clio compliance request-start --request-id r1 --kind erasure --subject-id s1 --verification verified
```

`--kind` is `copy` or `erasure`. `--subject-id` names one covered subject; omit it for a whole bank. `--verification` records your identity-verification decision as `verified` or `unverified`; omit it when none is recorded. `--deadline` records an operator-supplied deadline when you have one.

## Step 3: Watch the request state

Report one request's state, technical completion, pending operator actions, and outcome as separate fields:

```sh
clio compliance request-status --request-id r1
```

Read the fields separately. A technical completion is a software result; it is not your copy delivery and not the final request outcome.

## Step 4: Record copy custody

Record one copy-inventory disposition for the request:

```sh
clio compliance request-copy --copy-id c1 --request-id r1 --custody clio --owner harness --required-action erase --state pending
```

`--custody` is one of `clio`, `operator`, `caller`, `peer`, `review_output`, or `unknown`. `--required-action` is one of `erase`, `remove`, `delivery`, or `notice`. `--owner` is `harness` or `system`.

`--exception-reason` records why a copy has no request result yet. Record every copy you find, including the ones you keep.

## Step 5: Run a confirmed erasure retry

Retry a confirmed erasure through its durable request id:

```sh
clio compliance request-retry --request-id r1 --subject-id s1 --legal-basis gdpr-art-17 --confirm --actor harness
```

Without `--confirm` the tool refuses and never mutates. The legal basis is recorded as given; you supply it.

## Step 6: Record a retention exception

Record a request-level retention exception when part of the data stays:

```sh
clio compliance request-exception --request-id r1 --reason retention-hold
```

An open or excepted request yields a partial or refused result, never a full closure. Do not treat a failed event as complete.

## Step 7: Review the events

Read one content-free page of the local events bound to the request:

```sh
clio compliance request-events --request-id r1 --limit 50
```

Resume a longer review with `--cursor C` after the last decimal sequence position.

Write the ordered review events to a file under destination custody:

```sh
clio compliance review-export --out review.txt --actor harness
```

Only a `harness` or `system` actor may export. The tool records the destination as pending before any bytes land and refuses to overwrite an existing file.

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
clio compliance peer-disposition --request-id r1 --peer-id p1 --disposition removal_evidence --proof-ref ref-1
```

`--disposition` is one of `removal_evidence`, `retention_approved`, or `unknown`. A removed peer needs an explicit disposition before the request can be judged.

## Step 9: Close only when eligible

Accept closure for one request and finalize its outcome:

```sh
clio compliance request-close --request-id r1 --product-path ./product.db
```

`--product-path` names the covered product path. Keep the request open while any action is pending; closure accepts the request only when the software reports it eligible. Never call an open request complete.

## What is next

This runbook grows a section only after that section is walked on a fresh local fixture. The archive custody, journal handoff, and restore drill walks come next.