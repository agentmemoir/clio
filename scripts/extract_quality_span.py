"""Field-aware span grounding for entity, number, and date snapshot values."""

from __future__ import annotations

import math
import re
import unicodedata
from datetime import date, datetime
from typing import Any, Iterable, Mapping, Sequence

from extract_quality_core import FieldSpec, VerifyConfig, VerifyReport

_MONTH_NAME = (
    r"(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|"
    r"Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|"
    r"Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)"
)
_MONTH_DATE_RE = re.compile(
    rf"(?i)(?:{_MONTH_NAME}\s+\d{{1,2}}(?:,)?\s+\d{{4}}|"
    rf"\d{{1,2}}\s+{_MONTH_NAME}\s+\d{{4}})"
)


def nfc(value: str) -> str:
    return unicodedata.normalize("NFC", value)


def json_path(root: Any, path: str) -> Any | None:
    current = root
    for part in path.split("."):
        if not part:
            continue
        if isinstance(current, Mapping):
            if part not in current:
                return None
            current = current[part]
        elif isinstance(current, list) and part.isdigit():
            index = int(part)
            if index >= len(current):
                return None
            current = current[index]
        else:
            return None
    return current


def _entity_span(source: str, value: Any) -> tuple[int, int]:
    if not isinstance(value, str):
        raise ValueError("entity field must be a string")
    needle = nfc(value).strip()
    if not needle:
        raise ValueError("empty entity value")
    start = source.find(needle)
    if start < 0:
        raise ValueError("entity not found as contiguous source span")
    return start, start + len(needle)


def _parse_number(raw: str, comma_grouping: bool) -> float | None:
    text = raw.strip()
    for currency in "$€£¥":
        text = text.replace(currency, "")
    text = text.replace(" ", "")
    if not text:
        return None
    if comma_grouping:
        text = text.replace(",", "")
    else:
        text = text.replace(".", "").replace(",", ".")
    try:
        value = float(text)
    except ValueError:
        return None
    return value if math.isfinite(value) else None


def _number_span(source: str, value: Any, config: VerifyConfig) -> tuple[int, int]:
    if isinstance(value, bool):
        raise ValueError("number field must be a number or numeric string")
    if isinstance(value, (int, float)):
        expected = float(value)
    elif isinstance(value, str):
        expected = _parse_number(value, True)
        if expected is None:
            raise ValueError("number field string did not parse")
    else:
        raise ValueError("number field must be a number or numeric string")
    if not math.isfinite(expected):
        raise ValueError("number field is not finite")

    chars = list(source)
    index = 0
    while index < len(chars):
        char = chars[index]
        if not (char.isascii() and char.isdigit()) and char not in "$€£¥+-":
            index += 1
            continue
        end = index
        while end < len(chars) and chars[end] in "$€£¥":
            end += 1
        if end < len(chars) and chars[end] in "+-":
            end += 1
        while end < len(chars) and chars[end] in "$€£¥":
            end += 1
        digit_start = end
        while end < len(chars) and (chars[end].isascii() and chars[end].isdigit() or chars[end] in ",."):
            end += 1
        if end > digit_start:
            parsed = _parse_number("".join(chars[index:end]), config.comma_grouping)
            if parsed is not None and abs(parsed - expected) <= config.number_tolerance:
                return index, end
        # Match the verifier's fail-closed scan: retry at the next character
        # after a token that did not parse or did not equal the value.
        index += 1
    raise ValueError("no locatable number span parses to snapshot value")


def _date_value(value: Any) -> tuple[date, tuple[int, int, int] | None]:
    if not isinstance(value, str) or not value.strip():
        raise ValueError("date field must be a non-empty string")
    text = value.strip()
    # Keep the snapshot grammar aligned with the production verifier: the two
    # accepted date-time separators are ``T`` and a single space in the value;
    # source tokenization below is intentionally stricter for space-separated
    # source spans (the production token alphabet does not contain spaces).
    for format_string in ("%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"):
        try:
            parsed = datetime.strptime(text, format_string)
            return parsed.date(), (parsed.hour, parsed.minute, parsed.second)
        except ValueError:
            continue
    try:
        return datetime.strptime(text, "%Y-%m-%d").date(), None
    except ValueError as exc:
        raise ValueError("snapshot date must be ISO date or date-time") from exc


def _numeric_date(token: str) -> tuple[int, int, int] | None:
    separator = "/" if "/" in token else "." if token.count(".") >= 2 and "-" not in token else ""
    if not separator:
        return None
    parts = token.split(separator)
    if len(parts) != 3:
        return None
    try:
        first, second, year = (int(part) for part in parts)
    except ValueError:
        return None
    if year < 100:
        return None
    return first, second, year


def _safe_date(year: int, month: int, day: int) -> date | None:
    try:
        return date(year, month, day)
    except ValueError:
        return None


def _try_date_token(
    token: str,
    expected: tuple[date, tuple[int, int, int] | None],
    config: VerifyConfig,
) -> tuple[bool, bool]:
    """Return ``(matched, ambiguous)`` for one production-shaped token."""

    expected_date, expected_time = expected
    parsed_date: date | None = None
    parsed_time: tuple[int, int, int] | None = None
    try:
        if "T" in token:
            parsed = datetime.strptime(token, "%Y-%m-%dT%H:%M:%S")
            parsed_date = parsed.date()
            parsed_time = (parsed.hour, parsed.minute, parsed.second)
        else:
            parsed_date = datetime.strptime(token, "%Y-%m-%d").date()
    except ValueError:
        numeric = _numeric_date(token)
        if numeric is None:
            return False, False
        first, second, year = numeric
        # A component greater than twelve identifies the other component as
        # the day, so only the valid calendar interpretation is considered.
        if first > 12 and second <= 12:
            parsed_date = _safe_date(year, second, first)
        elif second > 12 and first <= 12:
            parsed_date = _safe_date(year, first, second)
        elif first <= 12 and second <= 12:
            day_first = _safe_date(year, second, first)
            month_first = _safe_date(year, first, second)
            possible = [
                item for item in (day_first, month_first) if item is not None
            ]
            if not possible:
                return False, False
            matches = expected_time is None and expected_date in possible
            if config.day_first is None:
                # Rust's verifier reports ambiguity only when both calendar
                # interpretations exist and one could match the snapshot.
                return False, len(possible) == 2 and matches
            parsed_date = day_first if config.day_first else month_first
        else:
            return False, False

    if parsed_date != expected_date:
        return False, False
    # A date-only snapshot can ground against a source date-time, but a
    # snapshot carrying a time requires the same time in the source.
    if expected_time is not None and parsed_time != expected_time:
        return False, False
    return True, False


def _month_date(token: str) -> date | None:
    normalized = re.sub(r"\s+", " ", token.strip()).replace(" ,", ",")
    for format_string in ("%b %d, %Y", "%B %d, %Y", "%d %b %Y", "%d %B %Y"):
        try:
            return datetime.strptime(normalized, format_string).date()
        except ValueError:
            continue
    return None


def _date_span(source: str, value: Any, config: VerifyConfig) -> tuple[int, int]:
    expected_date, expected_time = _date_value(value)
    for match in _MONTH_DATE_RE.finditer(source):
        parsed = _month_date(match.group(0))
        if parsed == expected_date and expected_time is None:
            return match.start(), match.end()
    expected = (expected_date, expected_time)
    chars = list(source)
    saw_ambiguous = False
    index = 0
    while index < len(chars):
        if chars[index].isascii() and chars[index].isdigit():
            end = index
            while end < len(chars) and (
                (chars[end].isascii() and chars[end].isdigit()) or chars[end] in "-/.:T"
            ):
                end += 1
            # Production tokenization requires a date-like token and does not
            # include spaces in its alphabet.  A space therefore terminates
            # the token, just as it does in the Rust verifier.  Punctuation is
            # deliberately not trimmed here: the reference follows the
            # authoritative scanner exactly.
            if end > index + 4:
                token = "".join(chars[index:end])
                matched, ambiguous = _try_date_token(token, expected, config)
                if matched:
                    return index, end
                saw_ambiguous = saw_ambiguous or ambiguous
                index = end
            else:
                index += 1
        else:
            index += 1
    if saw_ambiguous:
        raise ValueError("ambiguous date without day_first policy")
    raise ValueError("no locatable date span parses to snapshot value")


def _iter_snapshot_leaves(value: Any, path: str = "") -> Iterable[tuple[str, Any]]:
    if isinstance(value, Mapping):
        for raw_key, item in value.items():
            key = str(raw_key)
            child = f"{path}.{key}" if path else key
            yield from _iter_snapshot_leaves(item, child)
    elif isinstance(value, (list, tuple)):
        for index, item in enumerate(value):
            child = f"{path}.{index}" if path else str(index)
            yield from _iter_snapshot_leaves(item, child)
    else:
        yield path, value


def _is_empty_snapshot_leaf(value: Any) -> bool:
    return value is None or (isinstance(value, str) and not value.strip())


def verify_snapshot(
    source_text: str,
    snapshot: Any,
    fields: Sequence[FieldSpec],
    config: VerifyConfig | None = None,
) -> VerifyReport:
    config = config or VerifyConfig()
    source = nfc(source_text)
    if not isinstance(snapshot, Mapping):
        return VerifyReport(False, [], [{"field": "<snapshot>", "reason": "snapshot is not an object"}])
    spans: list[dict[str, Any]] = []
    failures: list[dict[str, str]] = []
    for spec in fields:
        value = json_path(snapshot, spec.path)
        missing = value is None or value == "" or (isinstance(value, str) and not value.strip())
        if missing:
            if spec.required:
                failures.append({"field": spec.path, "reason": "missing required extractive field"})
            continue
        try:
            if spec.kind == "entity":
                start, end = _entity_span(source, value)
            elif spec.kind == "number":
                start, end = _number_span(source, value, config)
            else:
                start, end = _date_span(source, value, config)
        except ValueError as exc:
            failures.append({"field": spec.path, "reason": str(exc)})
        else:
            spans.append({"field": spec.path, "start": start, "end": end})

    # A declared field has a typed extraction policy.  There is no safe typed
    # policy for an unlisted leaf, so reject every non-empty one fail-closed
    # instead of allowing schema-shaped metadata to bypass verification.
    # Snapshot keys are caller-controlled, so the failure names the walk
    # position only; the key and path are never echoed (see verify.rs).
    declared_paths = {spec.path for spec in fields}
    unlisted = 0
    for path, value in _iter_snapshot_leaves(snapshot):
        if path in declared_paths or _is_empty_snapshot_leaf(value):
            continue
        failures.append(
            {
                "field": f"<unlisted leaf #{unlisted}>",
                "reason": "unlisted snapshot leaf rejected",
            }
        )
        unlisted += 1
    return VerifyReport(not failures, spans, failures)
