from __future__ import annotations

import re

EXIT_USAGE = 2
EXIT_INTERNAL = 70
LOG_TAIL_LINES = 12
#: Terminal formatting, stripped before anything is read out of a tool's help
#: text. Whether a tool colours its output is the caller's setting, so a value
#: read from it has to be the same on a CI runner and on a terminal.
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")

CARGO_KINDS = {
    "test": "test",
    "nextest": "test",
    "llvm-cov": "coverage",
    "clippy": "clippy",
}
#: `run` executes tests, `bench` executes benchmarks, and other subcommands are lookups or utilities; `r` and `b` are nextest aliases, and the subcommand follows `nextest` before any flag.
NEXTEST_EXECUTING_SUBCOMMANDS = frozenset({"run", "r"})
NEXTEST_BENCH_SUBCOMMANDS = frozenset({"bench", "b"})
#: Every `cargo nextest` subcommand the tool advertises, aliases included, read
#: off its own `cargo nextest --help` at 0.9.146. This is not a list of options
#: that take a value, which is what an earlier version of this file carried and
#: which drifts every time the tool grows an option: the walk below needs to
#: recognise a subcommand, not to know which option swallowed the token in front
#: of it. A token that is not one of these is either an option's value or an
#: argument, and both are skipped, so an option this file has never heard of
#: cannot hide the subcommand behind it. A self-test compares this against the
#: installed tool, so a subcommand added upstream fails a check.
NEXTEST_KNOWN_SUBCOMMANDS = frozenset(
    {"list", "run", "r", "bench", "b", "archive", "show-config", "self", "help"}
)
#: This log kind keeps non-executing nextest commands apart from test runs; lookups use this name but no longer use cache verdict slots.
NEXTEST_LOOKUP_KIND = "nextest-lookup"
NEXTEST_BENCH_KIND = "nextest-bench"
#: The `cargo llvm-cov` subcommands that produce a coverage report, read off its
#: own `cargo llvm-cov --help` at 0.9.0. `test`, `run` and `nextest` run something
#: and measure it; `report` measures the data an earlier run left behind.
LLVM_COV_REPORTING_SUBCOMMANDS = frozenset({"test", "run", "report", "nextest"})
#: Every `cargo llvm-cov` subcommand the tool advertises. The set has the same job
#: here that `NEXTEST_KNOWN_SUBCOMMANDS` has there: the walk below needs to
#: recognise a subcommand, not to know which option swallowed the token in front
#: of it, so an option added upstream cannot hide the subcommand behind it.
LLVM_COV_KNOWN_SUBCOMMANDS = frozenset(
    {"test", "run", "report", "show-env", "clean", "nextest"}
)
#: The kind a `cargo llvm-cov` invocation that measures nothing is stored and
#: published under. `CARGO_KINDS` maps the tool rather than what it was asked to
#: do, and that is what let a version or environment probe publish under the
#: `coverage` kind and overwrite a real coverage log: the reader was handed the
#: exit code of a probe as the coverage result. A verdict is only readable if it
#: names the command that produced it.
LLVM_COV_LOOKUP_KIND = "llvm-cov-lookup"
LOOKUP_KINDS = frozenset({NEXTEST_LOOKUP_KIND, LLVM_COV_LOOKUP_KIND})
#: Published logs start with these markers; replays keep the previous <kind>.prev entry.
EXECUTED_LOG_MARKER = b"gate: command ran for this verdict\n"
REPLAYED_LOG_MARKER = b"gate: cached verdict replay; command did not run for this invocation\n"
#: The flags that ask the tool to describe itself rather than measure anything.
#: They stand in for a subcommand, because `cargo llvm-cov --version` carries
#: none, and a bare `cargo llvm-cov` is `cargo llvm-cov test` in every way the
#: gate cares about.
LLVM_COV_PROBE_FLAGS = frozenset({"--version", "-V", "--help", "-h"})
MAKE_KINDS = {
    # `ritual` is the cheap local gate: the format check and the workspace-wide
    # clippy. It runs no test and judges no coverage, so a stored verdict for it
    # is safe to replay in a way a stored test verdict is not. It is listed here
    # so its verdict publishes under a stable kind rather than the generic
    # `make`, which is what the procedure files read out of logs/.
    # `verify` was the previous name for a gate that ran all four checks. It was
    # never in this table, so it was never cached and never published, which made
    # every stage file's claim that the wrapper covered it false. Both names were
    # checked when the target was renamed; this comment exists so the next
    # rename does not repeat it.
    "ritual": "ritual",
    "check": "check",
    "test": "test",
    "lint": "lint",
    "coverage": "coverage",
    "cov": "coverage",
    # A distinct kind from `coverage`, so a replayed verdict is never reported
    # as the full-workspace gate. `coverage-fast` judges only the changed files,
    # and its scope depends on the diff, which is part of the fingerprint.
    "coverage-fast": "coverage-fast",
    # Each of these is a distinct kind too, and for the same reason as
    # `coverage-fast` plus one more: they all used to publish under the plain
    # `coverage` name, so the log a reader is sent to after one of them ran held
    # that one's output instead of the coverage run's, and a report derived from
    # a report was indistinguishable from the report. A verdict is only readable
    # if it names the command that produced it. `cov` is left as an alias of
    # `coverage` because it asks for the same thing under a shorter name.
    "coverage-clean": "coverage-clean",
    "coverage-html": "coverage-html",
    "coverage-lcov": "coverage-lcov",
    "coverage-open": "coverage-open",
    # Measures one package into an instrumented directory of its own, and
    # judges nothing. A distinct kind for the same reason as `coverage-fast`: a
    # replayed verdict published under `coverage` would read as the
    # whole-workspace gate, and this is the opposite of that. The name contains
    # `coverage` on purpose, because that is what makes the self-test's coverage
    # registration assertion apply to it: that a target with `coverage` in its
    # name publishes a registered coverage kind. The other assertion reads every
    # key here, so this one is dispatched by the shim whatever it is called. A
    # diagnostic missing either would be neither cached, locked nor published
    # while looking like a target that had them.
    "coverage-package": "coverage-package",
}
#: Every kind that produces or consumes a coverage report, which is what a clean
#: has to discard: a report measures a build, and a clean removes the build, so a
#: stored verdict for one of these describes a tree that no longer exists. A kind
#: missing from here survives that clean, and its replay then serves a verdict
#: measured against a build that is gone.
#:
#: Registered does not mean this file owns that kind's report, and for most of
#: this set it does not. What a coverage kind saves is the single path
#: `COVERAGE_ARTIFACT` names, because a make target's argument vector never
#: carries `--output-path` -- the recipe that would hold it is inside the
#: Makefile, not in the command the wrapper is handed. Only `make coverage` writes
#: that path. The scoped gate writes a report of its own, a package run writes one
#: named after the package, and the lcov and html targets write neither, so for
#: those kinds this set buys the clean, the lock and the published log rather than
#: an artifact of their own.
COVERAGE_KINDS = frozenset(
    {
        "coverage",
        "coverage-fast",
        "coverage-clean",
        "coverage-html",
        "coverage-lcov",
        "coverage-open",
        "coverage-package",
    }
)
SCOPED_KINDS = {"test", "clippy"}
#: The two values the scoping switch has. It is a switch rather than a dial, so
#: only these are named: an unrecognised value behaves as the default does, and
#: the cache key records the side of the switch a command ran on rather than the
#: spelling used to say it.
GATE_SCOPE_DEFAULT = "quick"
GATE_SCOPE_OFF = "off"
#: Explicit environment values and build-tool prefixes that can change a
#: command belong in the cache key. Hashing the whole environment would put a
#: timestamp or shell session id in the key and every run would miss. The
#: self-test checks values read by this module against these lists.
ENV_KEYS = (
    "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL",
    "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "CLIO_DEPLOYMENT_CONFIG",
    "CLIO_DATABASE_URL",
    "CC",
    "CFLAGS",
    "CXX",
    "CXXFLAGS",
    "GNUMAKEFLAGS",
    "HOME",
    "DYLD_LIBRARY_PATH",
    "LD_LIBRARY_PATH",
    "LDFLAGS",
    "MAKEFLAGS",
    "NEXTEST_PROFILE",
    "OPENSSL_DIR",
    "OPENSSL_INCLUDE_DIR",
    "OPENSSL_LIB_DIR",
    "PATH",
    "PKG_CONFIG_LIBDIR",
    "PKG_CONFIG_PATH",
    "PROFILE",
    "AR",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOC",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    # These values change which workspace packages or base ref a make target uses.
    "GATE_SCOPE",
    "BASE",
    "PACKAGES",
)
ENV_KEY_PREFIXES = (
    "CARGO_",
    "RUST",
    "CC",
    "CXX",
    "AR",
    "CFLAGS",
    "CXXFLAGS",
    "LDFLAGS",
    "PKG_CONFIG_",
    "OPENSSL_",
    "CMAKE_",
    "BINDGEN_",
    "LIBCLANG_",
)
#: Environment values this file reads that are deliberately not in the key, each
#: with the reason it cannot change what a command does. Anything read to decide
#: what runs belongs in `ENV_KEYS` or `ENV_KEY_PREFIXES`; the self-test also
#: checks these exemptions.
#: Reviewed 2026-10-10 per fix-inline-exceptions, keep: read-but-unhashed env values guarded by the env-read self-test.
ENV_KEY_EXEMPT = {
    "CLIO_GATE_LOG_DIR": "chooses where logs are published, not what runs",
    "XDG_CACHE_HOME": "chooses where the cache is kept, not what runs",
    "GATE_FORCE": "decides only whether to consult the cache, and setting it skips it",
    "CLIO_GOLDEN_CAPTURE": "switches on the golden capture, which runs every time and neither reads nor stores a verdict",
}
COVERAGE_ARTIFACT = "target/coverage/coverage.json"
#: The version salt mixed into every cache key. Bump it whenever the key stops
#: meaning what it meant, so an entry written under the old scheme cannot be read
#: back as if it had been written under the new one. Bazel calls this the action
#: cache salt and ccache the namespace; both use it the same way.
#:
#: v3 moved a `cargo llvm-cov` probe out of the `coverage` kind. An entry stored
#: before that was keyed on a kind the tool no longer publishes under, and it
#: would otherwise be readable as a coverage verdict.
#:
#: v4 keys the tree on the content of each changed path instead of the diff
#: text plus a separate untracked list, so a verdict measured before `git add`
#: replays after it.
#: v5 normalises the cargo/make executable paths and includes `BASE`, which make reads from the environment, in the fingerprint.
#: v6 hashes tool contents and active Rustup toolchain binaries, relevant Cargo
#: configuration, prefixed build environment inputs, and equivalent spellings.
KEY_SCHEME = b"clio-gate-v6\0"
USAGE = "usage: gate.py run [--force] [--log-dir DIR] -- CMD ARGS... | gate.py --self-test"
