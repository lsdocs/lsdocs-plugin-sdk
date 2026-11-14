"""Runs ``async def`` handlers on one persistent event loop.

Handlers are called from Rust on worker threads. A coroutine handler is
submitted to a single asyncio loop running on a daemon thread, and the
worker waits for its result (releasing the GIL while it waits). Sharing one
loop for the life of the plugin means loop-bound objects created in one call,
such as client sessions or locks, keep working in the next. With
``max_concurrency`` above 1, several async handlers can be in flight at once.
"""

from __future__ import annotations

import asyncio
import functools
import inspect
import threading
from collections.abc import Awaitable, Callable
from typing import Any, TypeVar

T = TypeVar("T")

_lock = threading.Lock()
_loop: asyncio.AbstractEventLoop | None = None


def loop() -> asyncio.AbstractEventLoop:
    """The shared event loop, started on first use."""
    global _loop
    with _lock:
        if _loop is None:
            new_loop = asyncio.new_event_loop()
            started = threading.Event()

            def run() -> None:
                asyncio.set_event_loop(new_loop)
                new_loop.call_soon(started.set)
                new_loop.run_forever()

            threading.Thread(target=run, name="lsdocs-asyncio", daemon=True).start()
            started.wait()
            _loop = new_loop
        return _loop


async def _await(awaitable: Awaitable[T]) -> T:
    return await awaitable


def run(awaitable: Awaitable[T]) -> T:
    """Runs ``awaitable`` on the shared loop and waits for its result."""
    if threading.current_thread().name == "lsdocs-asyncio":
        raise RuntimeError("cannot wait for a handler from inside the event loop")
    return asyncio.run_coroutine_threadsafe(_await(awaitable), loop()).result()


def adapt(func: Callable[..., Any]) -> Callable[..., Any]:
    """Wraps a handler so that coroutine results are awaited on the shared
    loop; plain results pass through."""

    @functools.wraps(func)
    def call(*args: Any) -> Any:
        result = func(*args)
        if inspect.isawaitable(result):
            return run(result)
        return result

    return call
