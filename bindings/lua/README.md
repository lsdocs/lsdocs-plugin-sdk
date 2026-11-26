# lsdocs-plugin for Lua

Write [lsdocs] plugins in Lua 5.4 or LuaJIT. A plugin is a set of Lua
functions handling parse, transform and render requests; the protocol
(framing, handshake, cancellation, shutdown) runs in Rust, bound with
[mlua].

```lua
local lsdocs = require("lsdocs_plugin")

local plugin = lsdocs.plugin("shout", "0.1.0")

plugin:transform({ "text" }, function(params, ctx)
  local doc = params.document
  for _, node in ipairs(doc.children) do
    if node.type == "text" then
      node.value = node.value:upper()
    end
  end
  ctx:info("shouted " .. doc.source)
  return doc -- shorthand for { document = doc }
end)

plugin:serve()
```

The site config runs it like any plugin: `command = ["lua", "shout.lua"]`.

- `lsdocs_plugin.plugin(name, version, options)`: `options.config_files`
  lists the settings files the plugin reads, relative to `.config/`, with
  `{name}` for its name in the site config
  (`"lsdocs/{name}/settings.toml"`).
- Handlers, each `function(params, ctx)` returning the result:
  `plugin:parse(extensions, f)`, `plugin:read(f)`,
  `plugin:transform(nodes, f)`, `plugin:render(formats, f)`,
  `plugin:render_all(f)`. `params` is the request as the protocol schema
  shapes it. Shorthand results: `parse` and `transform` may return the
  document alone; `read` any value (taken as `data`); `render` a string
  (taken as `output`, with `extension` set to the format); `render_all` a
  list of files.
- `ctx.site` (`title`, `base_url`, `root`), `ctx.settings` (each config
  file mapped to its text; a missing one is `nil`), `ctx.config` (`dir`,
  `name`); `ctx:log(level, message)`, `ctx:debug/info/warn/error(message)`,
  `ctx:progress(message, percentage)`, `ctx:diagnostic(t)`.
- `plugin:capabilities()`, `plugin:serve()` (until the host is done),
  and for tests `lsdocs_plugin.call(plugin, method, params, options)`,
  which runs one handler in-process as the host would and returns
  `result, events` (`options`: `site`, `settings`, `config`). Render
  output comes back as a string.

## Values

Values are plain tables. Into Lua, JSON `null` is `nil` (the key is
absent). Out of Lua:

- A table with keys `1..n` is a list, other tables are maps with string
  keys. An empty table is an empty list, except as `metadata` (a map in
  the document tree); `lsdocs_plugin.object(t)` marks a table (or a new
  empty one) as a map.
- `lsdocs_plugin.null` is `null`.
- A number with no fraction is an integer (LuaJIT has no integers, and
  fields such as a heading's `level` must be integers).
- A string that is not UTF-8 is sent as bytes (binary `output`).

## Running

Handlers run one at a time on the thread that called `serve()`. The
events a handler sends (`ctx:log`, ...) reach the host when it returns.

`lsdocs-lua` (`src/bin`) is a `lua` with the module built in (embedded
Lua 5.4), for running Lua plugins without a Lua installed:
`cargo run -p lsdocs-plugin-lua --bin lsdocs-lua -- plugin.lua`.

## Building

`nur test lua` runs the specs (`spec/*_spec.lua`) in an embedded Lua 5.4,
and in LuaJIT except on Windows, then the toc port test
(`examples/transform_toc.lua` must build the same site as the Rust
`transform-toc`; it needs an `lsdocs` binary, LSDOCS_BIN). The module for
an external Lua builds with `cargo build -p lsdocs-plugin-lua --release
--no-default-features --features lua54,module` (or `luajit,module`); the
library (`liblsdocs_plugin_lua.so`) is installed as `lsdocs_plugin.so`. On
macOS, link with `-C link-arg=-undefined -C link-arg=dynamic_lookup`.

## Known limitations

- No async handlers: handlers are plain functions, one at a time.
- Context events reach the host when the handler returns.
- No syntax declarations or generated documents (as for Python and
  Node.js).
- No Windows modules yet. `luarocks install lsdocs-plugin` builds from source
  with cargo: the prebuilt modules attached to each GitHub release
  (`lsdocs-plugin-lua/v<version>`, `<module>-<version>-<lua>-<target>.tar.gz`,
  for `package.cpath`) are not used by the rockspec yet.

[lsdocs]: https://github.com/lsdocs
[mlua]: https://github.com/mlua-rs/mlua
