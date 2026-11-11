# CodeQL — `/run-codeql`

`/run-codeql` as the whole text of an issue or pull request comment runs a CodeQL analysis of the Rust workspace, analysing the pull request head when the comment is on a pull request, and reports as the check `codeql / on demand`. Expect 5 to 8 minutes: `build-mode: none` needs no build, so the database is the whole cost.

It is a separate workflow (`codeql-command.yml` → `codeql.yml`): the comment must be exactly `/run-codeql`, and the gate denies anyone without repository write access. The command only works once the workflow exists on the default branch, because GitHub reads `issue_comment` workflows from the default branch and not from the pull request the comment is on. It is not a required check and does not block a merge, and it never runs on `pull_request`.

`codeql.yml` also analyses the same language on a daily schedule (03:17 UTC) and on manual dispatch, and a scheduled result lands in one open issue labelled `codeql` rather than on the pull request path. No local wrapper exists for it: the CodeQL CLI is not installed locally, so the scan runs in Actions only.
