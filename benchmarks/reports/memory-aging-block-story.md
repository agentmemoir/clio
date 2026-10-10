# Memory aging and provenance: what the block changed, what it measured, and what is still open

This story is for a reader who was not there. It records what the memory aging and provenance block changed in Clio, what was actually measured, which constants are still unmeasured, what remains open, and how to reproduce the comparison. It covers the block's written acceptance outcomes in order, each one marked as supported by a named artefact or listed as unsupported with its reason.

Every claim below cites a public artefact in this repository, or says plainly that no public citation exists for it. The story repeats no number that an artefact already holds: where a value matters, the sentence names the artefact that holds it.

## What the block changed, in order

- One command now drives two independent memory systems on the same pinned suite through the same adapter interface, so both systems are partitioned, judged, and tokenized the same way (`benchmarks/README.md:38`).
- Every harness report now names the settings that change each system's behaviour, through a per-system configuration fingerprint captured before any probe runs (`benchmarks/README.md:55`).
- One pure module now computes every age-shaped value in the product, and all four existing call sites use it. The retention curve is documented as a second, independent multiplier applied exactly once to a candidate's shaped score (`crates/clio-retrieve/src/hybrid_rank.rs:89`).
- The additive freshness boost is gone from the shaping step, and so is the temporal retrieval weight that fed it, leaving the retention curve as the only age response in ranking (`CHANGELOG.md:212`, `CHANGELOG.md:235`).
- An item written a year ago now ranks below an otherwise identical item written today, through a half-life-shaped age factor that can only lower a score (`CHANGELOG.md:212`). The admission score's recency factor was converted to the same one half-life form, so one name means one thing in both places (`CHANGELOG.md:329`).
- Every hit's `scores` object now reports the age factor that was applied to it, carried from the shaping step so the reported value is exactly the applied one (`CHANGELOG.md:99`, `crates/clio-retrieve/src/stage_score.rs:107`).
- A caller can restrict retrieval to a provenance floor and receive a provably correct set: a hit below the floor is removed from the answer set rather than ranked lower (`CHANGELOG.md:81`).
- Provenance now also refines a hit's rank by a bounded, monotone trust multiplier, and every hit reports the value that was applied to it (`CHANGELOG.md:210`, `CHANGELOG.md:67`).
- Every memory hit in a composed pack contributes exactly one marked form, the pack reports a per-form tally, and a caller can ask a composed pack for exact values, which the pack answers from the authoritative half (`CHANGELOG.md:53`, `CHANGELOG.md:65`, `CHANGELOG.md:42`).
- One read of `maintenance_status` now tells an operator what structural work is waiting, with one count per kind, by component (`CHANGELOG.md:40`).
- Structure now refreshes itself when there is a reason to: trigger evaluation rides the existing sweeper thread, and each maintenance wave runs on its own thread (`CHANGELOG.md:38`).
- A maintenance wave that is interrupted now resumes instead of repeating work, so the next wave finds the bank exactly as the interruption left it (`CHANGELOG.md:359`).
- An operator can now see a bank's age distribution and provenance mix from the `stats` surface that already reports the inventory (`CHANGELOG.md:28`).
- An operator can ask whether a correction actually outranks what it corrected, through a supersession section in the `inspect` tool that reports rather than repairs (`CHANGELOG.md:15`).
- Two named deployment profiles now exist and are loadable, and the harness runs one run per profile (`CHANGELOG.md:13`, `benchmarks/profiles/README.md:1`).

## The seven acceptance outcomes, in order

The seven outcomes below are the block's written overall acceptance outcomes. Each is marked supported by a named artefact, or unsupported with its reason. No public citation exists for the outcome wording itself.

1. **An aged item is demonstrably and visibly demoted.** Supported. The retention curve multiplies every candidate's shaped score by a factor that can only lower it, ships on by default, and every hit reports the exact factor applied (`CHANGELOG.md:212`, `CHANGELOG.md:99`).
2. **A provenance-shaped query returns a provably correct set.** Supported. The provenance floor removes weak evidence from the answer set rather than ranking it lower, and a mistyped floor is rejected rather than ignored (`CHANGELOG.md:81`).
3. **A pack states what each hit contributed and can be asked for exact values.** Supported. Each hit carries a marked form and a per-form tally, and an exact rendering can be asked for, answered from the stored snapshot rather than the narrative channel (`CHANGELOG.md:65`, `CHANGELOG.md:42`).
4. **Structure refreshes without a caller.** Supported for the behaviour: waves start on their own, the status block reports what is waiting, and an interrupted wave leaves no duplicate work (`CHANGELOG.md:38`, `CHANGELOG.md:40`, `CHANGELOG.md:359`). The latency half of this requirement is not supported, and is listed below.
5. **The inventory answers what the memory is made of.** Supported. The `stats` surface reports age-band counts, provenance-class counts, and maintenance pressure, and defines its tier word exactly (`CHANGELOG.md:28`).
6. **Four age curves have become one implementation with one age response per hit.** Supported. The freshness boost and the temporal weight are gone, the admission recency factor uses the same half-life form, and the curve is applied exactly once per hit (`CHANGELOG.md:212`, `CHANGELOG.md:235`, `CHANGELOG.md:329`, `crates/clio-retrieve/src/hybrid_rank.rs:89`).
7. **A reader can reproduce the before-and-after comparison from the repository without asking anyone what settings were used.** Supported. The comparison report records the exact commands, the equal harness settings, and every profile value, and the harness README documents how to run it (`benchmarks/reports/two-profiles-comparison.md:11`, `benchmarks/README.md:123`). The reproduction steps are restated below.

## What remains unsupported

One block outcome is unsupported and is listed here with its reason, so this story does not read as a clean sheet.

- **The trigger-off/on time-to-queryable latency comparison could not be produced.** The reason: the metric has no shipped producer, the measurement campaign is a recorded null result, the pre-committed bound was never exercised, and the repair is deferred to a new phase. The automatic-maintenance requirement says maintenance shall not extend time-to-queryable, so the behaviour shipped without its measurement.

## What is explicitly not an acceptance criterion

The block's acceptance rules name one thing they are not: a better score than any other system. The comparison exists to make our own decisions defensible, not to win. If another system scores higher on a category, the correct response is a named, reproducible case and a decision from the project's written decision process — never a factor re-tuned until the number moves. A reader who sees a rival's higher score should file the case, not retune the constants.

## Constant status

The block's comparison report is the sole authority on constant status, and its rule is adopted here verbatim: "Every constant below is an `unmeasured starting point`. None is justified by a tier-2 measurement, so none may be quoted as a measured result."

There are two tiers only: justified by a tier-2 measurement, or an unmeasured starting point. Every constant the block ships is currently an unmeasured starting point. The values, their source names, and their locations live in the comparison report's constant-status section and are not restated here (`benchmarks/reports/two-profiles-comparison.md:72`).

## Open findings

The completeness-check phase accepted this block with limitations, and the story states them, so a reader does not inherit a clean sheet.

- The completeness check found raw superseded-row residue in the store's nearest-neighbour read: closing a superseded item's interval deletes neither its embedding row nor its full-text row. This is recorded as open issue #124 in the public repository, and is not fixed by this block.
- No confirmed defect case was filed by the completeness check itself; that filing decision remains open.
- The completeness check's changelog entry exists, but its pull-request link is pending. The supersession ordering verdicts are proved at the report layer, by the report builder's two-member-page test (`crates/clio-mcp/src/read_inspect_supersession_tests.rs:236`), not yet by a production page.
- Remediation notes were recorded: first, decide whether the `inspect` supersession section accepts the full retrieve control set (domains, as-of, graph expansion, and minimum-score controls) or documents that it always reports the default-control page; second, a Postgres-side finding deferred with the same remediation list.
- The time-to-queryable latency comparison listed above stays unsupported until a new phase ships the metric's producer.

## How to reproduce the comparison

A reader can reproduce the recorded comparison without asking anyone and without any private data or frontier API key. The recorded runs used the harness's own stub judge and generator, so no key is needed, and the stub tier is documented as wiring-only (`benchmarks/reports/two-profiles-run-notes.md:1`).

1. Build the Clio binary and read the harness README's running section (`benchmarks/README.md:123`).
2. Load the first profile with `clio retention set`, as recorded verbatim in the comparison report (`benchmarks/reports/two-profiles-comparison.md:18`).
3. Run the harness with the stub judge and generator, with the profile store named by its environment variable, as recorded verbatim in the same report (`benchmarks/reports/two-profiles-comparison.md:19`).
4. Repeat the two steps with the second profile (`benchmarks/reports/two-profiles-comparison.md:25`, `benchmarks/reports/two-profiles-comparison.md:26`).
5. Compare the two reports. Every harness setting the runs held equal, and the one setting they differ in, is written down in the same report (`benchmarks/reports/two-profiles-comparison.md:11`).

The runs are smoke runs at the stub tier, so a reproduced comparison will not produce a quotable baseline. A quotable baseline needs a frontier key, and the harness refuses to substitute a lower tier for it (`benchmarks/README.md:145`).

## Known limitations

These are the block's recorded limitations, so a reader knows what the artefacts can and cannot show.

- The new adapter answers presence-only: it records whether a system produced retrieval, not retrieval quality beyond the probe scores (`benchmarks/README.md:46`).
- The other system's ranking levers are unpinned by choice: the fingerprint records what each system's read surface exposes, and a run with a required setting unreadable fails rather than reporting half a configuration (`benchmarks/README.md:55`).
- There is no quotable baseline, because no frontier key existed on the host when the runs were taken, and both runs are stub tier (`benchmarks/reports/two-profiles-comparison.md:7`).
- The fingerprint cannot see the newest profile fields: the read surface omits two of the profile differences, so the report does not claim the fingerprint proves those two (`benchmarks/reports/two-profiles-comparison.md:7`).
- The trigger-state read is stale: the adapter records trigger state as unreadable even though the maintenance status surface now returns a trigger block, so the runs cannot say whether maintenance ran (`benchmarks/reports/two-profiles-run-notes.md:1`).
- The half-life has no durable profile surface: both profiles run the system default half-life, and the profiles' intended half-lives are recorded as intent only (`benchmarks/profiles/README.md:19`).
- The supersession section reports rather than repairs, and its retrieval runs with co-activation reinforcement off, so asking the question adds no write and changes nothing it examines (`CHANGELOG.md:24`).
- The admission recency factor's conversion was retained, so the admission threshold admits more older records than before at unchanged settings; the block did not retune the affected constants (`CHANGELOG.md:329`).
- The blocked-gate rule stands: with no frontier key, the baseline command writes a blocked-gate record and no score exists to quote (`benchmarks/README.md:145`).

## Requirement sweep and changelog

The closing sweep and the changelog entries are part of this work, not a later task. No public citation exists for the sweep wording. The changelog carries the entries for the behaviour changes listed above (`CHANGELOG.md:9`).