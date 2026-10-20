# Where `make test` writes its phase-timing report. It is a timestamped file rather than a fixed name so two runs on the same machine cannot overwrite each other's evidence, and it lives outside the repository because a timing measured on one machine is meaningless on another.
export PHASE_TIMER_JSON ?= $(CURDIR)/target/phase-timing-$(shell date +%Y%m%d-%H%M%S).json

# A whole-workspace coverage report, for looking at the workspace.
#
# This is a diagnostic, not the gate. It used to carry `--fail-under-lines 80` and `--fail-under-functions 80` and to run the per-file guard over every file in the report, and it was the requirement this repository dropped: a change to one leaf crate cannot be made faster by measuring 507 files, and it cannot be made to pass by writing tests for a file it never touched. `make coverage-fast` is the gate, and it is the only coverage requirement in this repository: every Rust file a change created or modified, at 80% lines. Run this when you want the whole picture, and remember that a low number on a file this change did not touch is reported here and judged by nothing.
#
# No --summary-only, even though nothing here judges the report by machine: the full JSON is what makes the per-file rows available to a reader, and that flag would strip them. The export is about 70 MB.
#
# On the tool's own `--fail-under-*` family: it does have a per-file line gate, `--fail-under-file-lines`, and this target leaves it off by default. It gates every file the report holds, which is the workspace on this target, so a change would fail on a file it never touched. Only the scheduled whole-workspace workflow turns it on, through COV_TOOL_ARGS, because judging every file is that workflow's purpose. `scripts/scoped_runner.py` gates only the files a change touched, names each offender in its log, and fails closed when the report does not hold a changed file at all. That is the whole reason the judgement is written in Python, and it is a reason about scope and reporting, not about what the tool can do.
# Extra arguments for the one run `coverage` makes, given as `make coverage COV_TOOL_ARGS=...`.
# Empty unless a caller passes some. The scheduled whole-workspace workflow passes its per-file line gate here.
COV_TOOL_ARGS ?=

coverage cov:
	$(HERMETIC_OVERLAY_CLEAN)
	@command -v cargo-llvm-cov >/dev/null || { \
		echo 'error: cargo-llvm-cov not found; run: cargo install cargo-llvm-cov'; exit 1; }
	@mkdir -p $(COV_DIR)
	$(LLVM_COV_CLEAN_PROFRAW)
	@status=0; if $(LLVM_COV_CMD) $(COV_TOOL_ARGS) --json --output-path $(COV_JSON); then \
		printf 'JSON report: %s\n' '$(COV_JSON)'; \
	else \
		status=$$?; \
		$(CARGO) llvm-cov report --failure-mode=all --json --output-path $(COV_JSON) && printf 'JSON report: %s\n' '$(COV_JSON)'; \
		exit $$status; \
	fi

# THE COVERAGE GATE, and the whole of it: every Rust file this change created or modified must reach 80% of its lines. CI runs this, and there is no second coverage check anywhere in the tree, no other metric, and no way to change the number for one file or one run. A file's function coverage is measured and printed here and never held to a floor.
#
#  make coverage-fast
#  make coverage-fast PLAN=1       # print the selection, run nothing
#  make coverage-fast BASE=<ref>   # measure against a ref other than master
#
# Through the gate shim, a run with no terminal attached exits 3 and runs nothing when this worktree already ran it on this branch, or when `python3 scripts/quick.py --base <ref> --plan --closure` shows it will not fit in 180 s or cannot be made. CI runs this target without the shim.
#
# It selects what `make quick` selects: the test modules each changed Rust file declares, and the integration target of a changed file under `tests/`. All of them run together in one instrumented nextest run over only the packages that hold them. A changed file fails when it owns no test module of its own, is below 80% lines, or is not in the report. Files under `tests/`, declared test modules, `build.rs`, and files that hold only module declarations and re-exports are not judged. Function and region percentages are printed and compared against nothing.
COVERAGE_FAST_FLAGS := $(if $(PLAN),--plan )
coverage-fast:
	@python3 scripts/scoped_runner.py --self-test
	@python3 scripts/scoped_runner.py $(COVERAGE_FAST_FLAGS) --base $(or $(BASE),master)

# THE TEST GATE. The same selection without instrumentation, so it skips the two things that make coverage expensive: the instrumented rebuild and the profile-data merge. It returns nextest's own exit code, so a failing test fails the command, named by nextest.
#
#  make test-fast
#  make test-fast PLAN=1
#  make test-fast BASE=<ref>
#
# The same selection as `make coverage-fast`, run once without instrumentation. It judges nothing: its exit code is the test run's exit code.
#
# Compare `make coverage-fast` on the same change: the two are close once a build exists, because what dominates is the test run rather than the instrumentation; test-fast wins on a cold tree and on a change that does not need a coverage verdict. Measured times are in the CI timing record, not in this file.
test-fast:
	@python3 scripts/scoped_runner.py --self-test
	@python3 scripts/scoped_runner.py $(if $(PLAN),--plan )--tests-only --base $(or $(BASE),master)

# The cheap gate: format check plus lint, and nothing else.
#
# The name is the warning. This target runs no test and judges no coverage, so a green run here says the code is formatted and compiles cleanly, and says nothing at all about whether the tests pass or whether the changed files reach their coverage floor. It was named `verify` until it stopped running the tests, and a command called `verify` that verifies neither is the same kind of lie as a document that says CI runs it. The history has one already: `make check` was fmt, clippy and test wearing one name, a stage swapped it for `make test-fast`, and the formatting and lint checks were dropped without anyone noticing until a pull request said so.
#
# The tests and the coverage judgement are the two scoped targets below, and CI runs both of them on every pull request that touches a path in its path filters. That is the gate. This target is what is cheap enough to run before every commit, which is why a pipeline stage runs it and leaves the rest to the runner.
#
# fmt is checked, not written: this target reports, it does not repair. clippy is workspace-wide and takes no PACKAGES, deliberately. A crate you did not touch can still fail to compile against a signature you changed, and --all-targets means the test code is compiled too, so a test file that does not build is caught here even though no test is run.
#
# --workspace is spelled out on the clippy line rather than arriving through a variable, so the scope is readable where clippy is run and a check can read the guarantee off this line instead of inferring it.
#
# The clippy line below spells a bare `cargo`, not $(CARGO). $(CARGO) exists so llvm-cov comes from the same toolchain as the compiler that produced the coverage map, and clippy needs no such match. A toolchain binary invoked by path does not read rust-toolchain.toml, so a clippy run through $(CARGO) would lint with the compiler this host carries under `stable` while the runner uses the pinned one.
#
#  make ritual
ritual:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --all-features --locked

# From scratch: drop the warm instrumented build, then run the diagnostic above.
coverage-clean:
	@command -v cargo-llvm-cov >/dev/null || { \
		echo 'error: cargo-llvm-cov not found; run: cargo install cargo-llvm-cov'; exit 1; }
	$(CARGO) llvm-cov clean --workspace
	$(MAKE) coverage

# Judge every file in an existing whole-workspace report against its floor. Kept as a diagnostic for when you want the debt list; it is not a gate, and nothing in CI runs it. Measure ONE crate and print its per-file rows. A diagnostic, not a gate.
#
#  make coverage-package PACKAGE=clio-types
#
# Why a target instead of the tool's own per-package command. That command builds into the shared instrumented target directory, and the report's object list is every executable in that directory whose name matches the workspace. A package selection does not narrow that walk, and the merge unions what it is given, so an instrumented binary an earlier run of another package left behind moves this crate's per-file rows: a line count larger than the file itself, and a warning that N functions have mismatched data. This target gives each package its own instrumented directory, so that walk cannot see another package's binary at all, and it still drops unreadable profile buffers and still prunes the object list afterwards. It writes a report of its own, so it can never overwrite the gate's.
#
# What it does NOT establish:
#
# - It judges nothing. No floor is compared, and no exit code here means "below floor". A low number is a measurement to look at, not a failure and not a pass. `make coverage-fast` is the gate; this is how you see one crate.
# - Its numbers are only as good as its object list. A directory of its own removes the foreign binaries; it does not prove the remaining list is right, and the pruning rule is name-based.
# - It measures one crate, so a file covered by a test living in another package reads lower here than the gate reports it. Compare like with like: this row and a gate row are two different measurements, not one number and its double.
#
# Cost: each crate gets its OWN cold instrumented build the first time it is measured, because the gate's warm build lives in a different directory and cannot be reused. No duration is stated here: this target was written without running it, so any figure would be invented. The shape is what is certain: a full instrumented compile of that crate and everything it depends on, then one merge. Reclaim one crate's with `make sweep-clean SWEEP_DIR=target/llvm-cov-package SWEEP_LIMIT=0` (the limit is 0 because `sweep-clean` otherwise keeps a tree under the threshold, and a single crate's build is usually under it), or take the whole of `target/` with `make clean`.
coverage-package:
	@if [ -z '$(strip $(PACKAGE))' ]; then \
		printf 'coverage-package: needs PACKAGE=<crate>, e.g. make coverage-package PACKAGE=clio-types\n' >&2; \
		exit 2; \
	fi
	@python3 scripts/scoped_runner.py --self-test
	@python3 scripts/scoped_runner.py --coverage-package $(PACKAGE)


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
