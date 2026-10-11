# Agent Memoir (product: Clio)
#
# Rust (binary: clio):
#   make build|compile | test | lint | fmt | check | run | install
#   make ritual | test-fast | coverage-fast   (THE GATES)
#   make quick BASE=origin/master             (the fast local check, mk/quick.mk)
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
	ritual scripts-check scripts-check-gate scripts-check-scoped-runner \
	scripts-check-lint-scope scripts-check-toolchain-pin scripts-check-span \
	scripts-check-phase-timer \
	scripts-check-precommit scripts-check-coverage-surface \
	scripts-check-coverage-policy \
	coverage cov coverage-fast test-fast gate-boundary coverage-clean coverage-html cov-html coverage-lcov cov-lcov coverage-open coverage-package \
	compose up down mac linux

help:
	@printf '%s\n' \
		'Rust:     make build|compile | test | lint | fmt | check | run | install' \
		'Docs:     make docs | docs-open' \
		'Checks:   make precommit | precommit-list  (run every registered pre-commit hook)' \
		'Scripts:  make scripts-check  (the self-tests and suites in scripts/)' \
		'Disk:     make sweep (report) | sweep-clean (reclaim) | clean (delete all build output) | clean-models' \
		'Quick:    make quick BASE=origin/master (tests of the changed modules, stops at the first failure)' \
		'Gates:    make ritual (format+clippy only, no tests) | test-fast (tests the changed files'"'"' packages) | coverage-fast (80%+ lines per changed file)' \
		'Reports:  make coverage | coverage-clean | coverage-html | coverage-lcov | coverage-open | coverage-package PACKAGE=<crate>' \
		'Compose:  make compose up|down mac|linux  [PROFILE=...]' \
		'Install:  make install  (cargo install to ~/.cargo/bin)'

include mk/quick.mk
include mk/coverage.mk
include mk/scripts.mk

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
	cargo clippy $(PACKAGES) --all-targets --all-features --locked

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
# `make precommit` runs the commit-scoped hooks over every tracked file. The hooks
# marked `precommit-scope: manual` are left out of it, because each of them runs
# in CI as a step of its own. `python3 scripts/precommit.py` runs every hook,
# manual ones included, and `make precommit-list` shows which hooks `make
# precommit` runs and which it leaves out.
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
	python3 scripts/precommit.py --commit --all-files

precommit-list:
	python3 scripts/precommit.py --commit --list

# --- coverage (requires: cargo install cargo-llvm-cov nextest; rustup component add llvm-tools-preview) ---

# clio-store Postgres parity (Compose :34310 / CI service). Override with env if needed.
export CLIO_DATABASE_URL ?= postgres://clio:clio@127.0.0.1:34310/clio

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
