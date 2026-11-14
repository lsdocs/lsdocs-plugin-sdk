"""A Python port of the first-party ``transform-toc`` plugin.

Collects each document's headings, then inserts a list of links to them
after the document's first heading. Documents with fewer than two headings
are left alone. Option ``depth`` (default 3) is the deepest level listed.

Its output must match the Rust plugin exactly; see tests/test_toc_port.py.
"""

from __future__ import annotations

from typing import Any

from lsdocs_plugin import Context, Plugin

plugin = Plugin("transform-toc-py", "0.1.0")

Node = dict[str, Any]


def descendants(nodes: list[Node]):
    """Every node, depth first, parents before children."""
    for node in nodes:
        yield node
        yield from descendants(node.get("children", []))


def plain_text(nodes: list[Node]) -> str:
    out = []
    for node in nodes:
        if node["type"] in ("text", "inline_code"):
            out.append(node["value"])
        else:
            out.append(plain_text(node.get("children", [])))
    return "".join(out)


@plugin.collect
def collect(params: dict[str, Any], ctx: Context) -> Any:
    options = ctx.options or {}
    depth = options.get("depth", 3)
    headings = [
        {"id": node["id"], "level": node["level"], "title": plain_text(node.get("children", []))}
        for node in descendants(params["document"]["children"])
        if node["type"] == "heading" and node.get("id") is not None and node["level"] <= depth
    ]
    return {"headings": headings}


def toc_node(entries: list[dict[str, Any]]) -> Node:
    items = [
        {
            "type": "list_item",
            "children": [
                {
                    "type": "paragraph",
                    "children": [
                        {
                            "type": "link",
                            "url": f"#{entry['id']}",
                            "children": [{"type": "text", "value": entry["title"]}],
                        }
                    ],
                }
            ],
        }
        for entry in entries
    ]
    return {"type": "custom", "kind": "toc", "children": [{"type": "list", "children": items}]}


@plugin.transform(nodes=["heading"])
def apply(params: dict[str, Any], ctx: Context) -> Node:
    document = params["document"]
    collected = params.get("collected") or {}
    entries = (collected.get(document["source"]) or {}).get("headings", [])
    if len(entries) >= 2:
        children = document["children"]
        first = next((i + 1 for i, n in enumerate(children) if n["type"] == "heading"), 0)
        children.insert(first, toc_node(entries[1:]))
    return document


if __name__ == "__main__":
    plugin.serve()
