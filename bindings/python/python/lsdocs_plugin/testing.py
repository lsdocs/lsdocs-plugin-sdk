"""Run a plugin's handlers in-process, as the host would, for tests.

Each function returns ``(result, events)``: the handler's result normalized
to the full protocol shape, and the events it sent (log lines, progress,
diagnostics) as dicts with a ``kind`` key::

    from lsdocs_plugin import testing

    result, events = testing.parse(plugin, "a.txt", "hello")
    assert result["document"]["children"][0]["value"] == "hello"
"""

from __future__ import annotations

from typing import Any

from . import Plugin, _native

Result = tuple[dict[str, Any], list[dict[str, Any]]]


def call(
    plugin: Plugin,
    method: str,
    params: dict[str, Any] | None = None,
    *,
    site: dict[str, Any] | None = None,
    options: Any = None,
) -> Result:
    """Calls ``method`` (``"parse"``, ``"parse/transform/each"``,
    ``"render/transform/each"``, ``"render"``, ``"render/transform/all"`` or
    ``"initialize"``) with ``params``."""
    return _native.call(plugin._native, method, params, site, options)


def parse(plugin: Plugin, source: str, text: str, **kwargs: Any) -> Result:
    """Parses ``text`` as the file ``source``."""
    return call(plugin, "parse", {"source": source, "text": text}, **kwargs)


def read(plugin: Plugin, document: dict[str, Any], **kwargs: Any) -> Result:
    """Runs ``parse/transform/each`` on ``document``."""
    return call(plugin, "parse/transform/each", {"document": document}, **kwargs)


def apply(plugin: Plugin, document: dict[str, Any], collected: Any = None, **kwargs: Any) -> Result:
    """Runs the transformer on ``document`` with ``collected`` data."""
    return call(plugin, "render/transform/each", {"document": document, "collected": collected}, **kwargs)


def render(plugin: Plugin, document: dict[str, Any], format: str = "html", **kwargs: Any) -> Result:
    """Renders ``document`` to ``format``. ``output`` is returned as bytes."""
    return call(plugin, "render", {"document": document, "format": format}, **kwargs)


def render_all(
    plugin: Plugin, pages: list[dict[str, Any]], format: str = "html", **kwargs: Any
) -> Result:
    """Runs ``render/transform/all`` for ``pages`` (dicts with ``source``, ``path``
    and optional ``metadata``). Each file's ``output`` is returned as bytes."""
    return call(plugin, "render/transform/all", {"format": format, "pages": pages}, **kwargs)
