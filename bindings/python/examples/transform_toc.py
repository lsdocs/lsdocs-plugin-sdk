"""A Python port of the first-party ``transform-toc`` plugin.

Collects each document's section tree and writes it to the document's
``toc`` metadata: ``[{id, title, level, children}]``. A first section with a
level 1 heading is the page title and is left out. With the setting
``inline = true`` it also inserts a nested list of links to the sections
into the document's first section, right after its title. Documents with
nothing to list are left alone. The setting ``depth`` (default 3) is the
deepest heading level listed.

Its output must match the Rust plugin exactly; see tests/test_toc_port.py.
"""

from __future__ import annotations

import tomllib
from typing import Any

from lsdocs_plugin import Context, Plugin

# Its settings are lsdocs' own plugin's: `.config/lsdocs/<name>/settings.toml`.
SETTINGS = "lsdocs/{name}/settings.toml"
plugin = Plugin("transform-toc-py", "0.1.0", config_files=[SETTINGS])

Node = dict[str, Any]


def plain_text(nodes: list[Node]) -> str:
    out = []
    for node in nodes:
        if node["type"] in ("text", "inline_code"):
            out.append(node["value"])
        elif node["type"] == "image":
            out.append(node.get("alt", ""))
        else:
            out.append(plain_text(node.get("children", [])))
    return "".join(out)


def entries(nodes: list[Node], depth: int) -> list[dict[str, Any]]:
    """The sections among ``nodes`` as ``{id, title, children}`` entries,
    leaving out sections whose title is deeper than ``depth``."""
    out = []
    for node in nodes:
        if node["type"] != "section" or node.get("id") is None:
            continue
        children = node.get("children", [])
        if not children or children[0]["type"] != "heading":
            continue
        heading = children[0]
        if heading["level"] > depth:
            continue
        out.append(
            {
                "id": node["id"],
                "title": plain_text(heading.get("children", [])),
                "level": heading["level"],
                "children": entries(children[1:], depth),
            }
        )
    return out


@plugin.read
def read(params: dict[str, Any], ctx: Context) -> Any:
    text = ctx.settings.get(SETTINGS.format(name=ctx.config["name"]))
    settings = tomllib.loads(text) if text else {}
    return {"sections": entries(params["document"]["children"], settings.get("depth", 3))}


def link_list(items: list[dict[str, Any]]) -> Node:
    """A list of links to ``items``, with each item's children nested inside
    it."""
    out = []
    for item in items:
        children: list[Node] = [
            {
                "type": "paragraph",
                "children": [
                    {
                        "type": "link",
                        "url": f"#{item['id']}",
                        "children": [{"type": "text", "value": item["title"]}],
                    }
                ],
            }
        ]
        if item["children"]:
            children.append(link_list(item["children"]))
        out.append({"type": "list_item", "children": children})
    return {"type": "list", "children": out}


@plugin.transform(nodes=["section"])
def apply(params: dict[str, Any], ctx: Context) -> Node:
    document = params["document"]
    collected = params.get("collected") or {}
    sections = (collected.get(document["source"]) or {}).get("sections", [])
    if sections:
        rest = sections[1:]
        first = sections[0]
        entries_ = (first["children"] if first["level"] == 1 else [first]) + rest
        if entries_:
            document.setdefault("metadata", {})["toc"] = entries_
        text = ctx.settings.get(SETTINGS.format(name=ctx.config["name"]))
        inline = (tomllib.loads(text) if text else {}).get("inline", False)
        items = first["children"] + rest
        title = next((n for n in document["children"] if n["type"] == "section"), None)
        if inline and items and title is not None:
            children = title["children"]
            at = 1 if children and children[0]["type"] == "heading" else 0
            children.insert(at, {"type": "custom", "kind": "toc", "children": [link_list(items)]})
    return document


if __name__ == "__main__":
    plugin.serve()
