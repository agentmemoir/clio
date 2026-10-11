# The self-tests and the suites that live under `scripts/`. These files decide what the gates report and whether they report it at all: `scripts/gate.py` answers a repeated heavy command with a stored verdict, and `scripts/scoped_runner.py` decides which packages a scoped gate measures, so a wrong answer in either lands on an unrelated check rather than on the thing that is wrong.
#
# One target per suite, and one CI step per suite, because `make` stops at the first failing command. A single target running all of them reports one failure even when three suites are broken, and the check list shows one status where there is one per suite. Three of them had no step in CI on master and nothing ran them there; the others were steps of their own, and kept their names.
#
# What each one is, taken from its own file. `gate.py` answers whether to run a heavy command and what verdict to store, so its suite is the one that matters most here. `lint_scope_check.py` reads the Makefile, `mk/*.mk`, the workflows and the root manifest, and refuses a clippy run that could select fewer crates than the file claims it selects. `toolchain_pin_check.py` refuses a workflow that installs a compiler other than the pin. The two are separate files because they answer unrelated questions and a reader debugging one should not have to read the other to find out which file to open. `span_verify.py` is not machinery: it is the reference span-grounding check that `extract_quality.py` imports, and running it directly executes the self-check in its `__main__`. `test_phase_timer.py` and the `test_precommit*.py` files are suites for the phase timer and the pre-commit runner. `coverage_surface_check.py` reads the coverage commands in the Makefile and `mk/*.mk`, the workflows and the gate's own two command builders, and refuses a shape that could no longer measure one run and report it once: a per-package run nothing keeps inside its selection, one command doing both jobs at once, and a report step whose merge one malformed buffer can end.
#
#  make scripts-check                   all of them, stopping at the first failure
#  make scripts-check-precommit         one of them, named
scripts-check: scripts-check-gate scripts-check-scoped-runner \
	scripts-check-lint-scope scripts-check-toolchain-pin scripts-check-span \
	scripts-check-phase-timer \
	scripts-check-precommit scripts-check-coverage-surface \
	scripts-check-coverage-policy scripts-check-gate-install scripts-check-quick \
	scripts-check-ci-parity

scripts-check-gate:
	python3 scripts/gate.py --self-test

# The scoped runner's own tests, including the source matcher that decides a no-code file. It is the gate CI runs, so a break in its selection or its judgement would land on every other check.
scripts-check-scoped-runner:
	python3 scripts/scoped_runner.py --self-test

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
	python3 scripts/test_precommit_scopes.py
	python3 scripts/test_precommit_shim.py
	python3 scripts/test_precommit_all_files.py

# Whether the coverage surface can still measure one run and report it once. The run is split into a test step and a report step so the gate owns a boundary in which to drop a profile buffer it cannot read and an instrumented binary this run did not build, and the report step names a merge failure mode so one bad buffer costs its own coverage instead of the whole verdict. All four protections are invisible to a passing run, which is why nothing else in the tree would notice their removal. Runs no build and no coverage command.
scripts-check-coverage-surface:
	python3 scripts/coverage_surface_check.py --self-test
	python3 scripts/coverage_surface_check.py

# Whether the tree still holds exactly one coverage rule. The four rules that were not this one all arrived in changes about something else, and nothing stopped them, so this states the rule as shapes that must not come back: a threshold flag handed to the coverage tool, a retired mechanism by name, a second threshold constant, a second place a line percentage is compared, and prose that reads as a live second criterion. It is a lint over a closed list, not a proof, and its own header says where it cannot see. Runs no build and no coverage command.
scripts-check-coverage-policy:
	python3 scripts/coverage_policy_check.py --self-test
	python3 scripts/coverage_policy_check.py

# Whether the gate installer tells the truth about a shim it just linked. A symlink that comes after the real tool on PATH never runs, and the installer used to print a success line for it. This pins the NOT ACTIVE message and the non-zero exit, with a controlled PATH so the answer is the same on any host.
scripts-check-gate-install:
	python3 scripts/test_gate_install.py

# Whether CI runs every suite `scripts-check` runs, and nothing else. A suite added here with no step in the workflow runs on a developer's machine and nowhere else, which is how the gate installer check and the Quick check went unrun. Reads this file and the workflow; runs no build.
.PHONY: scripts-check-ci-parity
scripts-check-ci-parity:
	python3 scripts/ci_parity_check.py --self-test
	python3 scripts/ci_parity_check.py
