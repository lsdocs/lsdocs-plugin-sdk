"""The Python port of transform-toc produces the same site as the Rust one.

Builds tests/site (at the repository root) with the builtin Rust transformer, then with
examples/transform_toc.py run out-of-process (over stdio and over a pipe),
and compares every output file. Needs the `lsdocs` binary: set LSDOCS_BIN
(`nur test python` does).
"""

import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SAMPLE_DOCS = ROOT / "tests" / "site"
PORT = Path(__file__).resolve().parents[1] / "examples" / "transform_toc.py"
LSDOCS = os.environ.get("LSDOCS_BIN")

pytestmark = pytest.mark.skipif(not LSDOCS, reason="LSDOCS_BIN is not set")


def toml_str(value):
    # Literal (single-quoted) strings keep Windows paths intact.
    return "'" + str(value) + "'"


def config(toc_plugin: str) -> str:
    return f"""
[site]
title = "lsdocs sample"

[build]
source_dir = {toml_str(SAMPLE_DOCS)}

[[plugins]]
name = "markdown"
builtin = "parser-myst"

[[plugins]]
name = "toc"
{toc_plugin}

[[plugins]]
name = "html"
builtin = "render-html"
"""


def build(tmp_path: Path, label: str, toc_plugin: str, env: dict[str, str] | None = None) -> dict[str, bytes]:
    site = tmp_path / label
    site.mkdir()
    (site / "lsdocs.toml").write_text(config(toc_plugin), encoding="utf-8")
    # Not the default depth (3), so the port must read its settings.
    settings = site / ".config" / "lsdocs" / "toc" / "settings.toml"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_bytes(b"depth = 2\ninline = true\n")
    proc = subprocess.run(
        [LSDOCS, "build", "--config", str(site / "lsdocs.toml")],
        capture_output=True,
        text=True,
        timeout=120,
        env={**os.environ, **(env or {})},
    )
    assert proc.returncode == 0, proc.stderr
    assert "warning" not in proc.stderr and "error" not in proc.stderr, proc.stderr
    out = site / "site"
    return {p.relative_to(out).as_posix(): p.read_bytes() for p in sorted(out.rglob("*")) if p.is_file()}


def python_toc(transport: str) -> str:
    command = f"command = [{toml_str(sys.executable)}, {toml_str(PORT)}]"
    return f'{command}\ntransport = "{transport}"'


def test_python_port_matches_rust(tmp_path):
    rust = build(tmp_path, "rust", 'builtin = "transform-toc"')
    assert b'<div class="lsd-toc">' in rust["index.html"]

    for transport in ("stdio", "pipe"):
        python = build(tmp_path, f"python-{transport}", python_toc(transport))
        assert python.keys() == rust.keys()
        for name in rust:
            assert python[name] == rust[name], f"{name} differs over {transport}"

    # `runtime = "python"` runs `python -m <module>`; LSDOCS_PYTHON picks
    # this interpreter and PYTHONPATH makes the example importable.
    module = build(
        tmp_path,
        "python-module",
        'runtime = "python"\nmodule = "transform_toc"',
        env={"LSDOCS_PYTHON": sys.executable, "PYTHONPATH": str(PORT.parent)},
    )
    assert module == rust


ASYNC_PLUGIN = '''
import asyncio
from lsdocs_plugin import Plugin

plugin = Plugin("shout-async", "0.1.0")

@plugin.transform(nodes=["text"])
async def shout(params, ctx):
    await asyncio.sleep(0)
    def walk(nodes):
        for node in nodes:
            if node["type"] == "text":
                node["value"] = node["value"].upper()
            walk(node.get("children", []))
    walk(params["document"]["children"])
    return params["document"]

plugin.serve()
'''


def test_async_plugin_builds_and_exits(tmp_path):
    script = tmp_path / "shout_async.py"
    script.write_text(ASYNC_PLUGIN, encoding="utf-8")
    site = build(tmp_path, "async", f"command = [{toml_str(sys.executable)}, {toml_str(script)}]")
    assert b"GETTING STARTED" in site["guide/getting-started.html"]
