"""The plugin API, exercised in-process through ``lsdocs_plugin.testing``."""

import asyncio
import threading
import time

import pytest

from lsdocs_plugin import PROTOCOL_VERSION, Plugin, testing


def text(value):
    return {"type": "text", "value": value}


def make_plugin():
    plugin = Plugin("sample", "1.2.3")

    @plugin.parse(extensions=["txt"])
    def parse(params, ctx):
        # Bare document shorthand, without `source`.
        return {"children": [text(line) for line in params["text"].splitlines()]}

    @plugin.collect
    def collect(params, ctx):
        return len(params["document"]["children"])

    @plugin.transform(nodes=["text"])
    def apply(params, ctx):
        ctx.info(f"applying with {params['collected']!r}")
        doc = params["document"]
        for node in doc["children"]:
            node["value"] = node["value"].upper()
        return {"document": doc, "diagnostics": [{"severity": "hint", "message": "shouted"}]}

    @plugin.render(formats=["txt", "bin"])
    def render(params, ctx):
        joined = "\n".join(n["value"] for n in params["document"]["children"])
        if params["format"] == "bin":
            return {"output": joined.encode() + b"\x00\xff", "extension": "dat"}
        return f"{ctx.site.get('title')}: {joined}"

    return plugin


def doc(*lines):
    return {"source": "a.txt", "children": [text(line) for line in lines]}


def test_capabilities_follow_the_handlers():
    caps = make_plugin().capabilities
    assert caps == {
        "parse": {"extensions": ["txt"]},
        "transform": {"nodes": ["text"], "collect": True},
        "render": {"formats": ["txt", "bin"]},
        "max_concurrency": 1,
    }
    assert Plugin("empty").capabilities == {"max_concurrency": 1}


def test_initialize():
    result, _ = testing.call(make_plugin(), "initialize")
    assert result["protocol_version"] == PROTOCOL_VERSION
    assert result["plugin"] == {"name": "sample", "version": "1.2.3"}


def test_parse_shorthand_fills_in_the_document():
    result, events = testing.parse(make_plugin(), "notes/a.txt", "one\ntwo")
    assert result["document"]["source"] == "notes/a.txt"
    assert result["document"]["children"] == [text("one"), text("two")]
    assert events == []


def test_collect_and_apply():
    plugin = make_plugin()
    collected, _ = testing.collect(plugin, doc("a", "b"))
    assert collected == {"data": 2}

    result, events = testing.apply(plugin, doc("a", "b"), {"a.txt": 2})
    assert [n["value"] for n in result["document"]["children"]] == ["A", "B"]
    assert result["diagnostics"] == [{"severity": "hint", "message": "shouted"}]
    assert events == [{"kind": "log", "level": "info", "message": "applying with {'a.txt': 2}"}]


def test_render_strings_and_bytes():
    plugin = make_plugin()
    result, _ = testing.render(plugin, doc("hi"), "txt", site={"title": "Site"})
    assert result == {"output": b"Site: hi", "extension": "txt"}

    result, _ = testing.render(plugin, doc("hi"), "bin")
    assert result == {"output": b"hi\x00\xff", "extension": "dat"}


def test_render_site_returns_site_files():
    plugin = make_plugin()
    assert "site" not in plugin.capabilities["render"]

    @plugin.render_site
    async def site_files(params, ctx):
        paths = [page["path"] for page in params["pages"]]
        # A bare list of files; outputs may be str or bytes.
        return [
            {"path": "index.txt", "output": "\n".join(paths)},
            {"path": "raw.bin", "output": b"\x00\xff"},
        ]

    assert plugin.capabilities["render"]["site"] is True
    pages = [{"source": "a.txt", "path": "a.txt"}, {"source": "b.txt", "path": "b.txt"}]
    result, _ = testing.render_site(plugin, pages, "txt")
    assert result == {
        "files": [
            {"path": "index.txt", "output": b"a.txt\nb.txt"},
            {"path": "raw.bin", "output": b"\x00\xff"},
        ]
    }


def test_context_options_progress_and_diagnostics():
    plugin = Plugin("ctx")

    @plugin.transform()
    def apply(params, ctx):
        ctx.progress("halfway", 50)
        ctx.diagnostic({"severity": "warning", "message": "careful", "code": "x"})
        ctx.log("debug", f"depth={ctx.options['depth']}")
        return params["document"]

    _, events = testing.apply(plugin, doc(), options={"depth": 2})
    assert events == [
        {"kind": "progress", "message": "halfway", "percentage": 50},
        {"kind": "diagnostics", "diagnostics": [{"severity": "warning", "message": "careful", "code": "x"}]},
        {"kind": "log", "level": "debug", "message": "depth=2"},
    ]


def test_exceptions_carry_their_traceback():
    plugin = Plugin("broken")

    @plugin.transform()
    def apply(params, ctx):
        raise ValueError("no good")

    with pytest.raises(RuntimeError) as err:
        testing.apply(plugin, doc())
    message = str(err.value)
    assert message.startswith("ValueError: no good")
    assert "Traceback (most recent call last):" in message
    assert "in apply" in message


def test_invalid_results_are_reported():
    plugin = Plugin("bad")

    @plugin.transform()
    def apply(params, ctx):
        return {"document": "not a document"}

    with pytest.raises(RuntimeError, match="invalid transform/apply result"):
        testing.apply(plugin, doc())


def test_unsupported_values_are_rejected():
    plugin = Plugin("bad")

    @plugin.transform()
    def apply(params, ctx):
        return {"document": object()}

    with pytest.raises(RuntimeError, match="cannot send a value of type object"):
        testing.apply(plugin, doc())


def test_bad_log_level():
    plugin = Plugin("bad")

    @plugin.transform()
    def apply(params, ctx):
        ctx.log("loud", "x")

    with pytest.raises(RuntimeError, match='unknown log level "loud"'):
        testing.apply(plugin, doc())


# --- async def handlers ----------------------------------------------------


def test_async_handlers_in_every_phase():
    plugin = Plugin("async")

    @plugin.parse(extensions=["txt"])
    async def parse(params, ctx):
        await asyncio.sleep(0)
        return {"children": [text(params["text"])]}

    @plugin.collect
    async def collect(params, ctx):
        await asyncio.sleep(0)
        return "collected"

    @plugin.transform()
    async def apply(params, ctx):
        await asyncio.sleep(0.01)
        ctx.info("from a coroutine")
        return params["document"]

    @plugin.render(formats=["txt"])
    async def render(params, ctx):
        return "rendered"

    assert testing.parse(plugin, "a.txt", "hi")[0]["document"]["children"] == [text("hi")]
    assert testing.collect(plugin, doc())[0] == {"data": "collected"}
    result, events = testing.apply(plugin, doc("x"))
    assert result["document"]["children"] == [text("x")]
    assert events == [{"kind": "log", "level": "info", "message": "from a coroutine"}]
    assert testing.render(plugin, doc(), "txt")[0]["output"] == b"rendered"


def test_one_loop_is_shared_across_calls():
    plugin = Plugin("loop")
    loops = []
    state = {}

    @plugin.transform()
    async def apply(params, ctx):
        loops.append(asyncio.get_running_loop())
        # A loop-bound object created in one call must work in the next.
        lock = state.setdefault("lock", asyncio.Lock())
        async with lock:
            pass
        return params["document"]

    testing.apply(plugin, doc())
    testing.apply(plugin, doc())
    assert len(loops) == 2 and loops[0] is loops[1]


def test_sync_handlers_returning_awaitables_are_awaited():
    plugin = Plugin("awaitable")

    async def later(document):
        return document

    @plugin.transform()
    def apply(params, ctx):
        return later(params["document"])

    assert testing.apply(plugin, doc("y"))[0]["document"]["children"] == [text("y")]


def test_async_exceptions_carry_their_traceback():
    plugin = Plugin("broken-async")

    @plugin.transform()
    async def apply(params, ctx):
        await asyncio.sleep(0)
        raise KeyError("missing")

    with pytest.raises(RuntimeError) as err:
        testing.apply(plugin, doc())
    message = str(err.value)
    assert message.startswith("KeyError: 'missing'")
    assert "in apply" in message


def test_async_calls_overlap_and_release_the_gil():
    plugin = Plugin("overlap", max_concurrency=2)

    @plugin.transform()
    async def apply(params, ctx):
        await asyncio.sleep(0.3)
        return params["document"]

    results = []
    threads = [threading.Thread(target=lambda: results.append(testing.apply(plugin, doc()))) for _ in range(2)]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    elapsed = time.perf_counter() - start
    assert len(results) == 2
    assert elapsed < 0.55, f"two 0.3 s handlers took {elapsed:.2f} s; they did not overlap"
