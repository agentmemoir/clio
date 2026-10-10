# Agent Memoir (product: Clio)
#
# Rust (binary: clio):
#   make build|compile | test | lint | fmt | check | run | install
#   make ritual | test-fast | coverage-fast   (THE GATES)
#   make scripts-check                        (the self-tests and suites in scripts/)
#   make coverage | coverage-html | coverage-lcov   (diagnostics)
#   make coverage-package PACKAGE=<crate>                          (one crate)
#
# Compose:
#   make compose up mac|linux
#   make compose down mac|linux
#   PROFILE=local-retrieval make compose up mac
#   PROFILE= make compose up linux          # Profile A: postgres only
#
# Default PROFILE=local-rft (Profile C).

.DEFAULT_GOAL := help

BIN    := clio
PROFILE ?= local-rft
# Package selection for `test` and `lint`. The gate wrapper (scripts/gate.py)
# passes PACKAGES="-p a -p b" for a scoped quick pass over the changed crates
# and their dependents; the default stays the full workspace. Coverage targets
# are never scoped.
PACKAGES ?= --workspace

# The one crate `coverage-package` measures. Singular, and a separate variable
# from the one above on purpose. This target refuses an empty value instead of
# defaulting, because a mistyped variable that fell back to the whole workspace
# would produce a report and no complaint.

# Faster linking when a modern linker is installed. mold first, then lld. This
# is a no-op when neither is present, so it never breaks a host that lacks them.
# Linux only: macOS already ships a fast ld64 and clang does not accept a bare
# absolute path in -fuse-ld there.
ifeq ($(shell uname -s),Linux)
  FAST_LD := $(shell command -v mold 2>/dev/null || command -v ld.lld 2>/dev/null)
  ifneq ($(FAST_LD),)
    export RUSTFLAGS := $(RUSTFLAGS) -C link-arg=-fuse-ld=$(FAST_LD)
  endif
endif

COMPOSE := docker compose -f compose.yml

# One toolchain for the whole build. `llvm-cov` must match the compiler that
# produced the coverage map, so cargo, rustc, rustdoc, and llvm-tools come from
# the same toolchain. On a host where a non-rustup rustc (e.g. Homebrew) is
# first on PATH but ships no llvm-tools, the Makefile would otherwise compile
# with that rustc and report with rustup's llvm-cov, which mis-reads the map
# (phantom uncovered lines, e.g. comments). Prefer the toolchain rustup resolves
# for this checkout — the exact version rust-toolchain.toml pins, which `stable`
# is not obliged to be — and fall back to the PATH toolchain only when that
# toolchain's llvm-tools are absent.
HOST := $(shell rustc -vV 2>/dev/null | awk '/^host:/{print $$2}')
SYSROOT := $(shell rustc --print sysroot 2>/dev/null)
RUSTUP_TC := $(patsubst %/bin/rustc,%,$(shell rustup which rustc 2>/dev/null))
RUSTUP_BIN := $(RUSTUP_TC)/bin
RUSTUP_LLVM := $(RUSTUP_TC)/lib/rustlib/$(HOST)/bin
ifeq ($(wildcard $(RUSTUP_LLVM)/llvm-cov),)
  LLVM_TOOLS := $(SYSROOT)/lib/rustlib/$(HOST)/bin
  CARGO ?= cargo
  RUSTC ?= rustc
  RUSTDOC ?= $(SYSROOT)/bin/rustdoc
else
  LLVM_TOOLS := $(RUSTUP_LLVM)
  CARGO ?= $(RUSTUP_BIN)/cargo
  RUSTC ?= $(RUSTUP_BIN)/rustc
  RUSTDOC ?= $(RUSTUP_BIN)/rustdoc
endif
export RUSTC
export RUSTDOC
export LLVM_COV ?= $(LLVM_TOOLS)/llvm-cov
export LLVM_PROFDATA ?= $(LLVM_TOOLS)/llvm-profdata

COV_DIR := target/coverage
COV_JSON := $(COV_DIR)/coverage.json
# --no-clean keeps the instrumented build in target/llvm-cov-target warm, so a
# repeated gate reuses unchanged crates instead of rebuilding the whole
# workspace every time. `cargo llvm-cov` cleans by default, which is what made
# each gate a full instrumented rebuild. `make coverage-clean` is the
# authoritative from-scratch run (use it after large refactors, or when per-file
# numbers look wrong).
#
# The runner is nextest, not `cargo test`, and this matches CI on purpose. Each
# test then gets its own process, which is what keeps the shared-Postgres suites
# from competing for one database inside a single multi-threaded test binary.
# Running the suite a second time under `cargo test` to collect coverage gave
# every flake two chances to fail the build, and that second pass had the weaker
# isolation of the two. Requires: cargo install cargo-llvm-cov nextest.
#
# `--failure-mode=all` governs the merge of profile buffers only: it fails the
# merge only when every buffer is unreadable, so one bad buffer loses its own
# coverage instead of ending the run after every test has passed. It does not
# make the run survive a failing test, and the name of the flag is what keeps
# suggesting that it does. `scripts/coverage_surface_check.py` refuses a report
# step that leaves it off, so it stays on both commands the targets below run.
#
# `--no-fail-fast` runs every test even after the first failure (profile `ci`
# sets `fail-fast = false` too, so this makes that guarantee hold on the command
# line as well). What stops a report on a red suite is not the tests: cargo-llvm-cov
# returns as soon as the test command fails, before it would write any report. So
# each report target below keeps the one-command run on the green path, and on
# failure runs `cargo llvm-cov report` alone to write the report from the buffers
# the run left, then exits with the run's status. A red suite ends in a report, a
# printed path, and a non-zero exit.
LLVM_COV_CMD := $(CARGO) llvm-cov nextest --workspace --locked --no-clean --profile ci --failure-mode=all --no-fail-fast

# Clear this run's profile data before collecting new profile data.
#
# The report step merges every `*.profraw` it finds in the instrumented target
# directory, and `--no-clean` — which is what keeps the warm instrumented build
# above — also disables the profile clean-up that a default run would do. So the
# files accumulated forever: 21,792 of them and 14G here, growing by the number
# of test processes on every run, with nothing ever deleting one (#92).
#
# `--profraw-only` is the point: it deletes the profile files and leaves the warm
# build alone. The plain `--workspace` clean used by `coverage-clean` throws the
# whole directory away, which is the opposite of what the incremental gate wants.
LLVM_COV_CLEAN_PROFRAW := $(CARGO) llvm-cov clean --profraw-only

# Hermetic tests: never read the host's durable deployment overlay (for example a
# running sidecar in ~/.config/clio/deployment.json). The path does not exist, so the
# effective overlay is empty. Override on the command line to test a real config.
HERMETIC_DEPLOYMENT_CONFIG := $(COV_DIR)/no-deployment-overlay.json

ifeq ($(filter mac,$(MAKECMDGOALS)),mac)
  ARCH_FILE := compose.arm64.yml
endif
ifeq ($(filter linux,$(MAKECMDGOALS)),linux)
  ARCH_FILE := compose.amd64.yml
endif

PROFILE_FLAG := $(if $(strip $(PROFILE)),--profile $(PROFILE),)

# Directory search for `clean`. The top-level `.git` and `private` are pruned
# by path, not by name: one is the repository's own metadata and the other is
# a local-only directory a developer keeps beside this checkout, and neither is
# this target's to delete. Pruning by path rather than by name matters because a
# crate directory can legitimately be called `private`, and a name-based prune
# would skip its stray profiles.
CLEAN_FIND := find . \( -path ./.git -o -path ./private \) -prune -o

.PHONY: help build compile test lint fmt check run install docs docs-open \
	sweep sweep-clean clean clean-models precommit precommit-list \
	ritual scripts-check scripts-check-gate \
	scripts-check-lint-scope scripts-check-toolchain-pin scripts-check-span \
	scripts-check-phase-timer \
	scripts-check-precommit scripts-check-coverage-surface \
	scripts-check-codeless scripts-check-coverage-policy \
	coverage cov coverage-fast test-fast coverage-clean coverage-html cov-html coverage-lcov cov-lcov coverage-open coverage-package \
	compose up down mac linux

help:
	@printf '%s\n' \
		'Rust:     make build|compile | test | lint | fmt | check | run | install' \
		'Docs:     make docs | docs-open' \
		'Checks:   make precommit | precommit-list  (run every registered pre-commit hook)' \
		'Scripts:  make scripts-check  (the self-tests and suites in scripts/)' \
		'Disk:     make sweep (report) | sweep-clean (reclaim) | clean (delete all build output) | clean-models' \
		'Gates:    make ritual (format+clippy only, no tests) | test-fast (tests the changed files'"'"' packages) | coverage-fast (80%+ lines per changed file)' \
		'Reports:  make coverage | coverage-clean | coverage-html | coverage-lcov | coverage-open | coverage-package PACKAGE=<crate>' \
		'Compose:  make compose up|down mac|linux  [PROFILE=...]' \
		'Install:  make install  (cargo install to ~/.cargo/bin)'

build compile:
	$(CARGO) build --locked --bin $(BIN)

# The overlay's whole effect is that the path is absent, so absence has to be
# established rather than assumed: nothing creates that file and nothing removed
# it, and once something wrote there the gate read that leftover and reported on
# it instead of on the tree (#103). Removing it before the suite runs makes every
# run start from the empty overlay the comment above claims.
#
# `origin` is `command line` when the operator named their own path, and that
# path is theirs — deleting it would destroy a real deployment config to keep a
# test hermetic. So the removal is skipped in that case. `environment` is also
# skipped: that is an inherited export, not an instruction about this run, and
# the shell may well be pointing at the operator's real config.
ifneq ($(filter command\ line environment,$(origin CLIO_DEPLOYMENT_CONFIG)),)
HERMETIC_OVERLAY_CLEAN := @:
else
HERMETIC_OVERLAY_CLEAN := rm -f $(HERMETIC_DEPLOYMENT_CONFIG)
endif

# `test` feeds `check`, and every coverage target runs the suite under llvm-cov;
# all get the hermetic deployment overlay above. `make test CLIO_DEPLOYMENT_CONFIG=...`
# wins over this target-specific value.
#
# This stays a variable assignment and gets no recipe of its own: make does not
# accept a recipe line after a target-specific variable, and a second rule for the
# same target would silently override the first recipe with only a warning. The
# removal is therefore the first line inside each target's own recipe, where
# `$(HERMETIC_OVERLAY_CLEAN)` appears once per recipe rather than once per target.
test coverage cov coverage-html cov-html coverage-lcov cov-lcov coverage-open: \
	export CLIO_DEPLOYMENT_CONFIG = $(HERMETIC_DEPLOYMENT_CONFIG)

# nextest, matching the coverage targets and CI. `cargo test` parallelises only
# within one test binary and runs the binaries serially, which measured ~10
# minutes for this suite against 78s here.
#
# No doctest pass. Compiling documentation for all 17 crates cost 7 of the 11
# minutes of a full `make check` and ran 0 tests: the workspace has two doc
# examples and both are marked `ignore`. Remove this line's cost by not asking
# rustdoc to build docs nobody checks.
# Every test run is instrumented. `phase_timer.py` timestamps each line of
# output as it arrives and attributes every second of wall clock to a named
# cause, so a run that takes longer than expected says where the time went
# instead of leaving a residual for someone to guess at. Set GATE_PHASE_TIMER=0
# to skip it for a one-off run.
test:
	$(HERMETIC_OVERLAY_CLEAN)
	$(if $(filter 0,$(GATE_PHASE_TIMER)),,python3 scripts/phase_timer.py --json "$(PHASE_TIMER_JSON)" --) \
	$(CARGO) nextest run --locked $(PACKAGES) --profile ci

lint:
	cargo clippy $(PACKAGES) --all-targets --all-features --locked -- -D warnings

fmt:
	$(CARGO) fmt --all

# lint last: clippy reuses a prior build's artifacts, but a build does not reuse
# clippy's, so linting first compiled the workspace twice.
check: fmt test lint

run:
	$(CARGO) run --locked --bin $(BIN)

install:
	$(CARGO) install --path crates/clio --locked

docs:
	$(CARGO) doc --workspace --locked --no-deps

docs-open: docs
	@open target/doc/clio/index.html 2>/dev/null || xdg-open target/doc/clio/index.html 2>/dev/null || \
		printf 'Open: %s\n' 'target/doc/clio/index.html'

# Cargo never deletes artifacts from previous builds, so `target/debug/deps`
# grows forever: every branch switch and every dependency change writes a new
# set of files and abandons the old ones. Cargo tracks the missing garbage
# collection in rust-lang/cargo#13136, #5026 and #6229, and reports the
# resulting slowdown as a bug in rust-lang/cargo#16665: a flat deps directory
# with a very large number of entries makes path lookup dominate, and builds
# got 5.7x slower over a few days of ordinary work.
#
# Measured here: the directory reached 777,918 files, and listing 42 test
# binaries then cost 12m27s. After deleting it, 7,839 files, the same edit cost
# 1m48s. Deleting took 303s and the rebuild 29s, so the cleanup is paid back
# within a single run.
#
# Count-based rather than calendar-based: counting is cheap, deleting is not, and
# a count only fires when there is something to reclaim. `make sweep` reports
# without deleting so a run can decide. `make sweep-clean` deletes.
#
# The whole `target/debug` tree is measured, not just `deps`, because the
# incremental cache grows faster: after the deps cleanup this tree held 13,253
# files in `deps` but 164,499 in `incremental`, plus 14,371 in `.fingerprint`.
#
# Override the threshold with `make sweep SWEEP_LIMIT=100000`, and the directory
# with `make sweep SWEEP_DIR=target/llvm-cov-target` (52,124 files).
SWEEP_DIR ?= target/debug
SWEEP_LIMIT ?= 50000

sweep:
	@count=$$(find $(SWEEP_DIR) -type f 2>/dev/null | wc -l | tr -d ' '); \
	if [ "$${count:-0}" -gt "$(SWEEP_LIMIT)" ]; then \
		printf 'sweep: %s has %s files, over the %s limit. Run `make sweep-clean` to reclaim it.\n' \
			'$(SWEEP_DIR)' "$${count}" '$(SWEEP_LIMIT)'; \
	else \
		printf 'sweep: %s has %s files, within the %s limit. Nothing to do.\n' \
			'$(SWEEP_DIR)' "$${count:-0}" '$(SWEEP_LIMIT)'; \
	fi

sweep-clean:
	@count=$$(find $(SWEEP_DIR) -type f 2>/dev/null | wc -l | tr -d ' '); \
	if [ "$${count:-0}" -le "$(SWEEP_LIMIT)" ]; then \
		printf 'sweep-clean: %s has %s files, within the %s limit. Keeping it.\n' \
			'$(SWEEP_DIR)' "$${count:-0}" '$(SWEEP_LIMIT)'; \
	else \
		printf 'sweep-clean: removing %s files from %s. The next build is a full rebuild.\n' \
			"$${count}" '$(SWEEP_DIR)'; \
		rm -rf $(SWEEP_DIR); \
		printf 'sweep-clean: done.\n'; \
	fi

# --- full clean --------------------------------------------------------------
#
# `sweep-clean` only reclaims `target/debug`, and only past a file-count
# threshold. Nothing deleted the rest of the build tree, so a checkout grew
# without bound: one measured working tree held 43 GB under `target/`
# (27 GB instrumented coverage build, 14 GB debug) and 40,689 `*.profraw`
# files. `clean` is the one supported way to get back to a fresh checkout.
#
# It deletes build output only. The next build is a full rebuild by design, so
# this is not the command to run before a timed or coverage measurement.
#
# Deliberately NOT deleted, because losing them is expensive or destroys data:
#   models/   large GGUF downloads; use `make clean-models`
#   backups/  real user data
#   *.db.lock / *.sqlite.lock  a running process holds them
#   logs and other machine-local state
#   private/  a local-only directory kept beside this checkout, not this repo's to delete
#
# The `*.profraw` sweep covers the whole project, not just `target/`, because
# a test can drop a `default_*.profraw` next to a crate and a leftover profile
# is then merged into the next coverage report and skews it.

clean:
	@printf 'clean: removing target/ (the next build is a full rebuild).\n'
	@rm -rf target
	@profraw=$$($(CLEAN_FIND) -name '*.profraw' -print 2>/dev/null | wc -l | tr -d ' '); \
	if [ "$${profraw:-0}" -gt 0 ]; then \
		$(CLEAN_FIND) -name '*.profraw' -print0 2>/dev/null \
			| xargs -0 rm -f 2>/dev/null || true; \
		printf 'clean: removed %s *.profraw files.\n' "$$profraw"; \
	fi
	@pycache=$$($(CLEAN_FIND) -type d -name '__pycache__' -print 2>/dev/null | wc -l | tr -d ' '); \
	if [ "$${pycache:-0}" -gt 0 ]; then \
		$(CLEAN_FIND) -type d -name '__pycache__' -print0 2>/dev/null \
			| xargs -0 rm -rf 2>/dev/null || true; \
		printf 'clean: removed %s __pycache__ directories.\n' "$$pycache"; \
	fi
	@$(CLEAN_FIND) -name '*.pyc' -print0 2>/dev/null \
		| xargs -0 rm -f 2>/dev/null || true
	@$(CLEAN_FIND) -name '.DS_Store' -print0 2>/dev/null \
		| xargs -0 rm -f 2>/dev/null || true
	@rm -rf coverage
	@printf 'clean: done. models/ and backups/ were left alone.\n'

# Models are large and slow to fetch, so they get their own target rather than
# riding along with `clean`.
clean-models:
	@if [ -d models ]; then \
		printf 'clean-models: removing models/. Re-download on next use.\n'; \
		rm -rf models; \
	else \
		printf 'clean-models: no models/ directory. Nothing to do.\n'; \
	fi

# --- pre-commit hooks ---
#
# Not run by `check` and not run by CI. `check` is the branch gate and takes
# minutes; this is the fast loop you run while working, and it deliberately
# stays out of the branch gate so a failing hook cannot block an unrelated fix.
#
# The hook set is data, not code in this file. scripts/precommit.py finds every
# hook in hooks.d/ (tracked), in .clio-hooks/ (git-ignored, machine-local), and
# in whatever CLIO_PRECOMMIT_HOOKS names, then runs them. Adding a check to this
# project means dropping a file in hooks.d/, not editing this target.
#
# `precommit` is deliberately NOT wrapped in scripts/gate.py: the gate replays a
# stored verdict for an unchanged tree, and a hook that goes green from a cache
# tells you nothing about the tree in front of you.
precommit:
	python3 scripts/precommit.py

precommit-list:
	python3 scripts/precommit.py --list

# --- coverage (requires: cargo install cargo-llvm-cov nextest; rustup component add llvm-tools-preview) ---

# clio-store Postgres parity (Compose :34310 / CI service). Override with env if needed.
export CLIO_DATABASE_URL ?= postgres://clio:clio@127.0.0.1:34310/clio

# Where `make test` writes its phase-timing report. It is a timestamped file
# rather than a fixed name so two runs on the same machine cannot overwrite each
# other's evidence, and it lives outside the repository because a timing measured
# on one machine is meaningless on another.
export PHASE_TIMER_JSON ?= $(CURDIR)/target/phase-timing-$(shell date +%Y%m%d-%H%M%S).json

# A whole-workspace coverage report, for looking at the workspace.
#
# This is a diagnostic, not the gate. It used to carry `--fail-under-lines 80`
# and `--fail-under-functions 80` and to run the per-file guard over every file in
# the report, and it was the requirement this repository dropped: a change to one
# leaf crate cannot be made faster by measuring 507 files, and it cannot be made
# to pass by writing tests for a file it never touched. `make coverage-fast` is
# the gate, and it is the only coverage requirement in this repository: every
# Rust file a change created or modified, at 80% lines. Run this when you want
# the whole picture, and remember that a low number on a file this change did not
# touch is reported here and judged by nothing.
#
# No --summary-only, even though nothing here judges the report by machine: the
# full JSON is what makes the per-file rows available to a reader, and that flag
# would strip them. The export is about 70 MB.
#
# On the tool's own `--fail-under-*` family: it does have a per-file line gate,
# `--fail-under-file-lines`, and it is deliberately unused. It gates every file
# the report holds, which is the workspace on this target, so a change would fail
# on a file it never touched. `scripts/scoped_runner.py` gates only the files a
# change touched, names each offender in its log, and fails closed when the report
# does not hold a changed file at all. That is the whole reason the judgement is
# written in Python, and it is a reason about scope and reporting, not about what
# the tool can do.
coverage cov:
	$(HERMETIC_OVERLAY_CLEAN)
	@command -v cargo-llvm-cov >/dev/null || { \
		echo 'error: cargo-llvm-cov not found; run: cargo install cargo-llvm-cov'; exit 1; }
	@mkdir -p $(COV_DIR)
	$(LLVM_COV_CLEAN_PROFRAW)
	@status=0; if $(LLVM_COV_CMD) --json --output-path $(COV_JSON); then \
		printf 'JSON report: %s\n' '$(COV_JSON)'; \
	else \
		status=$$?; \
		$(CARGO) llvm-cov report --failure-mode=all --json --output-path $(COV_JSON) && printf 'JSON report: %s\n' '$(COV_JSON)'; \
		exit $$status; \
	fi

# THE COVERAGE GATE, and the whole of it: every Rust file this change created or
# modified must reach 80% of its lines. CI runs this, and there is no second
# coverage check anywhere in the tree, no other metric, and no way to change the
# number for one file or one run. A file's function coverage is measured and
# printed here and never held to a floor.
#
#   make coverage-fast
#   make coverage-fast PLAN=1       # print the selection, run nothing
#   make coverage-fast BASE=<ref>   # measure against a ref other than master
#
# Scoped by package, because that is the only unit cargo-llvm-cov supports, and
# by dependency closure, because the tests that cover a changed crate live in the
# crates that depend on it. Measured on an idle machine, one changed file in a
# leaf crate, which selects that crate plus its four dependents and runs 2190 of
# 4326 tests:
#
#   first run after `make clean`   1m29s   (184 crates compiled from empty)
#   subsequent runs                0m38s   (nothing to recompile)
#
# What it does not establish is in the script's own docstring: it is not a
# workspace verdict, and a green run is "no problem in the files you touched".
COVERAGE_FAST_FLAGS := $(if $(PLAN),--plan )
coverage-fast:
	@python3 scripts/scoped_runner.py --self-test
	@python3 scripts/scoped_runner.py $(COVERAGE_FAST_FLAGS) --base $(or $(BASE),master)

# THE TEST GATE. The same selection without instrumentation, so it skips the two
# things that make coverage expensive: the instrumented rebuild and the
# profile-data merge. It returns nextest's own exit code, so a failing test fails
# the command, named by nextest.
#
#   make test-fast
#   make test-fast PLAN=1
#   make test-fast BASE=<ref>
#
# It shares the coverage gate's package closure, so the two can never disagree
# about what a change touches, and that closure is also what pulls in every crate
# that depends on the one you edited, which is where a signature change shows up
# as a compile error rather than a test failure. `make test` scopes the same way
# but stops at the changed crates; this one goes all the way down the closure.
#
# Measured on an idle machine, one changed file in a leaf crate:
#
#   first run after `make clean`   1m26s   (uninstrumented build from empty)
#   subsequent runs                0m34s
#
# Compare `make coverage-fast` on the same change: 1m29s cold, 0m38s warm. The
# two are close once a build exists, because what dominates is the test run
# rather than the instrumentation; test-fast wins on a cold tree and on a change
# that does not need a coverage verdict.
test-fast:
	@python3 scripts/scoped_runner.py --self-test
	@python3 scripts/scoped_runner.py $(if $(PLAN),--plan )--tests-only --base $(or $(BASE),master)

# The cheap gate: format check plus lint, and nothing else.
#
# The name is the warning. This target runs no test and judges no coverage, so
# a green run here says the code is formatted and compiles cleanly, and says
# nothing at all about whether the tests pass or whether the changed files
# reach their coverage floor. It was named `verify` until it stopped running
# the tests, and a command called `verify` that verifies neither is the same
# kind of lie as a document that says CI runs it. The history has one already:
# `make check` was fmt, clippy and test wearing one name, a stage swapped it
# for `make test-fast`, and the formatting and lint checks were dropped without
# anyone noticing until a pull request said so.
#
# The tests and the coverage judgement are the two scoped targets below, and CI
# runs both of them on every pull request. That is the gate. This target is what
# is cheap enough to run before every commit, which is why a pipeline stage
# runs it and leaves the rest to the runner.
#
# fmt is checked, not written: this target reports, it does not repair.
# clippy is workspace-wide and takes no PACKAGES, deliberately. A crate you did
# not touch can still fail to compile against a signature you changed, and
# --all-targets means the test code is compiled too, so a test file that does
# not build is caught here even though no test is run.
#
# --workspace is spelled out on the clippy line rather than arriving through a
# variable, so the scope is readable where clippy is run and a check can read the
# guarantee off this line instead of inferring it. Without it this target meant
# `default-members`, which was one crate: sixteen of the seventeen crates, and
# their test targets, were never compiled here and the target still reported
# success.
#
# The clippy line below spells a bare `cargo`, not $(CARGO). $(CARGO) exists so
# llvm-cov comes from the same toolchain as the compiler that produced the
# coverage map, and clippy needs no such match. A toolchain binary invoked by
# path does not read rust-toolchain.toml, so a clippy run through $(CARGO) would
# lint with the compiler this host carries under `stable` while the runner uses
# the pinned one.
#
#   make ritual
ritual:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# From scratch: drop the warm instrumented build, then run the diagnostic above.
coverage-clean:
	@command -v cargo-llvm-cov >/dev/null || { \
		echo 'error: cargo-llvm-cov not found; run: cargo install cargo-llvm-cov'; exit 1; }
	$(CARGO) llvm-cov clean --workspace
	$(MAKE) coverage

# Judge every file in an existing whole-workspace report against its floor. Kept
# as a diagnostic for when you want the debt list; it is not a gate, and nothing
# in CI runs it.
# Measure ONE crate and print its per-file rows. A diagnostic, not a gate.
#
#   make coverage-package PACKAGE=clio-types
#
# Why a target instead of the tool's own per-package command. That command builds
# into the shared instrumented target directory, and the report's object list is
# every executable in that directory whose name matches the workspace. A package
# selection does not narrow that walk, and the merge unions what it is given, so
# an instrumented binary an earlier run of another package left behind moves
# this crate's per-file rows: a line count larger than the file itself, and a
# warning that N functions have mismatched data. This target gives each package
# its own instrumented directory, so that walk cannot see another package's
# binary at all, and it still drops unreadable profile buffers and still prunes
# the object list afterwards. It writes a report of its own, so it can never
# overwrite the gate's.
#
# What it does NOT establish:
#
# - It judges nothing. No floor is compared, and no exit code here means "below
#   floor". A low number is a measurement to look at, not a failure and not a
#   pass. `make coverage-fast` is the gate; this is how you see one crate.
# - Its numbers are only as good as its object list. A directory of its own
#   removes the foreign binaries; it does not prove the remaining list is right,
#   and the pruning rule is name-based.
# - It measures one crate, so a file covered by a test living in another package
#   reads lower here than the gate reports it. Compare like with like: this row
#   and a gate row are two different measurements, not one number and its double.
#
# Cost: each crate gets its OWN cold instrumented build the first time it is
# measured, because the gate's warm build lives in a different directory and
# cannot be reused. No duration is stated here: this target was written without
# running it, so any figure would be invented. The shape is what is certain: a
# full instrumented compile of that crate and everything it depends on, then one
# merge. Reclaim one crate's with
# `make sweep-clean SWEEP_DIR=target/llvm-cov-package SWEEP_LIMIT=0` (the limit
# is 0 because `sweep-clean` otherwise keeps a tree under the threshold, and a
# single crate's build is usually under it), or take the whole of `target/` with
# `make clean`.
coverage-package:
	@if [ -z '$(strip $(PACKAGE))' ]; then \
		printf 'coverage-package: needs PACKAGE=<crate>, e.g. make coverage-package PACKAGE=clio-types\n' >&2; \
		exit 2; \
	fi
	@python3 scripts/scoped_runner.py --self-test
	@python3 scripts/scoped_runner.py --coverage-package $(PACKAGE)

# The self-tests and the suites that live under `scripts/`. These files decide
# what the gates report and whether they report it at all: `scripts/gate.py`
# answers a repeated heavy command with a stored verdict, and
# `scripts/scoped_runner.py` decides which packages a scoped gate measures, so a
# wrong answer in either lands on an unrelated check rather than on the thing
# that is wrong.
#
# One target per suite, and one CI step per suite, because `make` stops at the
# first failing command. A single target running all of them reports one failure
# even when three suites are broken, and the check list shows one status where
# there is one per suite. Three of them had no step in CI on master and nothing
# ran them there; the others were steps of their own, and kept their names.
#
# What each one is, taken from its own file. `gate.py` answers whether to run a
# heavy command and what verdict to store, so its suite is the one that matters
# most here. `lint_scope_check.py` reads the Makefile, the workflows and the root manifest,
# and refuses a clippy run that could select fewer crates than the file claims it
# selects. `toolchain_pin_check.py` refuses a workflow that installs a compiler
# other than the pin. The two are separate files because they answer unrelated
# questions and a reader debugging one should not have to read the other to find
# out which file to open. `span_verify.py` is not
# machinery: it is the reference span-grounding check that `extract_quality.py`
# imports, and running it directly executes the self-check in its `__main__`.
# `test_phase_timer.py` and `test_precommit.py` are suites for the phase timer
# and the pre-commit runner. `coverage_surface_check.py` reads the coverage
# commands in this Makefile, the workflows and the gate's own two command
# builders, and refuses a shape that could no longer measure one run and report it
# once: a per-package run nothing keeps inside its selection, one command doing
# both jobs at once, and a report step whose merge one malformed buffer can end.
#
# One script under `scripts/` still has no CI step, and the absence is
# deliberate. `owned_tests.py --accept` runs `cargo nextest list`, which builds
# the workspace's test binaries, so it does not fit a target of pure Python and
# wants a step of its own; it is wired into a manual pre-commit hook only.
#
#   make scripts-check                   all of them, stopping at the first failure
#   make scripts-check-precommit         one of them, named
scripts-check: scripts-check-gate \
	scripts-check-lint-scope scripts-check-toolchain-pin scripts-check-span \
	scripts-check-phase-timer \
	scripts-check-precommit scripts-check-coverage-surface \
	scripts-check-codeless scripts-check-coverage-policy

scripts-check-gate:
	python3 scripts/gate.py --self-test

scripts-check-lint-scope:
	python3 scripts/lint_scope_check.py --self-test
	python3 scripts/lint_scope_check.py

scripts-check-toolchain-pin:
	python3 scripts/toolchain_pin_check.py --self-test
	python3 scripts/toolchain_pin_check.py

scripts-check-span:
	python3 scripts/span_verify.py

scripts-check-phase-timer:
	python3 scripts/test_phase_timer.py

scripts-check-precommit:
	python3 scripts/test_precommit.py

# Whether the coverage surface can still measure one run and report it once. The
# run is split into a test step and a report step so the gate owns a boundary in
# which to drop a profile buffer it cannot read and an instrumented binary this
# run did not build, and the report step names a merge failure mode so one bad
# buffer costs its own coverage instead of the whole verdict. All four protections
# are invisible to a passing run, which is why nothing else in the tree would
# notice their removal. Runs no build and no coverage command.
scripts-check-coverage-surface:
	python3 scripts/coverage_surface_check.py --self-test
	python3 scripts/coverage_surface_check.py

# Whether every file the coverage gate skips for holding no executable code still
# holds none. That list is the one place a file leaves the coverage rule without
# a floor, so an entry that has gone stale is a file silently outside the gate, and
# the gate itself cannot notice: it names a skipped file and moves on. Fourteen
# crate roots and one re-exporting module are on the list today. Runs no build and
# no coverage command.
scripts-check-codeless:
	python3 scripts/coverage_codeless_check.py --self-test
	python3 scripts/coverage_codeless_check.py

# Whether the tree still holds exactly one coverage rule. The four rules that were
# not this one all arrived in changes about something else, and nothing stopped
# them, so this states the rule as shapes that must not come back: a threshold
# flag handed to the coverage tool, a retired mechanism by name, a second
# threshold constant, a second place a line percentage is compared, and prose that
# reads as a live second criterion. It is a lint over a closed list, not a proof,
# and its own header says where it cannot see. Runs no build and no coverage
# command.
scripts-check-coverage-policy:
	python3 scripts/coverage_policy_check.py --self-test
	python3 scripts/coverage_policy_check.py

coverage-html cov-html:
	$(HERMETIC_OVERLAY_CLEAN)
	@command -v cargo-llvm-cov >/dev/null || { \
		echo 'error: cargo-llvm-cov not found; run: cargo install cargo-llvm-cov'; exit 1; }
	# llvm-cov appends its own `html/` under --output-dir.
	$(LLVM_COV_CLEAN_PROFRAW)
	@status=0; if $(LLVM_COV_CMD) --html --output-dir $(COV_DIR); then \
		printf 'HTML report: %s\n' '$(COV_DIR)/html/index.html'; \
	else \
		status=$$?; \
		$(CARGO) llvm-cov report --failure-mode=all --html --output-dir $(COV_DIR) && printf 'HTML report: %s\n' '$(COV_DIR)/html/index.html'; \
		exit $$status; \
	fi

coverage-lcov cov-lcov:
	$(HERMETIC_OVERLAY_CLEAN)
	@command -v cargo-llvm-cov >/dev/null || { \
		echo 'error: cargo-llvm-cov not found; run: cargo install cargo-llvm-cov'; exit 1; }
	@mkdir -p $(COV_DIR)
	$(LLVM_COV_CLEAN_PROFRAW)
	@status=0; if $(LLVM_COV_CMD) --lcov --output-path $(COV_DIR)/lcov.info; then \
		printf 'LCOV: %s\n' '$(COV_DIR)/lcov.info'; \
	else \
		status=$$?; \
		$(CARGO) llvm-cov report --failure-mode=all --lcov --output-path $(COV_DIR)/lcov.info && printf 'LCOV: %s\n' '$(COV_DIR)/lcov.info'; \
		exit $$status; \
	fi

coverage-open: coverage-html
	@open $(COV_DIR)/html/index.html 2>/dev/null || xdg-open $(COV_DIR)/html/index.html 2>/dev/null || \
		printf 'Open: %s\n' '$(COV_DIR)/html/index.html'

# Stubs so `make compose up mac` is valid Make syntax (each word is a goal).
compose up down:
	@:

mac linux:
ifndef ARCH_FILE
	$(error Use: make compose up|down mac|linux)
endif
ifeq ($(filter down,$(MAKECMDGOALS)),down)
	$(COMPOSE) -f $(ARCH_FILE) $(PROFILE_FLAG) down
else
	$(COMPOSE) -f $(ARCH_FILE) $(PROFILE_FLAG) up -d
endif
