+++
title = "Configuration"
nav_order = 2
+++
# Configuring a site {#config}

> Settings live in `lsdocs.toml`, next to the `docs/` directory.

> [!NOTE]
> Unknown keys are an error[^strict], so typos are caught early.

## Options

| Key | Default | Meaning |
|---|:---:|---|
| `build.source_dir` | `docs` | Where pages are read from |
| `build.output_dir` | `site` | Where the site is written |

## Plugins

```toml
[[plugins]]
name = "markdown"
builtin = "parser-myst"
```

<details>
<summary>Raw HTML passes through</summary>

Content inside is still Markdown.

</details>

Escaping works: 1 < 2 && "quotes" stay safe.

[^strict]: Plugin settings are the plugins' own files, under `.config/`.
