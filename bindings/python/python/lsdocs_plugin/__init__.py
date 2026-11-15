"""Write lsdocs plugins in Python.

A plugin is a :class:`Plugin` with handlers registered by decorator. Each
handler receives the request params (a dict shaped like the protocol schema)
and a :class:`Context`, and returns the result::

    from lsdocs_plugin import Plugin

    plugin = Plugin("shout", "0.1.0")

    @plugin.transform(nodes=["text"])
    def shout(params, ctx):
        doc = params["document"]
        for node in doc["children"]:
            if node["type"] == "text":
                node["value"] = node["value"].upper()
        return doc  # the bare document is shorthand for {"document": doc}

    if __name__ == "__main__":
        plugin.serve()

Shorthand results: ``parse`` and ``transform`` handlers may return just the
document; ``collect`` may return any value (taken as ``data``); ``render`` may
return a ``str`` or ``bytes`` (taken as ``output``, with ``extension`` set to
the format); ``render_site`` may return a list of files.

Handlers may be ``async def``: coroutines run on one event loop shared by
all calls, so loop-bound objects (client sessions, locks) can be reused::

    @plugin.transform(nodes=["link"])
    async def check_links(params, ctx):
        async with session.head(url) as response: ...

Handlers run one at a time unless ``max_concurrency`` is raised, which lets
async handlers overlap. The protocol itself (framing, handshake,
cancellation, shutdown) is handled in Rust.
"""

from __future__ import annotations

import sys
from collections.abc import Callable, Iterable
from typing import Any, TypeVar

from . import _aio, _native
from ._native import PROTOCOL_VERSION, Context

__all__ = ["PROTOCOL_VERSION", "Context", "Handler", "Plugin"]

Handler = Callable[[dict[str, Any], Context], Any]
"""A handler: ``(params, ctx) -> result``, or an ``async def`` returning it."""

F = TypeVar("F", bound=Handler)


class Plugin:
    """An lsdocs plugin: a name, a version and its handlers."""

    def __init__(self, name: str, version: str = "0.0.0", *, max_concurrency: int = 1) -> None:
        self.name = name
        self.version = version
        self._native = _native.NativePlugin(name, version, max_concurrency)

    def parse(self, *, extensions: Iterable[str]) -> Callable[[F], F]:
        """Registers the parser for files with these extensions (no dot)."""
        exts = list(extensions)

        def register(func: F) -> F:
            self._native.set_parse(exts, _aio.adapt(func))
            return func

        return register

    def collect(self, func: F | None = None) -> Any:
        """Registers the collect pass, run on every document before
        :meth:`transform`. Usable as ``@plugin.collect`` or
        ``@plugin.collect()``."""

        def register(f: F) -> F:
            self._native.set_collect(_aio.adapt(f))
            return f

        return register(func) if func is not None else register

    def transform(self, *, nodes: Iterable[str] = ()) -> Callable[[F], F]:
        """Registers the transformer. ``nodes`` are the node kinds it looks
        at (e.g. ``["heading", "custom:toc"]``); documents without any of them
        are not sent. Empty means every document."""
        kinds = list(nodes)

        def register(func: F) -> F:
            self._native.set_apply(kinds, _aio.adapt(func))
            return func

        return register

    def render(self, *, formats: Iterable[str]) -> Callable[[F], F]:
        """Registers the renderer for these output formats."""
        fmts = list(formats)

        def register(func: F) -> F:
            self._native.set_render(fmts, _aio.adapt(func))
            return func

        return register

    def render_site(self, func: F) -> F:
        """Registers the ``render/site`` handler, called once after every
        page is rendered with the list of rendered pages
        (``{"format", "pages": [{"source", "path", "metadata"}]}``). It
        returns site-wide files: ``{"files": [{"path", "output"}]}`` or just
        the list, each ``output`` a ``str`` or ``bytes``. Only used with a
        renderer (:meth:`render`)."""
        self._native.set_render_site(_aio.adapt(func))
        return func

    @property
    def capabilities(self) -> dict[str, Any]:
        """The capabilities reported to the host."""
        return self._native.capabilities()

    def serve(self) -> None:
        """Runs the plugin until the host is done with it.

        Talks to the host over stdin/stdout, or over the pipe in
        ``LSDOCS_PIPE``. ``print()`` output goes to stderr from here on, so
        it cannot corrupt the protocol stream.
        """
        sys.stdout.flush()
        sys.stdout = sys.stderr
        _native.serve(self._native)
