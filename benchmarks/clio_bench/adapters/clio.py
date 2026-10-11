"""Clio system adapter for clio-bench.

Drives the real ``clio`` binary through its JSON CLI surface so the harness
can benchmark Clio the same way it benchmarks any other memory system. Every
benchmark session maps to its own memory bank for isolation, and the data
directory can be pointed at a scratch location so runs stay hermetic.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import time

from ..adapter import MemoryBenchmarkAdapter
from ..fingerprint import (
    Fingerprint,
    FingerprintError,
    SettingRecord,
    read_setting,
    unreadable_setting,
)
from ..packing import pack_context
from ..tokenizer import ReferenceTokenizer
from ..types import DialogueTurn, IngestResult, RetrievalResult

_STDERR_LIMIT = 500
_RETENTION_ENV = "CLIO_RETENTION_PROFILES"
# The settings this system's own surfaces expose. A fingerprint that leaves
# one of them unreadable is incomplete, and a blocked record names them when
# a read fails outright.
REQUIRED_SETTINGS = ("retention_profile", "ranking_environment")
# The bank the retention read goes through. The run drives one bank per
# session and never stores an override for any of them, so they resolve bank
# override, deployment default, builtin. A probe bank with no stored override
# resolves through that same chain; the `default` bank could carry its own
# override that none of the run's banks would see.
_PROBE_BANK = "clio-bench-fingerprint"


class ClioAdapterError(RuntimeError):
    """Raised when the clio binary fails or returns unusable output."""


class ClioAdapter(MemoryBenchmarkAdapter):
    """Benchmark adapter that shells out to the clio command-line binary."""

    def __init__(
        self,
        *,
        binary: str = "clio",
        data_dir: str | None = None,
        bank_prefix: str = "clio-bench",
        actor: str = "harness",
        category: str = "output_constraint",
        tokenizer: ReferenceTokenizer,
        limit: int = 10,
        timeout: int = 600,
        extra_env: dict[str, str] | None = None,
    ) -> None:
        self._binary = binary
        self._data_dir = data_dir
        self._bank_prefix = bank_prefix
        self._actor = actor
        self._category = category
        self._tokenizer = tokenizer
        self._limit = limit
        self._timeout = timeout
        self._extra_env = dict(extra_env) if extra_env else {}
        self._bank = ""

    # -- helpers ---------------------------------------------------------

    def _bank_for(self, session_id: str) -> str:
        """Return the isolated memory bank name for one session."""
        return f"{self._bank_prefix}-{session_id}"

    def _env(self) -> dict[str, str]:
        """Build the subprocess environment: os.environ plus overrides."""
        env = dict(os.environ)
        if self._data_dir is not None:
            env["CLIO_DATA_DIR"] = self._data_dir
        env.update(self._extra_env)
        return env

    def _run(self, args: list[str], *, allow_failure: bool = False) -> dict:
        """Run clio and return the last JSON object printed on stdout.

        When ``allow_failure`` is set and the process exits non-zero but still
        printed a JSON object (for example an admission rejection), that object
        is returned instead of raising. A crash with no JSON still raises.
        """
        try:
            proc = subprocess.run(
                [self._binary, *args],
                capture_output=True,
                text=True,
                timeout=self._timeout,
                env=self._env(),
            )
        except subprocess.TimeoutExpired as exc:
            raise ClioAdapterError(
                f"clio {' '.join(args)} timed out after {self._timeout}s"
            ) from exc
        except OSError as exc:
            raise ClioAdapterError(
                f"could not run clio binary {self._binary!r}: {exc}"
            ) from exc

        stderr = (proc.stderr or "").strip()
        payload = self._json_document(proc.stdout or "")
        if payload is None:
            # On a refused write the product prints the result object to stderr.
            payload = self._json_document(proc.stderr or "")
        if proc.returncode != 0:
            if allow_failure and payload is not None:
                return payload
            raise ClioAdapterError(
                f"clio {' '.join(args)} exited {proc.returncode}: "
                f"{stderr[:_STDERR_LIMIT]}"
            )
        if payload is None:
            raise ClioAdapterError(
                f"clio {' '.join(args)} produced no JSON object on stdout: "
                f"{stderr[:_STDERR_LIMIT]}"
            )
        return payload

    @staticmethod
    def _last_json_object(stdout: str) -> dict | None:
        """Return the last stdout line that parses to a JSON object, if any."""
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
        return None

    @classmethod
    def _json_document(cls, stdout: str) -> dict | None:
        """Return a JSON object from stdout, one-line or pretty-printed.

        Some commands (`retention get`) pretty-print their payload over many
        lines, so a line-by-line scan alone would miss it. The whole-document
        parse is tried first and the line scan stays as the fallback for output
        that prints progress noise before a one-line payload.
        """
        stripped = stdout.strip()
        if stripped:
            try:
                payload = json.loads(stripped)
            except json.JSONDecodeError:
                payload = None
            if isinstance(payload, dict):
                return payload
        return cls._last_json_object(stdout)

    # -- interface -------------------------------------------------------

    def reset(self, session_id: str) -> None:
        """Select the session's bank and clear its on-disk database."""
        self._bank = self._bank_for(session_id)
        if self._data_dir is not None:
            db_path = os.path.join(self._data_dir, "clio.db")
            if os.path.exists(db_path):
                os.remove(db_path)
            os.makedirs(self._data_dir, exist_ok=True)

    def ingest(self, session_id: str, turn: DialogueTurn) -> IngestResult:
        """Store one dialogue turn through ``clio remember``.

        The product's admission gate can reject a turn; that is a normal,
        recorded outcome (``ok=False``), not a harness failure.
        """
        bank = self._bank_for(session_id)
        start = time.monotonic()
        payload = self._run(
            [
                "remember",
                turn.text,
                "--category",
                self._category,
                "--bank",
                bank,
                "--actor",
                self._actor,
                "--output",
                "json",
            ],
            allow_failure=True,
        )
        latency_ms = (time.monotonic() - start) * 1000.0
        ok = bool(payload.get("ok"))
        error = None
        if not ok:
            error = str(
                payload.get("message") or payload.get("code") or "ingest rejected"
            )
        return IngestResult(
            ok=ok,
            latency_ms=latency_ms,
            tokens_processed=self._tokenizer.count(turn.text),
            error=error,
        )

    def retrieve(
        self, session_id: str, query: str, token_budget: int
    ) -> RetrievalResult:
        """Recall hits for a query and pack them into the token budget."""
        bank = self._bank_for(session_id)
        start = time.monotonic()
        payload = self._run(
            [
                "recall",
                query,
                "--bank",
                bank,
                "--limit",
                str(self._limit),
                "--output",
                "json",
            ]
        )
        latency_ms = (time.monotonic() - start) * 1000.0

        hits: list[tuple[str, str]] = []
        for hit in payload.get("hits") or []:
            if not isinstance(hit, dict):
                continue
            text = hit.get("gist") or hit.get("snapshot") or ""
            item_id = hit.get("item_id", "")
            hits.append((item_id, f"[{item_id}] {text}"))

        context, used, item_ids = pack_context(hits, token_budget, self._tokenizer)
        return RetrievalResult(
            context=context,
            latency_ms=latency_ms,
            tokens_retrieved=used,
            item_ids=item_ids,
        )

    def healthcheck(self) -> bool:
        """Return True when ``clio health`` reports readiness."""
        try:
            payload = self._run(["health", "--output", "json"])
        except ClioAdapterError:
            return False
        return payload.get("ok") is True

    # -- configuration fingerprint ---------------------------------------

    def fingerprint(self) -> Fingerprint:
        """Read Clio's behaviour-changing settings before any probe runs.

        The retention profile and the ranking environment are the two fields a
        run must read, so their failure aborts the run. The trigger state and
        the engine profile have no read surface in this build; both are
        recorded with the surfaces tried rather than omitted.
        """
        return Fingerprint(
            system="clio",
            version=self._read_version(),
            settings={
                "retention_profile": self._read_retention_profile(),
                "ranking_environment": self._read_ranking_environment(),
                "trigger_state": self._probe_trigger_state(),
                "engine_profile": self._probe_engine_profile(),
            },
            required=REQUIRED_SETTINGS,
        )

    def _read_version(self) -> str:
        try:
            payload = self._run(["health", "--output", "json"])
        except ClioAdapterError as exc:
            raise FingerprintError(f"clio health unreadable: {exc}") from exc
        version = payload.get("version")
        if not isinstance(version, str) or not version:
            raise FingerprintError("clio health returned no version")
        return version

    def _read_ranking_environment(self) -> SettingRecord:
        try:
            payload = self._run(["ranking", "get", "--output", "json"])
        except ClioAdapterError as exc:
            raise FingerprintError(
                f"clio ranking environment unreadable: {exc}"
            ) from exc
        return read_setting(
            value=payload, surface="clio ranking get --output json"
        )

    def _read_retention_profile(self) -> SettingRecord:
        """Read the profile through a probe bank the run resolves the same way.

        The run's per-session banks carry no stored override, so they resolve
        through the deployment default or the builtin profile. A probe bank
        with no stored override resolves through that same chain; the
        `default` bank could carry its own override the run would never see.
        """
        store, surface = self._retention_store()
        try:
            payload = self._run(
                ["retention", "get", "--bank", _PROBE_BANK, "--profiles", store]
            )
        except ClioAdapterError as exc:
            raise FingerprintError(
                f"clio retention profile unreadable: {exc}"
            ) from exc
        return read_setting(value=payload, surface=surface)

    def _retention_store(self) -> tuple[str, str]:
        """Resolve the retention store the way the run's subprocesses see it.

        The adapter passes its environment through to every command. When a
        store is configured it is read where it is; when none is, a pinned path
        that holds no store resolves to the builtin profile, which is the
        profile such a run actually uses. The surface string names the env var
        and never its value, so a configured host path cannot leak into the
        report.
        """
        configured = self._extra_env.get(_RETENTION_ENV) or os.environ.get(
            _RETENTION_ENV
        )
        if configured:
            return (
                configured,
                f"clio retention get --bank {_PROBE_BANK} "
                f"(store named by {_RETENTION_ENV})",
            )
        if self._data_dir is not None:
            return (
                os.path.join(self._data_dir, "retention-profiles.json"),
                f"clio retention get --bank {_PROBE_BANK} (no store configured; "
                "builtin profile at the pinned benchmark data directory)",
            )
        return (
            os.path.join(
                tempfile.gettempdir(), f"clio-bench-retention-{os.getpid()}.json"
            ),
            f"clio retention get --bank {_PROBE_BANK} (no store configured; "
            "builtin profile)",
        )

    def _probe_trigger_state(self) -> SettingRecord:
        """Record the trigger state as unreadable, with the probe that shows it."""
        try:
            self._run(["maintenance", "status", "--output", "json"])
            detail = (
                "clio maintenance status --output json (index pipeline fields "
                "only; no trigger state field exists yet)"
            )
        except ClioAdapterError as exc:
            detail = f"clio maintenance status --output json (failed: {exc})"
        return unreadable_setting(tried=(detail,))

    def _probe_engine_profile(self) -> SettingRecord:
        """Read an engine profile when a surface answers, else record the miss."""
        try:
            payload = self._run(
                ["config", "get", "engine_profile"], allow_failure=True
            )
        except ClioAdapterError as exc:
            return unreadable_setting(
                tried=(f"clio config get engine_profile (failed: {exc})",)
            )
        if payload.get("ok") is True:
            return read_setting(
                value=payload.get("value"),
                surface="clio config get engine_profile",
            )
        reason = payload.get("message") or payload.get("code") or "no surface"
        return unreadable_setting(
            tried=(f"clio config get engine_profile ({reason})",)
        )
