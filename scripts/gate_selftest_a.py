from __future__ import annotations

import os
import re
import shutil
import subprocess
import unittest
from pathlib import Path
from unittest import mock

from exec_tempdir import exec_tempdir
from gate import *  # noqa: F403 - re-exports every engine name the tests reference

def state(head: str, contents=(), changed=()) -> TreeState:
    return TreeState(head, tuple(contents), tuple(changed))

members = [
    Member("a", "/repo/crates/a", ("b",)),
    Member("b", "/repo/crates/b", ()),
    Member("c", "/repo/crates/c", ()),
]

class Tests(unittest.TestCase):
    def test_fingerprint_stable_and_sensitive(self):
        env = {"CLIO_DATABASE_URL": "x"}
        touched = ("crates/a/src/lib.rs",)
        baseline = fingerprint(state("h", changed=touched), env, "test", ["cargo", "test"])
        self.assertEqual(baseline, fingerprint(state("h", changed=touched), env, "test", ["cargo", "test"]))
        self.assertNotEqual(baseline, fingerprint(state("h2", changed=touched), env, "test", ["cargo", "test"]))
        self.assertNotEqual(
            baseline,
            fingerprint(state("h", contents=(("crates/a/src/lib.rs", "d2"),), changed=touched), env, "test", ["cargo", "test"]),
        )
        self.assertNotEqual(
            baseline,
            fingerprint(state("h", contents=(("new.rs", "z"),), changed=touched), env, "test", ["cargo", "test"]),
        )
        self.assertNotEqual(baseline, fingerprint(state("h", changed=touched), {"CLIO_DATABASE_URL": "y"}, "test", ["cargo", "test"]))
        self.assertNotEqual(baseline, fingerprint(state("h", changed=touched), env, "test", ["cargo", "test", "--workspace"]))
        # The kind is hashed as well as the command, so the name a verdict is
        # published under is part of what the entry is keyed on rather than
        # something decided after the lookup.
        self.assertNotEqual(baseline, fingerprint(state("h", changed=touched), env, "coverage", ["cargo", "test"]))

    def test_the_package_list_from_the_environment_separates_entries(self):
        # Two `make test` runs that differ only in the package list the
        # environment supplies were one command line, so the second was told
        # the first one's exit code. The scoping code reads both of these,
        # which is why they belong in the key.
        touched = ("crates/a/src/lib.rs",)
        argv = ["make", "test"]
        one = fingerprint(state("h", changed=touched), {"PACKAGES": "-p a"}, "test", argv)
        other = fingerprint(state("h", changed=touched), {"PACKAGES": "-p b"}, "test", argv)
        self.assertNotEqual(one, other)
        for key in ("PACKAGES", "GATE_SCOPE"):
            self.assertIn(key, ENV_KEYS)
        # An unset scoping switch and its default name are one command, and
        # so is a value the code never acts on, because the switch has two
        # sides rather than a range.
        for spelling in ({}, {"GATE_SCOPE": "quick"}, {"GATE_SCOPE": "junk"}, {"GATE_SCOPE": ""}):
            self.assertEqual(
                fingerprint(state("h", changed=touched), {}, "test", argv),
                fingerprint(state("h", changed=touched), spelling, "test", argv),
                spelling,
            )
        self.assertNotEqual(
            fingerprint(state("h", changed=touched), {}, "test", argv),
            fingerprint(state("h", changed=touched), {"GATE_SCOPE": "off"}, "test", argv),
        )
        # The default the key normalises to and the default the code branches
        # on are one value, not two copies of a literal that can drift.
        self.assertEqual(GATE_SCOPE_DEFAULT, "quick")
        self.assertEqual(GATE_SCOPE_OFF, "off")

    def test_every_environment_read_that_changes_a_run_is_in_the_key(self):
        # The one collision this branch fixed was a variable the scoping code
        # read and the key ignored. Naming the two variables that happened to
        # cause it restates the fix; parsing this file's own source for every
        # name it reads out of the environment states the rule, so the next
        # such variable fails here rather than in a run that never happened.
        source = "".join(p.read_text() for p in sorted(Path(__file__).resolve().parent.glob("gate*.py")))
        reads = set(re.findall(r'os\.environ(?:\.get\(|\[)\s*"([A-Z_][A-Z0-9_]*)"', source))
        self.assertIn("GATE_SCOPE", reads, "the scan found nothing and is not testing anything")
        for name in sorted(reads - set(ENV_KEYS)):
            self.assertIn(
                name,
                ENV_KEY_EXEMPT,
                f"read from the environment to decide what runs, but absent from ENV_KEYS: {name}. "
                "Two commands differing only in it would share one stored verdict.",
            )
        for name, reason in ENV_KEY_EXEMPT.items():
            self.assertNotEqual("", reason.strip(), f"{name} is exempt with no stated reason")

    def test_the_key_scheme_is_hashed_and_tracked(self):
        # Two separate promises. The salt has to reach the digest, or an
        # entry written under an older key would be read back under a newer
        # one; and the salt has to be recorded, or moving it is invisible.
        touched = ("crates/a/src/lib.rs",)
        env = {}
        kind = "test"
        argv = ["cargo", "test"]
        baseline = fingerprint(state("h", changed=touched), env, kind, argv)
        with mock.patch("gate.KEY_SCHEME", b"clio-gate-some-other-scheme\0"):
            self.assertNotEqual(
                baseline, fingerprint(state("h", changed=touched), env, kind, argv)
            )
        self.assertEqual(b"clio-gate-v4\0", KEY_SCHEME)
        # The version is the fourth, so an entry from the scheme before this
        # one cannot be mistaken for one from this one.
        self.assertTrue(KEY_SCHEME.endswith(b"v4\0"))

    def test_a_test_lookup_is_not_a_test_verdict(self):
        self.assertEqual("test", command_kind(["cargo", "nextest", "run"]))
        self.assertEqual("test", command_kind(["cargo", "nextest", "r"]))
        for argv in (
            ["cargo", "nextest", "list", "--workspace", "--locked"],
            ["cargo", "nextest", "show-config"],
            ["cargo", "nextest", "archive", "--workspace"],
            # `bench` executes benchmark binaries, so it is not a lookup in
            # the ordinary sense. It is bucketed with them anyway because it
            # does not run the test suite, and a benchmark's verdict must
            # never be able to stand in for one.
            ["cargo", "nextest", "bench"],
            ["cargo", "nextest", "help"],
            # Nothing to read, so nothing is claimed.
            ["cargo", "nextest"],
            ["cargo", "nextest", "--help"],
            # A subcommand added upstream that this file has never heard of
            # must not be reported as a run.
            ["cargo", "nextest", "some-future-subcommand"],
        ):
            self.assertEqual(NEXTEST_LOOKUP_KIND, command_kind(argv), argv)
        self.assertNotIn(NEXTEST_LOOKUP_KIND, SCOPED_KINDS)

    def test_the_nextest_subcommand_survives_its_own_options(self):
        # nextest puts its options before the subcommand, so the token after
        # `nextest` is not always the subcommand. These are the shapes that
        # make reading one token wrong.
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "run", "-p", "a"]))
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--verbose", "run"]))
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "-v", "run"]))
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--no-pager", "run"]))
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--color", "always", "run"]))
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--color=always", "run"]))
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "-P", "ci", "run"]))
        self.assertEqual("list", nextest_subcommand(["cargo", "nextest", "--manifest-path", "Cargo.toml", "list"]))
        # The shape a list of options taking values was read to handle, and
        # the one it could not survive: an option that takes a value this
        # file has never heard of. The value is skipped because it is not a
        # subcommand, so the walk never needed to know the option existed.
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--new-upstream-option", "VALUE", "run"]))
        self.assertEqual("test", command_kind(["cargo", "nextest", "--new-upstream-option", "VALUE", "run"]))
        self.assertEqual("", nextest_subcommand(["cargo", "nextest"]))
        # A filter after `--` may be spelled like a subcommand and must not
        # be read as one.
        self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "run", "--", "list"]))
        self.assertEqual("", nextest_subcommand(["cargo", "nextest", "--", "list"]))
        for shape in (["cargo", "nextest", "--verbose", "run"], ["cargo", "nextest", "--color", "always", "run"]):
            self.assertEqual("test", command_kind(shape), shape)

    def test_the_known_subcommands_match_the_installed_nextest(self):
        # The list is read off the tool's own help, and nextest adds
        # subcommands. One this file has never heard of is classified as a
        # lookup, which is the safe answer but silently loses quick scoping
        # and publishes under a name that may not describe the command, so a
        # new subcommand has to fail a check rather than change behaviour.
        #
        # The real tool is invoked with GATE_ACTIVE set, which is what the
        # shim looks for to pass straight through, so asking the question
        # cannot itself be cached, locked or replayed.
        proc = subprocess.run(
            ["cargo", "nextest", "--help"],
            capture_output=True,
            text=True,
            check=False,
            env=dict(os.environ, GATE_ACTIVE="1"),
        )
        if proc.returncode != 0 or "Commands:" not in proc.stdout:
            self.skipTest("cargo nextest is not installed on this machine")
        # Whether the tool colours its help is the caller's decision, and CI
        # sets CARGO_TERM_COLOR=always, so the names arrive wrapped in escape
        # sequences there and bare on a terminal. Stripping the formatting is
        # what makes this check read the same names on both, rather than
        # asserting a property of whichever machine ran it.
        plain = ANSI_ESCAPE.sub("", proc.stdout)
        listed: set[str] = set()
        # The heading is followed by a newline of its own, so the remainder
        # is split from the first line that carries a name.
        body = plain.partition("Commands:")[2].lstrip("\n")
        for line in body.splitlines():
            if not line.strip():
                break
            parts = line.split()
            if not parts:
                continue
            listed.add(parts[0])
            # `run` is annotated `[alias: r]`, so an alias is a name this
            # file has to recognise too or an aliased call reads as
            # unreadable.
            for alias in re.findall(r"\[alias:\s*([^\]\s]+)\]", line):
                listed.add(alias)
        self.assertTrue(listed, "the help output was parsed into nothing")
        for name in sorted(listed):
            self.assertEqual(name, name.strip(), f"{name!r} carries formatting into the comparison")
        self.assertEqual(
            sorted(listed),
            sorted(NEXTEST_KNOWN_SUBCOMMANDS),
            "the subcommands this file recognises no longer match the installed tool. "
            "Anything new is classified as a lookup rather than a run, so add it to "
            "NEXTEST_KNOWN_SUBCOMMANDS, and to NEXTEST_EXECUTING_SUBCOMMANDS if it runs tests.",
        )
        self.assertTrue(
            NEXTEST_EXECUTING_SUBCOMMANDS <= listed,
            "this file claims a subcommand runs tests that the tool does not offer",
        )

    def test_a_recognised_subcommand_is_never_taken_from_behind_a_separator(self):
        # The walk stops at `--` because everything past it is the
        # subcommand's own argument, and a filter is allowed to be spelled
        # exactly like a subcommand. Reading one would classify a filtered
        # run as a lookup.
        self.assertIsNone(nextest_subcommand_index(["cargo", "nextest", "--", "run"]))
        self.assertEqual(2, nextest_subcommand_index(["cargo", "nextest", "run", "--", "list"]))
        # An option's value may also be spelled like a subcommand, and there
        # is no way to tell that from a real one. The first match wins,
        # because a subcommand always precedes its own arguments, so the
        # only way to reach this is an option whose value is spelled exactly
        # like a subcommand name.
        self.assertEqual(3, nextest_subcommand_index(["cargo", "nextest", "--color", "list", "run"]))

    def test_a_coverage_probe_is_not_a_coverage_verdict(self):
        # The defect this classifies: `CARGO_KINDS` maps the tool, so
        # `cargo llvm-cov --version` published under `coverage` and
        # overwrote a real coverage log with the exit code of a version
        # probe. Every shape that measures nothing has to land in the
        # lookup kind, and every shape that measures has to stay out of it.
        for argv in (
            # A version probe, which is the shape the plan names.
            ["cargo", "llvm-cov", "--version"],
            ["cargo", "llvm-cov", "-V"],
            # An environment dump, the other one named.
            ["cargo", "llvm-cov", "show-env"],
            # Removes the artifacts and reports nothing.
            ["cargo", "llvm-cov", "clean"],
            ["cargo", "llvm-cov", "clean", "--workspace"],
            # Describing itself.
            ["cargo", "llvm-cov", "--help"],
            # A probe flag and a subcommand in the same line. The tool
            # answers the flag and writes no report, so reading the
            # subcommand first would publish its exit code as a coverage
            # verdict, which is the defect this whole classification exists
            # to stop.
            ["cargo", "llvm-cov", "--version", "report"],
            ["cargo", "llvm-cov", "--help", "test"],
        ):
            self.assertEqual(LLVM_COV_LOOKUP_KIND, command_kind(argv), argv)
        self.assertNotIn(LLVM_COV_LOOKUP_KIND, COVERAGE_KINDS)
        self.assertNotIn(LLVM_COV_LOOKUP_KIND, SCOPED_KINDS)
        # The other side. A bare `cargo llvm-cov` is documented as
        # equivalent to `cargo llvm-cov test`, so it measures and must stay
        # a coverage run; a probe must not have narrowed that.
        for argv in (
            ["cargo", "llvm-cov"],
            ["cargo", "llvm-cov", "--workspace"],
            ["cargo", "llvm-cov", "--summary-only"],
            ["cargo", "llvm-cov", "test", "--workspace"],
            ["cargo", "llvm-cov", "run"],
            ["cargo", "llvm-cov", "nextest", "--workspace"],
            ["cargo", "llvm-cov", "report"],
        ):
            self.assertEqual("coverage", command_kind(argv), argv)
        # Coverage is never narrowed, whatever produced it.
        for argv in (["cargo", "llvm-cov", "report"], ["cargo", "llvm-cov", "test"]):
            self.assertEqual(
                (argv, None), scope_command(command_kind(argv), argv, "/repo", ("crates/b/src/lib.rs",))
            )

    def test_the_llvm_cov_subcommand_survives_its_own_options(self):
        # Same shapes as the nextest walk, and the same reason. llvm-cov
        # writes `[SUBCOMMAND] [OPTIONS]`, so a leading option must not hide
        # the subcommand behind it.
        self.assertEqual(2, llvm_cov_subcommand_index(["cargo", "llvm-cov", "report", "--json"]))
        self.assertEqual(
            3, llvm_cov_subcommand_index(["cargo", "llvm-cov", "--verbose", "show-env"])
        )
        # An option this file has never heard of, whose value is skipped
        # because it is not a subcommand.
        self.assertEqual(
            4, llvm_cov_subcommand_index(["cargo", "llvm-cov", "--new-upstream-option", "VALUE", "clean"])
        )
        self.assertIsNone(llvm_cov_subcommand_index(["cargo", "llvm-cov", "--", "clean"]))
        self.assertIsNone(llvm_cov_subcommand_index(["cargo", "llvm-cov"]))
        # An option's value may be spelled like a subcommand, and there is no
        # way to tell that from a real one. The first match wins.
        self.assertEqual(3, llvm_cov_subcommand_index(["cargo", "llvm-cov", "--manifest-path", "clean", "run"]))

    def test_the_two_lists_of_llvm_cov_subcommands_are_one_list(self):
        # The same rule the nextest pair holds to: a subcommand in one list
        # and not the other is a command published under a kind that says it
        # is not one.
        self.assertTrue(LLVM_COV_REPORTING_SUBCOMMANDS)
        self.assertTrue(LLVM_COV_REPORTING_SUBCOMMANDS <= LLVM_COV_KNOWN_SUBCOMMANDS)
        for sub in sorted(LLVM_COV_REPORTING_SUBCOMMANDS):
            self.assertEqual("coverage", command_kind(["cargo", "llvm-cov", sub]), sub)
        for sub in sorted(LLVM_COV_KNOWN_SUBCOMMANDS - LLVM_COV_REPORTING_SUBCOMMANDS):
            self.assertEqual(LLVM_COV_LOOKUP_KIND, command_kind(["cargo", "llvm-cov", sub]), sub)

    def test_the_known_llvm_cov_subcommands_match_the_installed_tool(self):
        # Read off the tool's own help, for the reason the nextest check
        # gives: an unrecognised subcommand is classified as a probe, which
        # is the safe side but silently publishes a coverage run under the
        # wrong name, so an upstream addition has to fail a check rather than
        # change behaviour. GATE_ACTIVE is what the shim looks for to pass
        # straight through, so asking the question cannot itself be cached.
        proc = subprocess.run(
            ["cargo", "llvm-cov", "--help"],
            capture_output=True,
            text=True,
            check=False,
            env=dict(os.environ, GATE_ACTIVE="1"),
        )
        if proc.returncode != 0 or "SUBCOMMANDS:" not in proc.stdout:
            self.skipTest("cargo llvm-cov is not installed on this machine")
        plain = ANSI_ESCAPE.sub("", proc.stdout)
        body = plain.partition("SUBCOMMANDS:")[2].lstrip("\n")
        listed: set[str] = set()
        for line in body.splitlines():
            if not line.strip():
                break
            # Every subcommand in this tool's own help carries a
            # description on the following lines, so the run of indented
            # description text is skipped until the next bare name.
            if line.startswith("        ") or line.startswith("\t"):
                continue
            parts = line.split()
            if not parts:
                continue
            listed.add(parts[0])
        self.assertTrue(listed, "the help output was parsed into nothing")
        self.assertEqual(
            sorted(listed),
            sorted(LLVM_COV_KNOWN_SUBCOMMANDS),
            "the llvm-cov subcommands this file recognises no longer match the installed tool. "
            "Anything new is classified as a probe rather than a coverage run, so add it to "
            "LLVM_COV_KNOWN_SUBCOMMANDS, and to LLVM_COV_REPORTING_SUBCOMMANDS if it produces a report.",
        )
        self.assertTrue(
            LLVM_COV_REPORTING_SUBCOMMANDS <= listed,
            "this file claims a subcommand produces a report that the tool does not offer",
        )

    def test_a_make_dry_run_is_never_gated(self):
        # The second defect. The shim's loop skipped every argument starting
        # with `-`, so the dry-run flag never stopped it: `make -n coverage`
        # was classified heavy, and the recipe list it printed was published
        # as the coverage verdict under the coverage kind's name, replacing
        # the log of the run it stood in for. Every spelling GNU make accepts
        # has to pass through.
        shim = Path(__file__).resolve().parent / "gate-shim"
        # Driven rather than parsed. The shim is a shell script and the
        # defect was in its loop, so a check that only reads the file would
        # pass against a shim carrying the right words and the same bug. The
        # shim is installed as a symlink named `make` earlier on PATH, with
        # a stand-in behind it, exactly as `gate-install.sh` does it: the
        # stand-in records whether the gate wrapped it, and the gate exports
        # GATE_ACTIVE to the command it runs, so a direct pass-through leaves
        # no mark.
        with exec_tempdir(purpose="a stand-in make command") as tmp:
            shimdir = Path(tmp) / "shim"
            realdir = Path(tmp) / "real"
            shimdir.mkdir()
            realdir.mkdir()
            marker = Path(tmp) / "gated"
            stand_in = realdir / "make"
            stand_in.write_text(
                f'#!/bin/sh\n'
                f'if [ "${{GATE_ACTIVE:-}}" = 1 ]; then echo gated >> "{marker}"; fi\n'
                f'exit 0\n'
            )
            stand_in.chmod(0o755)
            (shimdir / "make").symlink_to(shim)
            env = dict(
                os.environ,
                PATH=f"{shimdir}{os.pathsep}{realdir}{os.pathsep}{os.environ.get('PATH', '')}",
                GATE_ACTIVE="",
            )
            # The repo root is the working directory on purpose: the shim
            # asks git for the checkout before it classifies anything, so a
            # run from anywhere else would pass through for that reason
            # instead and every row below would be measuring the wrong thing.
            root = Path(__file__).resolve().parent.parent
            for row, (args, makeflags, expect_gated) in enumerate(
                (
                    # Every spelling of the dry-run flag as an argument.
                    (["-n", "coverage"], "", False),
                    (["--dry-run", "coverage"], "", False),
                    (["--just-print", "coverage"], "", False),
                    (["--recon", "coverage"], "", False),
                    (["-ns", "coverage"], "", False),
                    (["-C", ".", "-n", "coverage"], "", False),
                    (["coverage", "-n"], "", False),
                    # And through MAKEFLAGS, which GNU make also reads.
                    (["coverage"], "-n", False),
                    (["coverage"], "--dry-run", False),
                    # Real options carrying an `n`, which must stay gated.
                    # A check that simply refused anything with an `n` in it
                    # would pass the rows above and fail these.
                    (["--no-print-directory", "coverage"], "", True),
                    (["-e", "coverage"], "", True),
                    # The same, arriving as an export rather than an argument.
                    (["coverage"], "--no-print-directory", True),
                    (["coverage"], "--output-sync=target", True),
                    (["coverage"], "--keep-going", True),
                    (["coverage"], "-j4", True),
                    # The plain call, so a check that gated nothing at all
                    # fails here rather than passing every row above.
                    (["coverage"], "", True),
                )
            ):
                before = marker.read_text() if marker.is_file() else ""
                # A private cache per row, because the gate's whole point is
                # that a repeat on an unchanged tree replays instead of
                # running. Sharing one cache made every gated row after the
                # first replay, so the stand-in was never reached and the row
                # failed for a reason that had nothing to do with the flag.
                cache = Path(tmp) / f"cache-{row}"
                call_env = dict(env, MAKEFLAGS=makeflags, XDG_CACHE_HOME=str(cache))
                proc = subprocess.run(
                    [str(shimdir / "make"), *args],
                    capture_output=True,
                    text=True,
                    check=False,
                    env=call_env,
                    cwd=root,
                )
                after = marker.read_text() if marker.is_file() else ""
                self.assertEqual(
                    expect_gated,
                    len(after.splitlines()) > len(before.splitlines()),
                    f"{args} MAKEFLAGS={makeflags!r}: the shim "
                    f"{'gated' if expect_gated else 'passed through'} nothing",
                )
                if not expect_gated:
                    self.assertEqual(0, proc.returncode, args)
                shutil.rmtree(cache, ignore_errors=True)
