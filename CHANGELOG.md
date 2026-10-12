# Changelog

All notable changes to Clio. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). See
[CONTRIBUTING.md](CONTRIBUTING.md) for how to add an entry, and for the choices
we made where that specification is silent.

## [Unreleased]

### Added

- Every command registry record now carries its positional arguments, its working aliases, and two to four worked examples, and per-group checks pin each record's value kinds, allowed values, alias pairs, hygiene entries, and examples to the code that reads them, so a wrong record fails its own test. ([#83](https://github.com/agentmemoir/clio/pull/83))

### Fixed

- The phase-timer test suite tells a temporary directory that refuses to run a file apart from a probe that could not run, so a loaded host skips the check instead of failing it. [Issue #60](https://github.com/agentmemoir/clio/issues/60) [PR #146](https://github.com/agentmemoir/clio/pull/146)
- Correct the `clio auth token add --help` note so it lists only `--db` and `--backend` as refused while the required `--bank` flag stays in the usage line. [Issue #142](https://github.com/agentmemoir/clio/issues/142) [PR #144](https://github.com/agentmemoir/clio/pull/144)
- Scoped coverage runs remove old executable objects for selected packages and targets before rebuilding, so stale source maps cannot inflate a changed file's line count. [Issue #47](https://github.com/agentmemoir/clio/issues/47) [PR #141](https://github.com/agentmemoir/clio/pull/141)
- The admission gate-boundary tripwires scan the whole workspace but were tests in `clio-store`, so a scoped run skipped them for a change that did not reach that crate; they are now a `scripts/` check that runs on every change. [Issue #40](https://github.com/agentmemoir/clio/issues/40) [PR #140](https://github.com/agentmemoir/clio/pull/140)
- `clio compose up` and `make compose up` with the `local-rft` profile refuse to start the extract service while its GGUF model is missing, naming the file and the command that fetches it; the file they require is the one Compose itself resolves and mounts, so a `CLIO_EXTRACT_GGUF` override in the environment or in `.env` is followed rather than guessed at, and a `models/` directory Docker created itself no longer satisfies the check. [Issue #18](https://github.com/agentmemoir/clio/issues/18) [PR #139](https://github.com/agentmemoir/clio/pull/139)
- `clio recall` names the binding score floor when the floors empty the page and echoes the effective limit; an unknown `--bank` is refused on reads and on a summarize dry-run; and a bad `--domains` refusal names the flag. [Issue #16](https://github.com/agentmemoir/clio/issues/16) [PR #138](https://github.com/agentmemoir/clio/pull/138)
- Reject out-of-range persona observations, unverifiable source text, invalid inspect/export filters, missing retention profile files, and reads from unknown banks; reject unknown summarize preview scopes and source-text CLI flags without snapshot support. Warn when semantic or keyword floors remove all retrieval candidates, include the named day in date-only inspect end bounds, and explain process-local scratchpad lifetime and hygiene `keep` behavior. [Issue #14](https://github.com/agentmemoir/clio/issues/14) [PR #114](https://github.com/agentmemoir/clio/pull/114)
- Make CLI error codes and exit groups consistent for domain failures, missing items and snapshots, auth tokens, and ops reports; document signal-based shell statuses; unhealthy reports and incomplete restores now exit nonzero. [PR #63](https://github.com/agentmemoir/clio/pull/63)
- A `triple_add` whose `snapshot_ref` names an item the bank does not hold reports `item_not_found` instead of `invalid_argument`, and a caller that branches on the code no longer reads a missing target as a bad argument; the message and the exit code are unchanged. [PR #104](https://github.com/agentmemoir/clio/pull/104)
- Raise the declared minimum Rust version from 1.89 to 1.95 and replace both `fetch_update` calls with the `try_update` rename, so the manifest matches the compiler the code needs and the two deprecation suppressions go away; Clippy now also checks test targets for newer-than-minimum API use. [PR #115](https://github.com/agentmemoir/clio/pull/115)
- clio-bench records `tokens_retrieved` as the cost of the context it hands to the answer generator, so `--token-budget` now bounds that context instead of a total the adapters charged per retrieval hit; a pack that was over budget is now refused at the margin, and a run refuses to write a report when an adapter's reported cost is not the cost of its context. [PR #113](https://github.com/agentmemoir/clio/pull/113)
- clio-bench classifies a write by its refusal code instead of its `ok` flag alone, so a configuration failure such as a relative `--data-dir` stops the run with exit 2 and writes no report instead of being counted as an admission rejection. A declined turn, an exact duplicate, a turn the system's own write filter dropped, and a write the harness could not complete are counted and named apart, no probe is scored against a store that is missing a turn or that took none of them, and a suite with nothing scored reports no accuracy. [PR #117](https://github.com/agentmemoir/clio/pull/117)
- `clio remember`, `admit`, `triple add`, `canonical put`, and `shared store` refuse a `--source-type`/`--epistemic-kind` mismatch, and an unknown `--source-type`, as a usage error (exit 2) that names both flags, the valid values, and the rule; a `--confidence` outside `0.0..=1.0` is now a usage error too, and each affected `--help` states the pairing. [PR #116](https://github.com/agentmemoir/clio/pull/116)
- Gate logs distinguish fresh runs from cached replays, report Cargo package work when status lines are present, and publish to the per-worktree cache by default; dry runs bypass cache and publication even through direct gate calls, and cache keys include tool contents, the active Rustup toolchain, Cargo configuration, build environment values, and equivalent command spellings. [PR #135](https://github.com/agentmemoir/clio/pull/135)
- Every command's `--help` now prints its flags' descriptions and real defaults from the command registry, commands that refuse a global flag no longer list it as accepted, and refusal notes agree with the usage lines. ([#83](https://github.com/agentmemoir/clio/pull/83))

## [0.1.0] - 2026-10-10

### Added

- Prebuilt binaries for macOS on Apple silicon and Intel, and Linux on x86_64
  and arm64, installed with one command and no compiler, package manager, or
  admin rights. The installer checks the download against the checksum published
  with the release.

- Harness install and uninstall from the command line. `clio install --harness`
  registers Clio with a harness and `clio uninstall --harness` removes it. The
  supported harnesses are Claude, Cursor, and GitHub Copilot. Each command merges
  exactly one Clio entry into the harness's configuration file and leaves every
  other server and permission bit as it was, and each is safe to run twice.
  Uninstall removes only the Clio entry, and when its helper binary is missing it
  refuses and prints the exact removal command to run by hand. A configuration
  file that cannot be parsed safely is refused with a snippet to paste by hand.
  Unknown flags and a valueless `--harness` exit 2 before anything runs, and
  `--help` lists only harnesses that exist.

- Two retention deployment profiles, `benchmarks/profiles/long-lived-companion.json`
  and `benchmarks/profiles/coding-agent.json`, with a comparison report of one
  harness run per profile. The profiles choose settings and change no constant,
  default, or ranking value.

- A `supersession` section on the `inspect` tool. Called with an `item_id` and a
  `query`, it reads the item's correction lineage, runs the query through
  retrieval once, and reports whether the replacement precedes the superseded
  item, the superseded item precedes the replacement, or one of them is not on
  the page, naming which and whether the reason is absence or the page's cut. It
  reports both ids and both scores when both are on the page, and reports an item
  with no correction link as no link. Its retrieval runs with co-activation
  reinforcement off, so asking the question adds no write and changes nothing it
  examines.

- Two lists on the `stats` tool. `by_age_band` buckets active memories by age in
  half-life units, where the unit is the bank's retention half-life in days and
  each band's boundaries in days appear in its label, and `by_provenance_class`
  counts user_stated, agent_inferred, third_party, and fact. The tool also relays
  three maintenance pressure counts (flagged, stale summaries, archive
  candidates) from the same sources the `maintenance_status` pressure block
  reads, and leaves a count out when its source cannot answer rather than showing
  a zero. On this surface `tier` means exactly the semantic-versus-episodic split.

- Automatic maintenance waves. The sweeper checks each bank's maintenance
  pressure every five seconds and, when the dirty-ancestor count, the eligible-hub
  count, or the stale-unit count crosses its threshold (each default 1), fires one
  maintenance wave for that bank on its own thread, never inside a write. Three
  bounds apply: a cooldown between waves (default 300 seconds), an hourly cap
  (default 12 waves per bank), and a per-wave hub budget (default 16 hubs, which
  defers the rest to later waves). Every check records exactly one outcome, so a
  declined check names one reason: no pressure, cooldown, budget, contention, or
  the bank's retention profile having the trigger off. A declined check starts no
  work, and `maintenance_status` reports a `trigger` block with the pressure the
  decision read, whether the last check fired, the wave count over the last hour,
  the last wave's age, the next eligible time, and the decline reason.

- A `pressure` block on `maintenance_status`: one count per kind of structural
  work waiting on the bank, covering dirty and refreshing ancestors, eligible
  hubs, pending and retry index jobs, stale distilled units, and archive
  candidates, each with its source or an explicit unavailable reason. Reading
  pressure performs no writes and starts no maintenance. The stale count is as of
  the last finished wave plus stored unit state, and says so; the archive count
  comes from a bounded on-demand scan of the hygiene archive band (score 0.70 to
  below 0.90) and carries its incomplete flag.

- An `exact_render` field on a `retrieve` hit, a bounded rendering of that item's
  stored snapshot, requested with the optional `include_exact` toggle. The key is
  absent from every hit unless the call asks. The rendering is whole or nothing: a
  snapshot whose rendering would exceed 512 bytes yields no rendering rather than
  a shortened one. It is content, so it stays out of `warnings`, the `explain`
  trace, telemetry, and the health surfaces. It is a rendering and not the
  authoritative half, so read `get_snapshot` when the exact stored value matters.

- A `fidelity` channel on `compose_context`. `flow` (the default) packs the
  narrative channel with each hit's gist first, and `exact` prefers the hit's
  bounded rendering. A rendering is charged against the same `budget_tokens` as
  every other packed text, and a hit whose rendering does not fit is marked as a
  reference rather than replaced by its identifier, so the packed text and the
  per-form tally cannot disagree. `flow` is a narrative channel, so a number or
  identifier taken from it alone is not exact. Omitting the argument applies the
  bank's `pack_fidelity_default`, and a value outside the two names is rejected as
  `invalid_argument`.

- A form label on every hit in a `compose_context` pack, prose or reference, and
  a per-form tally with one count each for prose, exact, and reference. A
  reference line inside a pack section begins with the `[reference] ` marker, so
  an identifier in the packed text is visibly a pointer to a stored item rather
  than content. The marker is charged against the token budget, so each reference
  contribution costs three tokens more than the bare identifier it replaces.

- A `trust_factor` on every `retrieve` hit: the number the trust ratio multiplied
  the hit's score by at the shaping site, carried from that step so the reported
  value is exactly the applied one. A fact and a user-stated belief are `1.0`, a
  third-party belief is `0.95`, and an agent-inferred belief is `0.85` under the
  shipping defaults. The value is always present for a returned hit and never
  changes a hit's `category`, `epistemic_kind`, or `confidence`. A `trust` group
  in the ranking environment holds three knobs, `user_stated`, `third_party`, and
  `agent_inferred`, each a finite ratio above 0 and at most 1.

- An optional `provenance_floor` on `retrieve`, naming the weakest provenance a
  returned hit may sit at: `verified` (facts only), `stated`, `third_party`, or
  `inferred` (everything). A hit below the floor is removed from the answer set
  rather than ranked lower, and is never reinforced. The floor runs before the hit
  limit, the token budget, and duplicate suppression, so a floored page is not
  promised to be shorter. The `dense_candidates`, `lexical_candidates`, and
  `fused_candidates` counters read lower with a stricter floor. Omitting the
  argument applies the bank's `provenance_floor_default`, which `compose_context`
  also resolves for its own retrieval, and a value outside the four rungs is
  rejected as `invalid_argument`.

- An `age_factor` on every `retrieve` hit: the number the retention curve
  multiplied the hit's score by, carried from the shaping step. It is a plain
  number in the bank's `[retention_floor, 1]` range, exactly `1` when the curve is
  off, and never null for a returned hit. The `final` minimum score acts on that
  demoted value.

- Bearer-token auth for `clio mcp http` from a `--token-file` store, so the
  secret is not passed where any process on the machine can read it. A request
  presenting a stored secret acts as the actor its record names and reaches only
  the banks it lists, so the actor and bank a caller sends do not decide who it
  is: a declared actor that disagrees with the record is refused, and a bank
  outside the record's list is refused whether the caller named it or the default
  resolved to it. The store is loaded once at start, and a path that is absent,
  unreadable, malformed, readable by anyone but its owner, or holding no record
  refuses the start rather than serving a 401 for every request. A non-loopback
  bind needs a `--token-file` holding at least one record.

- `clio auth token add`, `list`, and `revoke`, which manage the network
  credentials the servers accept. Each token names a label, an actor, and the
  banks it may reach, and lives in one owner-only file,
  `<config>/clio/auth_tokens.json`, overridable with `CLIO_AUTH_TOKEN_FILE`. The
  secret is 32 CSPRNG bytes, printed exactly once by `add`, and never stored: the
  file holds only its SHA-256 digest, so a leaked file yields nothing usable.
  `revoke` takes effect at the next server start, because both servers load the
  store once at start. Every write holds an exclusive lock on `<store>.lock` for
  the whole read-modify-write, so two concurrent `add` runs cannot each overwrite
  the other's record. The three verbs bind no MCP tool, so an agent cannot mint,
  widen, or list credentials.

- A sync key lifecycle of `clio sync keygen`, `clio sync key export`, and `clio
  sync key status`. The key is created once and printed exactly once, so `key
  export` is the only way to read it again. `keygen --store file|keystore`
  chooses where it lives and records the choice, and on a node that already holds
  a key it rotates instead of refusing. `key status` reports the store, a
  16-character fingerprint two nodes can compare, the creation time, and the key
  count, and never the key or any prefix of it. The same report is available to
  an agent as the `sync_key_status` MCP tool; `keygen` and `export` stay on the
  command line because both print the sealing key. A file-backed node can point
  the other node at its key with `CLIO_SYNC_KEY_FILE`. A new key never inherits a
  previous key's creation time.

- Several sync keys held at once. Content is sealed with the newest key and
  opened with any key the node holds, so a key can be changed without stranding
  what the previous one sealed. `keygen` archives the current key and its
  creation time in an owner-only file beside the key file before it writes the
  new one, so an interrupted rotation loses nothing, and a key held in the OS
  secret store is copied to that file rather than dropped. `key status`, and the
  `encryption_summary` under the `encryption` field in `clio sync status`, list
  every held key with its store, fingerprint, creation time, and whether it is
  current, and never a key or any prefix of one. A row sealed under a key the node
  does not hold is dead-lettered with a reason naming the missing key and the keys
  held, and the pull stops at the last row it could read until the key is added or
  the row is skipped with `clio sync ack-skip`. A node with no keys configured
  means encryption off, not an error.

- A `clio setup` question about where to keep the sync key, asked only on a
  deployment that did not omit sync. The answer is recorded the same one-time way
  the encryption key's is, and the preview names the store rather than promising a
  key file, because `clio sync keygen` creates the key and not the wizard.

- A `clio sync serve` warning on stderr, in every mode including `--dev`, when a
  bind is not loopback. The warning names the five fields a relay on that
  interface reads to route, order, and authorize a row: bank scope, record kind,
  operation, transaction time, and device id. It then names the two values that
  cross in the clear in a form derived under a key the relay does not hold, the
  record identifier and the content digest, and names the residual, the
  record-kind mix and device-id stability. The bind proceeds, and only the content
  and the admission score stay sealed. The same enumeration, the key lifecycle,
  and the rotation procedure with its two peer-visible failure modes are in the
  help for `clio sync serve`, `clio sync keygen`, `clio sync key export`, `clio
  sync key status`, and `clio sync status`.

- One coverage rule: every Rust file a change creates or modifies must reach 80%
  of its lines, and nothing else about coverage is a requirement. The function
  half and every second rule are withdrawn, including the recorded per-file floors
  in `scripts/coverage_floors.json`, the added-lines gate, the aggregate baseline
  comparison, and `make coverage-guard`. No flag changes the number for one file
  or one run.

- A single naming rule for environment variables: every variable Clio owns starts
  with `CLIO_`, and only the prefixed spelling is read. There is no fallback, no
  alias, and no warning printed at startup. The database connection is
  `CLIO_DATABASE_URL`; the embedding, reranking, and extraction settings carry the
  prefix on their URL, model, provider, dimensions, and API key; the model-hub
  token is `CLIO_HF_TOKEN`; and the Compose and per-service knobs carry it too.
  `ENVIRONMENT.md` lists every name Clio owns and every name it only consumes,
  including the operating system names, the build and coverage toolchain names,
  the container-only names, and `NO_COLOR` and `CLICOLOR_FORCE`, which keep their
  unprefixed spelling.

- A benchmark harness named `clio-bench`, under `benchmarks/clio_bench`, whose
  commands print as `clio-bench` and whose environment names are `CLIO_BENCH_*`
  (`CLIO_BENCH_API_KEY`, `CLIO_BENCH_STORE`, `CLIO_BENCH_RECORD`,
  `CLIO_BENCH_MNEMOSYNE_BINARY`, `CLIO_BENCH_FAKE_STATE`,
  `CLIO_BENCH_FAKE_REFUSE`).

- A `--bank` selector on `consolidate` (MCP: `bank`), which names the target bank
  like every other verb's bank selector, and rejects an empty value like the rest.
  `consolidate --force` runs the full maintenance wave: it refreshes every dirty
  ancestor, then reconciles the bank's consolidated summaries.

- A retention curve that multiplies each candidate's shaped score, on by default.
  The multiplier is `floor + (1 - floor) * exp(-ln2 * age_days / half_life_days)`,
  where `age_days` counts days since the item's `created_at`, so an item one
  half-life old keeps `floor + (1 - floor) * 0.5` of its shaped score. Age can
  only lower a score, and the curve changes no category, epistemic kind, or
  confidence. A bank that wants no age response sets `retention_curve: false` in
  its retention profile. That profile gains five defaulted fields and its stored
  version stamp is 3: `retention_curve` (default `true`), `retention_floor`
  (default `0.25`), `pack_fidelity_default` (default `"flow"`),
  `provenance_floor_default` (default none), and `maintenance_trigger` (default
  `true`). A profile that omits them reads the defaults, and one carrying an
  unknown field is refused. A `retention` group in the ranking environment holds
  one knob, `half_life_days`, which defaults to 30 and must be finite and above 0.

- A retrieval weight group of `dense`, `lexical`, and `importance` that sums to
  1.0, so `importance` carries 1/3. A ranking patch that carries any other field
  is rejected with the unknown field named. The two search legs split their own
  blend evenly at 0.5 each, and relative-date retrieval resolves against an
  explicit anchor and fuses that list in by rank.

- A trust ratio applied to each shaped score before the sort, read from the item's
  provenance class. Under the shipping defaults a third-party belief keeps `0.95`
  of its shaped score and an agent-inferred belief keeps `0.85`, while facts and
  user-stated beliefs keep `1.0`. The multiplier changes no category, epistemic
  kind, or confidence, and hides or deletes no record. A bank that wants the
  unadjusted order sets all three `trust` knobs to `1.0`.

- A recency factor, used in both of its places, that means a half-life. At the
  default of 30 days the factor for a record 7 days old is 0.8507, at 30 days
  0.5000, at 90 days 0.1250, and at 365 days 0.000218. The hygiene staleness
  window keeps its historical name, `hygiene_stale_half_life_days`, and stays a
  linear ramp rather than a half-life.

- Commit hooks that run a check reading your change on every commit:
  `scripts/git-hooks/pre-commit` and `scripts/git-hooks/pre-merge-commit`,
  installed as symlinks under `.git/hooks/` once per machine. They run the
  commit-scoped checks over the staged files, which is the set that reads the
  change; checks that run a test suite stay behind `make precommit`, because a
  commit hook that costs twenty seconds is a hook that gets passed with
  `--no-verify`. A hook declares its scope with a `precommit-scope: manual` line,
  and a hook that says nothing is commit-scoped, so a new check cannot be
  accidentally left out of commits. `git cherry-pick` and `git revert` are not
  covered by either hook.

- `make precommit`, which runs every registered check before a commit. Checks are
  files in `hooks.d/`, and the runner also picks up hooks in `.clio-hooks/`, which
  is git-ignored, so a check kept outside this repository joins the run without
  this repository containing it. It fails if a registered hook cannot run, rather
  than reporting a pass it cannot support.

- Freshness reporting on a consolidated summary. `get` and `inspect` report a
  `freshness` block with the proof count, the time the summary was last rebuilt,
  whether it is out of date, and which sources supported each revision, and the
  consolidation report counts out-of-date summaries by reason. Once a summary is
  out of date, the raw items it summarizes come back in place of the stale
  summary, and a summary whose source was withdrawn through `discard` stops being
  eligible for the consolidated-only scope, so the freed search slots fill with
  sources that are still there. A consolidation wave records an out-of-date
  summary on the summary itself rather than only when it is read, and the next
  summary it rebuilds clears it.

### Fixed

- The rule for reading the database variable lived in five places that disagreed.
  Three read the prefixed name as a fallback after the unprefixed one, and two
  never read the prefixed name at all, so a shell that exported only the prefixed
  name pointed the command at one database and the shared-Postgres suites running
  beside it at another. One function owns the name and the way it is read, and
  all five call sites use it, including the two in test-only code. A `clio` asked
  for a Postgres backend with no address names the variable it looked for:
  `postgres backend requires store.database_url or CLIO_DATABASE_URL`.

- The environment overlay documentation named `AM_*` variables the code does not
  read, and the example environment file documented the unprefixed database
  variable and not the prefixed one. Both name the variables the code reads.

- An interrupted maintenance wave resumes instead of repeating work. Each hub is
  claimed before it is distilled and the claim is finished once its distilled
  unit lands, so a second wave finishes hubs left half-done and a hub that already
  finished costs no new work. Erasing a hub or its distilled unit removes the
  claim with it.

- A restore refused while another process holds the store names that process on
  more filesystems. The holder lookup reads the whole device number, so a device
  with a large major or minor number still names the holder rather than only the
  lock file.

- An empty `embed.provider` or `rerank.provider` is rejected with the allowed
  providers named (`tei|openai` for embed, `tei|cohere` for rerank), rather than
  silently keeping the default. Leaving the key unset still selects the built-in
  default.

- The arm64 Linux binary links. The compiler builds the bundled D-Bus with
  outline atomics on that architecture, which call a helper glibc provides and
  musl does not, so the musl build compiles that code without them.

### Security

- A deployment file refuses a plaintext secret at a sealed path no matter how the
  file is shaped. A file carrying an unsealed value where a sealed secret belongs
  (`credentials.embed_api_key`, `store.database_url`, and the other sealed paths)
  is refused whether the value is written flat or nested. The refusal names the
  path and points at re-sealing through the normal save path, and the secret never
  loads. Every read of a deployment file that fails to load warns with the file
  and the reason, and a deployment write refuses to replace a file that failed to
  load instead of rewriting it with only the keys it still holds.

- A `MCP-Session-Id` on a credential-bound `clio mcp http` server belongs to the
  credential that created it. A session id presented by a different credential is
  answered exactly like an unknown id, which discloses nothing about whether the
  id exists, and only the credential that created a session can end it with
  `DELETE`. Servers started with no credential, the loopback development default,
  are unchanged. The session identity is the record label, and the store refuses a
  duplicate label, so no record answers for another credential's sessions.

- The actor and bank a caller declares over MCP HTTP are checked against the
  credential that presented, instead of being trusted on their own. The `erase`
  and `export` authorization gates read an actor the credential fixed, so a caller
  cannot claim `harness` by sending `"actor": "harness"`. The local paths
  (`stdio`, the CLI) have no credential edge and are unchanged.

- The admission score does not reach a relay. It travels inside the sealed sync
  payload beside the record and is put back on the mutation before the conflict
  comparison, so a same-timestamp conflict still resolves by admission score on a
  current peer and a relay holding the sync journal learns no score.
