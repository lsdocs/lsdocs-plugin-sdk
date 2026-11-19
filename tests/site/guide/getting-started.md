---
nav_order: 1
---
# Getting started

Install lsdocs, create a site, and build it. Back to [home](../index.md).

## Install

```sh
cargo install lsdocs
```

## Create a site

1. Run `lsdocs init my-docs`.
2. Edit *my-docs/docs/index.md*.
3. Build with **`lsdocs build`**.

## Build

The output goes to `site/`; [](configuration.md#options) says how to change
that.
Each Markdown page becomes an HTML page;
other files are copied as they are.

## Build

A second heading with the same title gets a unique id.
Building $n$ pages takes about $$O(n)$$ time.
