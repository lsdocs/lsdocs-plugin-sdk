# lsdocs-plugin

Write [lsdocs](../../ROADMAP.md) plugins in Python.

```python
from lsdocs_plugin import Plugin

plugin = Plugin("shout", "0.1.0")

@plugin.transform(nodes=["text"])
def shout(params, ctx):
    doc = params["document"]
    for node in doc["children"]:
        if node["type"] == "text":
            node["value"] = node["value"].upper()
    return doc

if __name__ == "__main__":
    plugin.serve()
```

Use it from `lsdocs.toml`:

```toml
[[plugins]]
name = "shout"
runtime = "python"
module = "shout"
```

`lsdocs_plugin.testing` runs handlers in-process for unit tests.
`examples/transform_toc.py` is a full plugin: a Python port of the
first-party table-of-contents transformer.

## Development

```sh
nur test python   # from the repository root: builds the extension, runs pytest
```
