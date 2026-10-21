# Contributing

## Changelog

Never edit [CHANGELOG.md](CHANGELOG.md) by hand on a normal pull request. Add one fragment per user-facing change under `changelog.d/`, named `<slug>.<category>.md`, for example `add-user-authentication.feature.md`. The slug is lowercase words joined by hyphens, at least 8 characters, and it ends with `-issues-<numbers>` when the change has issues, for example `fix-session-expiration-issues-45-46.bugfix.md`. The name carries no pull request number: the number does not exist yet when the file is written, and the name must already be unique inside `changelog.d/`. One fragment holds one logical change, written as a concise, user-facing description.

The category is `feature`, `bugfix`, or `improvement`, which the release build renders as `Added`, `Fixed`, or `Changed` under [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). A security fix, a deprecation, or a removal still starts as a fragment with the closest category and a plain name; the release manager places it under the right heading when building the release. The build leaves a type out when it has nothing in it. That specification asks for empty sections to be left out rather than shown as empty.

The specification says nothing about linking, so this is our choice: the release build appends `[Issue #N]` links from the `-issues-` marker in the filename, so a reader can trace the change to its discussion. A pull request link is no longer required on an entry, because the number does not exist when the fragment is written.

Check the fragments before you push:

```
python3 scripts/changelog_fragments.py --check --base origin/master
```

That validates every name and every paragraph, and it fails a pull request that changes production code outside tests and fixtures with no fragment added. A production-code change with no user-visible effect carries exactly `No user-facing change.`, which the check accepts and the release build leaves out. CI runs the same check with the pull request's own diff base.

### Writing the entry

Write for a person deciding whether to upgrade. Lead with what they will notice, not with what moved between files. A refactor, a test, or a rename is not an entry unless it changes what a user sees.

An entry is as long as the change deserves. Most are one sentence. A change that breaks someone's setup gets more: what broke, what they must do, and whether there is a way to keep the old behaviour. A security fix gets a paragraph saying what an attacker could do.

That specification is explicit that a changelog mentioning only some changes is as bad as no changelog, because a reader will treat it as the whole truth. So write down the change even when it is easy to describe wrongly.

### Commit and pull request titles

Prefix the title with `feat:`, `fix:`, `chore:`, or `docs:`, following [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/). Most titles on this repository already do. The prefixes keep the history readable. The changelog is written by hand and is not generated from them.

### The gate

CI is the gate. It runs on a pull request that touches a path in the filters of `.github/workflows/ci.yml`, and that file is the list of what it runs: the format check, clippy, the script suites, the scoped coverage judgement (the only scoped test run; `make test-fast` is a developer target), and the MCP conformance job. Nothing about a change is verified until CI says so.

What you run before you commit is the cheap half:

```
make ritual              # fmt check and clippy — no tests, no coverage
```

The name is the warning. A green `make ritual` says the code is formatted and compiles cleanly, including its test code. It says nothing about whether the tests pass or whether the files you changed reach their coverage floor.

Those two are separate commands, scoped to what you touched:

```
make test-fast           # the tests of the crate you touched plus its dependents
make coverage-fast       # measures that same set, judges the changed files
```

`make coverage-fast` is the whole of the coverage requirement: every Rust file your change created or modified must reach 80% of its lines. There is no second coverage check, no other metric, and no per-file number you can record to hold a file below 80%.

`make coverage-fast` runs those same tests a second time under instrumentation, so running all three repeats the suite. Run `make ritual` every time. Run the other two when you want a local answer before you push, and let CI be the verdict when you do not.

Through the gate shim, a `make coverage-fast` run with no terminal attached, such as a program's run, exits 3 and runs nothing when this worktree already made three fresh runs on this branch, when `python3 scripts/quick.py --base origin/master --plan --closure` shows it will not fit in 180 s, or when that estimate cannot be made. CI gives the verdict then.

There is also a scoped lint, and it is cheaper than `make ritual`:

```
make lint                # clippy over the changed crates and their dependents when the gate shim wraps make (scripts/gate-install.sh); the whole workspace without it
```

A change to one file in `clio-ops` selects 3 of the 17 packages against 17 for `make ritual`'s workspace-wide pass. Use it while iterating. Use `make ritual` before you push, because CI's clippy is workspace-wide and a scoped local lint is a weaker approximation of it.

The tests and the coverage step are both scoped to the files you changed. Format and clippy are not scoped, because a crate you did not touch can still fail to compile against a signature you changed.

Prose stays one paragraph per line: `make scripts-check-prose` fails on hard-wrapped paragraphs outside fenced code blocks, HTML comments, and YAML frontmatter.

### Postgres tests

A Postgres-backed test fails closed when the database cannot be opened: without an opt-out an unreachable store panics the test, so a green suite proves the Postgres path actually ran. Set `CLIO_SKIP_PG_TESTS` to `1`, `true`, or `yes` to turn that into a printed skip note and an early return instead. Unset, or any other value, keeps the strict default. The opt-out is read once, so a test that asserts on the skip branch and the gate that acts on it share the same decision.

### Pre-commit checks

Run them before you commit:

```
make precommit          # run the commit-scoped hooks over every tracked file
make precommit-list     # show which hooks that runs, and which it leaves out
```

`precommit` is separate from the gate and CI does not run it. It builds nothing, so it is quick; the gate is the expensive half. It leaves out the `manual` hooks (see below), which run in CI as steps of their own. `python3 scripts/precommit.py` runs every hook, manual ones included.

### The commit hook

A hook can also run on every commit or push, so a check that reads your change cannot be skipped by forgetting to ask for it. Three files: git does not use one hook for every commit, and a push runs its own hook:

```
ln -s ../../scripts/git-hooks/pre-commit .git/hooks/pre-commit
ln -s ../../scripts/git-hooks/pre-merge-commit .git/hooks/pre-merge-commit
ln -s ../../scripts/git-hooks/pre-push .git/hooks/pre-push
```

The push hook checks every tracked file in the pushed commit with the commit-scoped checks. It uses a detached temporary worktree, so local edits cannot hide a finding. A push with only deletions needs no scan. New branches use the merge-base with `origin/master`. The hook removes the worktree after a pass or a failure. Machine-local executable checks must live on storage that permits execution. Keep these checks fast; the push check has a five-minute budget.

The second is not a duplicate. `git merge` does not consult `pre-commit` when a merge completes on its own: it invokes `pre-merge-commit` instead, and the only thing that would connect the two is a sample template git ships commented out. Without the second link, a merge commits with no check at all. A merge that needs conflict resolution is the other way round: git does not run `pre-merge-commit` there, and runs `pre-commit` when you commit the resolved result. Between the two, every merge is checked once.

`make precommit` and the commit hooks run the same checks. The difference is the files: the commit hooks read the staged files, and `make precommit` reads every tracked file. Both take only the commit-scoped checks, because a commit hook cannot afford to be slow: a check that runs a test suite is not bounded by the size of a commit, and a hook that costs twenty seconds is a hook that gets passed with `--no-verify`. The manual checks still run when you ask for them by name, and in CI.

What the hooks cover, precisely: `git commit`, `git commit -a`, `git commit --amend`, `git commit --allow-empty`, an automatic `git merge`, and a merge you resolved and committed. What they do not: `git cherry-pick` and `git revert` create commits without consulting either hook. Run `make precommit` after either.

The links are symlinks on purpose. `core.hooksPath` would be the alternative, and it is the wrong one here: it moves where git looks for hooks and stops it looking in `.git/hooks`, so every other hook you or a tool had put there stops running without a word. A link adds a gate and disables nothing.

Three things these hooks will not tell you. A green run means the checks that ran found nothing, not that the tree is free of every kind of reference a reviewer might object to; the checks are a floor, not a proof. On a machine with no commit-scoped check registered they say so and let the commit through, because there is no gate there to enforce and refusing every commit would only teach people to pass `--no-verify`. And a hook that is registered but cannot run still fails the commit.

### Adding a check

Drop a file in `hooks.d/`. It does not need to be executable and it does not need a shebang: a `.py` or `.sh` file is run through the interpreter, and anything else has to carry the execute bit. Name it with a leading number so the order is stable (`30-my-check.py` runs after `20-...`). A hook exits 0 to pass and non-zero to fail. That is the whole contract.

A hook is given the list of tracked text files as arguments, and two environment variables:

| Variable | Value |
|----------|-------|
| `CLIO_PRECOMMIT_ROOT` | absolute path of the repository root |
| `CLIO_PRECOMMIT_MODE` | `all`, or `staged` under `--staged` and under `--commit` without `--all-files` |

A hook that does not care about files can ignore the arguments. One that needs the files should read them from the argument list rather than walking the tree, so it stays fast and so it cannot drift into reading a build artifact.

A hook is commit-scoped unless it says otherwise. To keep one off the commit hook, put this line in the hook:

```python
# precommit-scope: manual
```

The default is the scope that cannot be lost: a check nobody has classified yet runs on every commit, so the worst case is a slow commit rather than a check that quietly stopped running. A value the runner does not recognise is an error rather than a guess, because a misspelling would otherwise pick the other scope without saying so. Say which scope a hook is in when it is not the default. The hooks in `hooks.d/` that run a Python suite and read nothing about the change are `manual`: run `make precommit-list` for the current set and its scopes rather than counting them here, because the set changes and this sentence would go stale without anyone noticing.

Skip a slow one while iterating:

```
python3 scripts/precommit.py --skip 20-phase-timer-tests
```

### Checks kept outside this repository

A check does not have to live here. `scripts/precommit.py` also runs every hook in `.clio-hooks/` at the repository root, which `.gitignore` excludes, so a hook registered there is never staged, never pushed, and never appears in a diff. Point `CLIO_PRECOMMIT_HOOKS` at a directory or a file to add more.

Whatever a check matches is the whole of what it matches. A pattern list is written by someone thinking about the case in front of them, so a shape nobody thought of is not caught, and no amount of running the check changes that. Read what a check looks for before treating its silence as a clean bill: a check that greps for four identifier shapes says nothing about a reference written as a section number, and nothing about prose copied from a design record. If a check misses something, that is a bug in the check worth reporting rather than a judgement call you are expected to make at the commit.

This is how a check that only some maintainers run joins the run without this repository containing it. It is a normal extension point, not a secret: the runner lists every hook it found, including ones from outside, so you can always see what is about to run.

If a hook is registered but cannot run, the run **fails** rather than passing. A gate that has quietly stopped running looks exactly like a gate that is passing, so the runner refuses to report a verdict it cannot support.

### Cutting a release

1. Run `python3 scripts/changelog_fragments.py --build --version X.Y.Z`, which groups the fragments under their headings, writes one `## [X.Y.Z] - YYYY-MM-DD` section dated today below the empty `[Unreleased]`, and deletes the consumed fragments as part of the change. It also names any fragments that may belong under Security, Deprecated, or Removed, which you place by hand. Pass `--date` to set another date.
2. Move any leftover `[Unreleased]` hand entries under the new heading when the release still carries entries written before fragments existed.
3. Bump `version` in `[workspace.package]` in `Cargo.toml`. Every crate inherits it, so that one line covers all of them.
4. Commit, open a pull request, and merge.
5. Tag the merge commit `vX.Y.Z` and push the tag.

The release workflow builds the four platform binaries and publishes the matching changelog section as the release notes. It refuses to publish when the tag disagrees with the version in `Cargo.toml`, when a build fails, when any of the four binaries is missing, or when the changelog has nothing under that version. A partial release, or one with no notes, is never published.

Release notes are the hand-written section rather than a generated list of commit titles, for the reason that specification gives: a generated changelog is mostly noise, and a commit log is not a changelog.

### After a release fails to build

A tag whose build failed stays where it is. Do not move it and do not delete it. Bump the version and release again.

Record the abandoned version with a normal heading and a plain sentence saying it published nothing, so the next reader knows there is nothing to install from that tag. Keep a Changelog uses `[YANKED]` for a version that was published and then had to be pulled for a serious bug or a security problem. That is a different situation, so use that word only for it.
