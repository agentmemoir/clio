# Clio Philosophy Evaluation

**What this is.** Twenty scenarios that test whether a behaviour follows `PHILOSOPHY.md`. Run them by hand now; wire them into CI later.

**How to score.** Run the scenario. Compare the observable result with the expected line. A pass meets the expected result and breaks no belief. A fail names the belief number it broke.

| # | Scenario | Expected result | Belief |
|---|---|---|---|
| 1 | Raw unclassified text stream sent to admission | Refused at gate with category whitelisting error | 1, Hard line 1 |
| 2 | Identical memory payload sent within admission window | Duplicate detected and rejected before leaf storage | 1, Hard line 1 |
| 3 | Inbound leaf write admitted to storage | Queryable in under 2 seconds; consolidation queued in background | 1 |
| 4 | Background summarization or compaction in progress | Inbound write completes without blocking on background tasks | 1 |
| 5 | Request to ingest a 50MB PDF document library | Refused; Clio rejects bulk document RAG workloads | 2 |
| 6 | Interactive human note-taking editor requested | Refused; Clio serves machine reasoning and persona continuity | 2 |
| 7 | Structured tool execution snapshot submitted | Admitted with parameter fidelity and provenance metadata | 2 |
| 8 | Dense vector retrieval engine times out | Falls back to keyword search; response reports degradation | 3, Hard line 5 |
| 9 | Query matches no records or storage encounters failure | Returns clean empty response with diagnostic status, never fakes memory | 3, Hard line 5 |
| 10 | Single-turn model deduction submitted as ground fact | Admitted only as belief with confidence trajectory; fact status denied | 4, Hard line 3 |
| 11 | Observation reinforced across distinct sessions | Confidence trajectory accumulates; graduates to fact at saturation threshold | 4, Hard line 3 |
| 12 | Query for ground facts when only beliefs exist | Returns facts; beliefs flagged with explicit epistemic markers | 4 |
| 13 | Record value changes (e.g., entity attribute update) | Closes bi-temporal validity interval of old record; old record queryable | 5, Hard line 2 |
| 14 | In-place destructive overwrite attempted on past fact | Storage engine disallows in-place overwrite; history preserved | 5, Hard line 2 |
| 15 | Time-travel query with past timestamp | Returns fact valid at specified timestamp, not current state | 5 |
| 16 | Compliance erasure requested for a subject | Crypto-shreds subject key and logs tombstone; data irrecoverable | 5, Hard line 6 |
| 17 | Sync or export of memory payload to remote relay | Encrypted client-side with zero-knowledge keys before transport | Hard line 4 |
| 18 | Fresh boot on empty database | Bootstraps schema migrations automatically with zero manual ritual | 6 |
| 19 | Thirty days of continuous operation | Autonomous link decay and background compaction run without manual chore | 6 |
| 20 | Contributor or agent proposes behaviour change | Commit or pull request cites governing belief number | How to use |

**Status.** This is a manual rubric. It is not wired to CI, and no run has been recorded yet. The first recorded run becomes the baseline.

**A fail is information.** Fix the behaviour or change the belief through the recorded decision in `PHILOSOPHY.md`. Do not quietly edit the expected result.

Next: `PHILOSOPHY.md` for the beliefs, and `README.md` for project setup.
