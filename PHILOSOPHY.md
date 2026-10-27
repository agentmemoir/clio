# The Clio Philosophy

This document defines the core beliefs, hard lines, and design boundaries of Clio. It is not a feature roadmap or style guide; it is the constitutional rules every design, plan, and pull request must obey. It is built from three sources: the project's codebase truths, architectural research, and the operator's decisions.

This document is for everyone who designs, writes, reviews, or operates Clio, human or agent. When any other document disagrees with this one, this one wins.

## Terms

| Term | Meaning |
| --- | --- |
| `Fact` | Verifiable observation with stable source provenance identity. |
| `Belief` | Inferred proposition tracking confidence trajectories across sessions. |
| `Invalidation` | Closing a fact's validity time interval while preserving historical auditability. |
| `Crypto-Shredding` | Irrevocable data erasure accomplished by destroying the subject encryption key. |
| `Zero-Knowledge Sync` | Relay-blind synchronization using domain-separated client encryption. |

---

## Beliefs

### 1. Memory is governed at the gate and consolidated in the background

Unfiltered storage poisons model attention and exhausts token budgets. Clio filters every write through strict category whitelisting, multi-factor scoring, and duplicate rejection before admission. Leaf writes become queryable in under two seconds, while hierarchical summarization, associative graph decay, and pruning run asynchronously in the background.

* **Right:** Rejecting low-signal conversational noise at admission and queuing dirty memory paths for background distillation.
* **Wrong:** Admitting raw event streams without classification, or blocking write responses until a global summary tree rebuilds.
* **Forbids:** Ingesting unclassified text streams, and synchronous blocking of writes on background consolidation.

### 2. Built for autonomous agents and companion bots, not document RAG or human note apps

Clio serves machine-to-machine reasoning in autonomous agents and persistent persona continuity in companion bots. It optimizes for deterministic retrieval, tool parameter fidelity, and session recall. It refuses generic unstructured document search and human note management.

* **Right:** Storing structured tool execution snapshots and compact persona states for autonomous agent turns.
* **Wrong:** Ingesting multi-megabyte PDF libraries for bulk chunked vector search or building interactive note-taking editors.
* **Forbids:** General-purpose bulk document RAG workloads, and manual human note-taking user interfaces.

### 3. Degrade gracefully, but always tell the truth

When components degrade or storage times out, Clio falls back to resilient alternatives (such as BM25 keyword search over dense vectors). However, Clio never fakes memory or conceals failures. Every response explicitly discloses its operational state, query degradation, and write rejection reasons in structured metadata.

* **Right:** Falling back to keyword search when the vector index times out and reporting the fallback in response metadata.
* **Wrong:** Returning an empty array or fabricated context when a query fails, or reporting success when a write was rejected.
* **Forbids:** Silently faking memory, returning synthetic success on failed operations, and crashing when a safe fallback exists.

### 4. Inference is separated from fact, graduating only through verified reinforcement

What an agent observes is a fact; what an agent deduces is a belief. Facts require direct provenance and stable source identity. Beliefs carry confidence trajectories and epistemic markers. A belief may graduate into a fact only when multi-session Hebbian reinforcement crosses an explicit saturation threshold with an auditable provenance trail.

* **Right:** Storing an observed API port as a fact with source evidence, and storing user preferences as a belief with a confidence score.
* **Wrong:** Allowing an LLM's speculative hypothesis to be saved as an ungrounded ground fact on a single turn.
* **Forbids:** Silent graduation of model inferences into facts, and storing facts without source provenance.

### 5. The past is invalidated, never erased or overwritten

The world changes, but history does not disappear. When facts change, Clio closes the valid-time and transaction-time intervals of the previous record. Old truths remain queryable for historical context and auditing; they never masquerade as current state. Destruction of data occurs only through cryptographic compliance shredding.

* **Right:** Setting `valid_until` on an old address when a user moves, keeping the historical interval intact for time-travel queries.
* **Wrong:** Executing an in-place SQL `UPDATE` that overwrites previous values and destroys historical visibility.
* **Forbids:** Destructive in-place overwrites of historical knowledge during normal state changes.

### 6. Zero ritual by default, full control when configured

Autonomous agents run unattended across days and weeks. Clio must maintain itself out of the box with zero operator toil: auto-running migrations, automatic Hebbian link decay, and autonomous background compaction. Power users and administrators retain explicit configuration knobs to tune policies, thresholds, and storage engines.

* **Right:** Bootstrapping schema migrations and pruning decayed associative links automatically without manual intervention.
* **Wrong:** Requiring cron jobs, manual vacuum scripts, or DBA commands before an agent can safely restart.
* **Forbids:** Mandatory manual maintenance rituals for normal operation, and unconfigurable hardcoded policies for power users.

---

## Hard lines

* Never admit raw, unclassified text streams without category validation and duplicate checks.
* Never overwrite historical facts in place; close bi-temporal validity intervals instead.
* Never graduate an inference into ground fact without multi-session reinforcement and full provenance.
* Never sync or export memory payloads without client-side zero-knowledge encryption.
* Never fake memory or conceal retrieval degradation behind silent empty results.
* Never destroy personal data without cryptographic key shredding and tombstone logging.

---

## What Clio is not

* Clio is not a generic vector database or unstructured document RAG engine.
* Clio is not a human-facing manual note-taking or personal knowledge management tool.
* Clio is not a passive append-only event log.
* Clio is not an ungrounded summarizer that paraphrases away exact numbers, dates, or parameters.
* Clio is not an interactive chat assistant; it is the persistent memory substrate for agents.

---

## How to use this document

* Read it before you plan. Design against it, not around it.
* When a plan breaks a belief, change the plan, not the belief.
* When two beliefs collide, the one numbered lower wins. If they cannot be ordered, stop and settle it here first.

## How this document changes

* Changes need the owner's explicit approval.
* Every change carries a dated note: what changed, and why.

## The test

A design is true to Clio only if it passes every belief and breaks no hard line. If it fails one, it is not Clio, whatever the roadmap says.

