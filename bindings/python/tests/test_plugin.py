"""The plugin API, exercised in-process through ``lsdocs_plugin.testing``."""

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
