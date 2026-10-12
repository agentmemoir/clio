# Changelog fragments

This directory holds one file per user-facing change. The release script collects them into `CHANGELOG.md` and deletes them, so concurrent pull requests never edit the same file. Two pull requests that pick the same name collide at merge time, which is the signal to rename; nothing checks across branches.

Add a file named `<slug>.<category>.md`, for example `add-user-authentication.feature.md`. The slug is lowercase words joined by hyphens, at least 8 characters, and it ends with `-issues-<numbers>` when the change has issues, for example `fix-session-expiration-issues-45-46.bugfix.md`. The name carries no pull request number: the number does not exist yet when the file is written, and the name must already be unique inside `changelog.d/`.

The category is one of `feature`, `bugfix`, or `improvement`. The release script renders them as `Added`, `Fixed`, or `Changed`, the `Keep a Changelog` sections most entries fall under. A security fix, a deprecation, or a removal still starts as a fragment with the closest category and a plain name; the release manager places it under the right heading when building the release.

Write one paragraph of user-facing prose, at least 12 characters, on one line. Say what a reader deciding whether to upgrade will notice, not what moved between files. A production-code change with no user-visible effect carries exactly `No user-facing change.` instead, which satisfies the check and is left out of the release. Never edit `CHANGELOG.md` by hand on a normal pull request; only the release script writes it.

Which rules are whose: the one-file-per-change shape and the `Added`, `Fixed`, `Changed` headings follow `towncrier` practice and `Keep a Changelog`; the descriptive slug, the 8-character and 12-character minimums, and the one-line rule are this directory's own checks, enforced by `scripts/changelog_fragments.py`.

CI checks every file here for a valid name and real prose, and it fails a pull request that changes product code without adding a file here. Run `python3 scripts/changelog_fragments.py --check` before you push, with `--base origin/master` to run the same presence check CI runs.
