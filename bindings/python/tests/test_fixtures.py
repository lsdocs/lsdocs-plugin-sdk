"""Every golden fixture decodes from Python's msgpack to its JSON twin.

This is the Python counterpart of ``nur test fixtures`` (nushell): a reader
outside Rust and serde that would expose structs encoded as positional
arrays, or enums in an unexpected shape.
"""

import json
from pathlib import Path

import msgpack
import pytest

FIXTURES = Path(__file__).resolve().parents[4] / "fixtures"
FILES = sorted(FIXTURES.rglob("*.msgpack"))


def normalize(value):
    """MessagePack `bin` becomes a list of byte values, as serde writes it to JSON."""
    if isinstance(value, bytes):
        return list(value)
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in value.items()}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    return value


def test_fixtures_exist():
    assert FILES, f"no fixtures under {FIXTURES}; run `nur gen`"


@pytest.mark.parametrize("path", FILES, ids=lambda p: p.relative_to(FIXTURES).as_posix())
def test_fixture_matches_json_twin(path):
    decoded = msgpack.unpackb(path.read_bytes(), raw=False, strict_map_key=True)
    expected = json.loads(path.with_suffix(".json").read_text(encoding="utf-8"))
    assert normalize(decoded) == expected
