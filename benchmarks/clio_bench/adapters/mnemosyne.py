"""Mnemosyne system adapter for clio-bench.

Drives the real ``mnemosyne`` command-line binary so the harness can benchmark
a second, independent memory system the same way it benchmarks Clio. Every
benchmark session maps to its own memory bank for isolation, and the data
directory can be pointed at a scratch location so runs stay hermetic.

The system under test is Mnemosyne, the ``mnemosyne-memory`` distribution
(version 3.15.1, MIT). Its entry point is the ``mnemosyne`` CLI, which this
adapter reaches with the standard library only; no Mnemosyne client library is
imported into the harness.

Three of the CLI's documented commands are driven: ``store`` with an explicit
source and importance, ``recall`` with ``--json``, and ``bank list`` as the
readiness probe. ``reset`` is the fourth interface method and it does **not**
shell out: it removes the session's bank directory directly, the way the Clio
adapter removes its session database file, so clearing a session needs no
round trip and cannot be refused. The ``bank create`` and ``bank delete``
subcommands exist upstream but this adapter never invokes them; a missing bank
directory is already an empty bank, so creating one before the first write buys
nothing and deleting one is what the direct removal already does.

`answer` is left to the interface's optional default. Mnemosyne exposes no
question-answering command: its ``mcp`` server offers retrieval tools to a
host model, which is the harness's own answerer rather than the system's, so
implementing it here would measure the harness rather than the rival.

Configuration surface. Mnemosyne resolves settings as config.yaml over
environment variable over built-in default, and it reads ``MNEMOSYNE_BANK`` and
``MNEMOSYNE_DATA_DIR`` straight from the environment. This adapter pins the
data directory so a run cannot inherit a host store or a host ``config.yaml``,
which is what makes two systems' settings comparable at all. The levers that
move recall ranking remain readable rather than forced, and they are the ones a
later configuration fingerprint has to record: ``vec_weight`` (0.5),
``fts_weight`` (0.3), ``importance_weight`` (0.2), normalized to sum to 1.0;
``recency_halflife`` (168 hours); ``tier2_days``/``tier2_weight`` (30/0.5) and
``tier3_days``/``tier3_weight`` (180/0.25); ``wm_max_items`` (10000);
``wm_ttl_hours`` (168); ``cross_session`` (false); ``embedding_model``
(BAAI/bge-small-en-v1.5); ``embedding_dim`` (384); ``vec_type`` (int8). Pass
them through ``extra_env``. ``mnemosyne doctor --bank NAME --format json`` is
the system's own read-only report and is the natural fingerprint source.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import time

from ..adapter import MemoryBenchmarkAdapter
from ..fingerprint import (
    Fingerprint,
    FingerprintError,
    SettingRecord,
    read_setting,
)
from ..tokenizer import ReferenceTokenizer
from ..types import DialogueTurn, IngestResult, RetrievalResult

_STDERR_LIMIT = 500
_STORED_PREFIX = "Stored: "
# Mnemosyne reports a declined write by printing this literal in the id slot and
# exiting zero, so a refusal is read from the payload and never from the status.
_REFUSED_ID = "None"
# The configuration keys that move recall ranking, read one at a time through
# the system's own `config get` surface. The list is the adapter docstring's
# set, and it is this system's fingerprint minimum.
REQUIRED_SETTINGS = (
    "vec_weight",
    "fts_weight",
    "importance_weight",
    "recency_halflife",
    "tier2_days",
    "tier2_weight",
    "tier3_days",
    "tier3_weight",
    "wm_max_items",
    "wm_ttl_hours",
    "cross_session",
    "embedding_model",
    "embedding_dim",
    "vec_type",
)


class MnemosyneAdapterError(RuntimeError):
    """Raised when the mnemosyne binary fails or returns unusable output."""


class MnemosyneAdapter(MemoryBenchmarkAdapter):
    """Benchmark adapter that shells out to the mnemosyne CLI."""

    def __init__(
        self,
        *,
        binary: str = "mnemosyne",
        data_dir: str | None = None,
        bank_prefix: str = "clio-bench",
        source: str = "clio-bench",
        tokenizer: ReferenceTokenizer,
        limit: int = 10,
        importance: float = 0.5,
        timeout: int = 600,
        extra_env: dict[str, str] | None = None,
    ) -> None:
        self._binary = binary
        self._data_dir = data_dir
        self._bank_prefix = bank_prefix
        self._source = source
        self._tokenizer = tokenizer
        self._limit = limit
        self._importance = importance
        self._timeout = timeout
        self._extra_env = dict(extra_env) if extra_env else {}

    # -- helpers ---------------------------------------------------------

    def _bank_for(self, session_id: str) -> str:
        """Return the isolated memory bank name for one session."""
        return f"{self._bank_prefix}-{session_id}"

    def _env(self, bank: str | None) -> dict[str, str]:
        """Build the subprocess environment: os.environ plus overrides.

        The data directory is pinned so the run cannot read a host store or a
        host ``config.yaml``. ``MNEMOSYNE_BANK`` is set per call because the
        bank is the session isolation mechanism and must not be inherited.
        """
        env = dict(os.environ)
        if self._data_dir is not None:
            env["MNEMOSYNE_DATA_DIR"] = self._data_dir
        if bank is not None:
            env["MNEMOSYNE_BANK"] = bank
        env.update(self._extra_env)
        return env

    def _run(
        self,
        args: list[str],
        bank: str | None = None,
        *,
        env_override: dict[str, str] | None = None,
    ) -> str:
        """Run mnemosyne and return its stdout, raising on any harness fault.

        A non-zero exit, an unrunnable binary, and a timeout are all the
        harness's own faults, so they raise a named error. A product refusal
        exits zero and is read from the payload by the caller. `env_override`
        replaces variables for one call; the version probe uses it to point a
        short-lived bank at a scratch directory.
        """
        env = self._env(bank)
        if env_override:
            env.update(env_override)
        try:
            proc = subprocess.run(
                [self._binary, *args],
                capture_output=True,
                text=True,
                timeout=self._timeout,
                env=env,
            )
        except subprocess.TimeoutExpired as exc:
            raise MnemosyneAdapterError(
                f"mnemosyne {' '.join(args)} timed out after {self._timeout}s"
            ) from exc
        except OSError as exc:
            raise MnemosyneAdapterError(
                f"could not run mnemosyne binary {self._binary!r}: {exc}"
            ) from exc
        if proc.returncode != 0:
            stderr = (proc.stderr or "").strip()
            raise MnemosyneAdapterError(
                f"mnemosyne {' '.join(args)} exited {proc.returncode}: "
                f"{stderr[:_STDERR_LIMIT]}"
            )
        return proc.stdout or ""

    @staticmethod
    def _last_json_object(stdout: str, command: str) -> dict:
        """Return the last stdout line that parses to a JSON object.

        Mnemosyne prints progress noise before its payload, so parsing is
        defensive and a payload that never appears is a named error rather
        than an empty result that would read as a successful empty retrieval.
        """
        for line in reversed(stdout.splitlines()):
            line = line.strip()
            if not line:
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(payload, dict):
                return payload
        raise MnemosyneAdapterError(
            f"mnemosyne {command} produced no JSON object on stdout"
        )

    @staticmethod
    def _json_document(stdout: str, command: str) -> dict:
        """Return a JSON object from stdout, one-line or pretty-printed.

        `doctor` pretty-prints its report over many lines, so the whole stdout
        is parsed first and the line scan stays as the fallback for output that
        prints progress noise before a one-line payload.
        """
        stripped = stdout.strip()
        if stripped:
            try:
                payload = json.loads(stripped)
            except json.JSONDecodeError:
                payload = None
            if isinstance(payload, dict):
                return payload
        return MnemosyneAdapter._last_json_object(stdout, command)

    @staticmethod
    def _stored_id(stdout: str) -> str:
        """Return the memory id from a ``Stored: <id>`` line."""
        for line in stdout.splitlines():
            line = line.strip()
            if line.startswith(_STORED_PREFIX):
                return line[len(_STORED_PREFIX) :].strip()
        raise MnemosyneAdapterError("mnemosyne store printed no 'Stored:' line")

    # -- interface -------------------------------------------------------

    def reset(self, session_id: str) -> None:
        """Drop the session's bank directory and recreate the banks root.

        Deleting the bank directory is what clears one session without
        touching another's, and it mirrors the Clio adapter, which removes its
        session database file. Without a pinned data directory there is no
        harness-owned store to clear, so nothing is deleted.
        """
        if self._data_dir is None:
            return
        banks_root = os.path.join(self._data_dir, "banks")
        shutil.rmtree(os.path.join(banks_root, self._bank_for(session_id)), ignore_errors=True)
        os.makedirs(banks_root, exist_ok=True)

    def ingest(self, session_id: str, turn: DialogueTurn) -> IngestResult:
        """Store one dialogue turn through ``mnemosyne store``.

        The system can decline a write on its own admission rules and reports
        that by printing no id. That is a normal, recorded outcome
        (``ok=False``), not a harness failure.
        """
        start = time.monotonic()
        stdout = self._run(
            [
                "store",
                turn.text,
                self._source,
                str(self._importance),
            ],
            self._bank_for(session_id),
        )
        latency_ms = (time.monotonic() - start) * 1000.0
        ok = self._stored_id(stdout) != _REFUSED_ID
        return IngestResult(
            ok=ok,
            latency_ms=latency_ms,
            tokens_processed=self._tokenizer.count(turn.text),
            error=None if ok else "mnemosyne declined to store the turn",
        )

    def retrieve(
        self, session_id: str, query: str, token_budget: int
    ) -> RetrievalResult:
        """Recall hits for a query and pack them into the token budget."""
        start = time.monotonic()
        stdout = self._run(
            ["recall", query, str(self._limit), "--json"],
            self._bank_for(session_id),
        )
        latency_ms = (time.monotonic() - start) * 1000.0
        payload = self._last_json_object(stdout, "recall")
        results = payload.get("results")
        if not isinstance(results, list):
            raise MnemosyneAdapterError("mnemosyne recall returned no results list")

        pieces: list[str] = []
        item_ids: list[str] = []
        used = 0
        for hit in results:
            if not isinstance(hit, dict):
                continue
            item_id = str(hit.get("id") or "")
            line = f"[{item_id}] {hit.get('content') or ''}"
            cost = self._tokenizer.count(line)
            if used + cost > token_budget:
                break
            pieces.append(line)
            item_ids.append(item_id)
            used += cost

        return RetrievalResult(
            context="\n".join(pieces),
            latency_ms=latency_ms,
            tokens_retrieved=used,
            item_ids=tuple(item_ids),
        )

    def healthcheck(self) -> bool:
        """Return True when the binary runs and its bank store is usable.

        ``bank list`` is the probe because it is the one command that answers
        before anything has been written: it resolves the bank store against
        the pinned data directory and exits zero. ``doctor`` is bank-scoped and
        its target does not exist until the first write, so it cannot answer
        this question.
        """
        try:
            self._run(["bank", "list"])
        except MnemosyneAdapterError:
            return False
        return True

    # -- configuration fingerprint ---------------------------------------

    def fingerprint(self) -> Fingerprint:
        """Read Mnemosyne's behaviour-changing settings before any probe runs.

        Ranking weights and embedding settings are read one at a time through
        ``config get``, which reports the effective value after the precedence
        the system documents (config.yaml over environment over defaults). The
        version comes from ``doctor``.
        """
        settings: dict[str, SettingRecord] = {}
        requested_env = self._env(None)
        for key in REQUIRED_SETTINGS:
            setting = self._read_setting(key)
            if key == "cross_session":
                setting = read_setting(
                    value=self._parse_bool_setting(setting.value),
                    surface=setting.surface,
                )
            settings[key] = setting
            env_key = f"MNEMOSYNE_{key.upper()}"
            requested = requested_env.get(env_key)
            if requested is None:
                continue
            if key == "cross_session":
                requested_value = self._parse_bool_setting(requested)
            else:
                requested_value = self._parse_value(requested)
            if setting.value != requested_value:
                raise FingerprintError(
                    f"mnemosyne config.yaml overrides {env_key}: effective {key} is "
                    f"{setting.value!r}, requested {requested!r}; use a fresh data directory"
                )
        return Fingerprint(
            system="mnemosyne",
            version=self._read_version(),
            settings=settings,
            required=REQUIRED_SETTINGS,
        )

    def _read_setting(self, key: str) -> SettingRecord:
        try:
            stdout = self._run(["config", "get", key])
        except MnemosyneAdapterError as exc:
            raise FingerprintError(
                f"mnemosyne config get {key} failed: {exc}"
            ) from exc
        prefix = f"{key} = "
        for line in stdout.splitlines():
            line = line.strip()
            if line.startswith(prefix):
                raw = line[len(prefix) :].strip()
                if raw == "(not set)":
                    raise FingerprintError(
                        f"mnemosyne config key {key} has no effective value"
                    )
                return read_setting(
                    value=self._parse_value(raw),
                    surface=f"mnemosyne config get {key}",
                )
        raise FingerprintError(f"mnemosyne config get {key} printed no value")

    @staticmethod
    def _parse_value(raw: str) -> object:
        """Parse a ``config get`` value, falling back to the raw string.

        The command prints Python literals, so the two boolean spellings and
        ``None`` are mapped before the JSON attempt; JSON covers numbers and
        quoted strings.
        """
        python_literals = {"True": True, "False": False, "None": None}
        if raw in python_literals:
            return python_literals[raw]
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return raw

    @staticmethod
    def _parse_bool_setting(raw: object) -> bool:
        """Match Mnemosyne's `get_bool` conversion for cross_session."""
        if isinstance(raw, bool):
            return raw
        return str(raw).strip().lower() in ("1", "true", "yes", "on")

    def _read_version(self) -> str:
        """Read the system version from ``doctor``, the only surface reporting it.

        ``doctor`` is bank-scoped and needs an existing database, so a probe
        bank is created in a temporary data directory for the read and removed
        with the directory. This probe does not write the run's own data
        directory.
        """
        try:
            with tempfile.TemporaryDirectory(prefix="clio-bench-mnemosyne-") as tmp:
                override = {"MNEMOSYNE_DATA_DIR": tmp}
                self._run(
                    ["bank", "create", "clio-bench-fingerprint"],
                    env_override=override,
                )
                stdout = self._run(
                    ["doctor", "--bank", "clio-bench-fingerprint", "--format", "json"],
                    env_override=override,
                )
            payload = self._json_document(stdout, "doctor")
        except (MnemosyneAdapterError, OSError) as exc:
            raise FingerprintError(f"mnemosyne version unreadable: {exc}") from exc
        checks = payload.get("runtime_diagnostics", {}).get("checks", [])
        for check in checks:
            if check.get("check") == "mnemosyne_version":
                detail = check.get("detail")
                if isinstance(detail, str) and detail:
                    return detail
        raise FingerprintError("mnemosyne doctor reported no mnemosyne_version")
