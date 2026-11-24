-- The plugin API, exercised in-process through lsdocs_plugin.call (the
-- Lua counterpart of the Python package's tests/test_plugin.py). Each
-- spec file returns a table of test functions, run by tests/specs.rs.
local lsdocs = require("lsdocs_plugin")

-- Deep equality of plain values, with a path to the first difference.
local function diff(a, b, path)
  if type(a) ~= type(b) then
    return path .. ": " .. type(a) .. " vs " .. type(b)
  end
  if type(a) ~= "table" then
    if a ~= b then
      return path .. ": " .. tostring(a) .. " vs " .. tostring(b)
    end
    return nil
  end
  for k, v in pairs(a) do
    local d = diff(v, b[k], path .. "." .. tostring(k))
    if d then
      return d
    end
  end
  for k, v in pairs(b) do
    if a[k] == nil then
      return path .. "." .. tostring(k) .. ": missing, expected " .. tostring(v)
    end
  end
  return nil
end

local function same(actual, expected, what)
  local d = diff(actual, expected, what)
  if d then
    error(d, 2)
  end
end

-- `fn` raises an error whose message contains `needle`.
local function raises(fn, needle)
  local ok, err = pcall(fn)
  if ok then
    error("expected an error containing " .. needle, 2)
  end
  if not tostring(err):find(needle, 1, true) then
    error("error " .. tostring(err) .. " does not contain " .. needle, 2)
  end
end

local function text(value)
  return { type = "text", value = value }
end

local function doc(...)
  local children = {}
  for i, line in ipairs({ ... }) do
    children[i] = text(line)
  end
  return { source = "a.txt", children = children }
end

local function make_plugin()
  local plugin = lsdocs.plugin("sample", "1.2.3")

  plugin:parse({ "txt" }, function(params)
    -- Bare document shorthand, without `source`.
    local children = {}
    for line in params.text:gmatch("[^\n]+") do
      children[#children + 1] = text(line)
    end
    return { children = children }
  end)

  plugin:read(function(params)
    return #params.document.children
  end)

  plugin:transform({ "text" }, function(params, ctx)
    ctx:info("applying with " .. params.collected["a.txt"])
    local d = params.document
    for _, node in ipairs(d.children) do
      node.value = node.value:upper()
    end
    return { document = d, diagnostics = { { severity = "hint", message = "shouted" } } }
  end)

  plugin:render({ "txt", "bin" }, function(params, ctx)
    local lines = {}
    for i, node in ipairs(params.document.children) do
      lines[i] = node.value
    end
    local joined = table.concat(lines, "\n")
    if params.format == "bin" then
      -- Not UTF-8: sent as bytes.
      return { output = joined .. "\0\255", extension = "dat" }
    end
    return (ctx.site.title or "") .. ": " .. joined
  end)

  return plugin
end

local tests = {}

function tests.capabilities_follow_the_handlers()
  same(make_plugin():capabilities(), {
    parse = { extensions = { "txt" } },
    transform = { nodes = { "text" }, each = true },
    render = { formats = { "txt", "bin" } },
    max_concurrency = 1,
  }, "capabilities")
  same(lsdocs.plugin("empty"):capabilities(), { max_concurrency = 1 }, "empty")
end

function tests.initialize()
  local result = lsdocs.call(make_plugin(), "initialize")
  same(result.protocol_version, lsdocs.PROTOCOL_VERSION, "protocol_version")
  same(result.plugin, { name = "sample", version = "1.2.3" }, "plugin")
end

function tests.parse_shorthand_fills_in_the_document()
  local result, events =
    lsdocs.call(make_plugin(), "parse", { source = "notes/a.txt", text = "one\ntwo" })
  same(result.document.source, "notes/a.txt", "source")
  same(result.document.children, { text("one"), text("two") }, "children")
  same(events, {}, "events")
end

function tests.read_and_apply()
  local plugin = make_plugin()
  local collected = lsdocs.call(plugin, "parse/transform/each", { document = doc("a", "b") })
  same(collected, { data = 2 }, "collected")

  local result, events = lsdocs.call(
    plugin,
    "render/transform/each",
    { document = doc("a", "b"), collected = { ["a.txt"] = 2 } }
  )
  same(result.document.children, { text("A"), text("B") }, "children")
  same(result.diagnostics, { { severity = "hint", message = "shouted" } }, "diagnostics")
  same(events, { { kind = "log", level = "info", message = "applying with 2" } }, "events")
end

function tests.render_strings_and_bytes()
  local plugin = make_plugin()
  local result = lsdocs.call(
    plugin,
    "render",
    { document = doc("hi"), format = "txt" },
    { site = { title = "Site" } }
  )
  same(result, { output = "Site: hi", extension = "txt" }, "txt")
  result = lsdocs.call(plugin, "render", { document = doc("hi"), format = "bin" })
  same(result, { output = "hi\0\255", extension = "dat" }, "bin")
end

function tests.render_all_returns_extra_files()
  local plugin = make_plugin()
  same(plugin:capabilities().render.all, nil, "no all yet")
  plugin:render_all(function(params)
    local paths = {}
    for i, page in ipairs(params.pages) do
      paths[i] = page.path
    end
    -- A bare list of files.
    return {
      { path = "index.txt", output = table.concat(paths, "\n") },
      { path = "raw.bin", output = "\0\255" },
    }
  end)
  same(plugin:capabilities().render.all, true, "all")
  local pages = { { source = "a.txt", path = "a.txt" }, { source = "b.txt", path = "b.txt" } }
  local result = lsdocs.call(plugin, "render/transform/all", { pages = pages, format = "txt" })
  same(result, {
    files = {
      { path = "index.txt", output = "a.txt\nb.txt" },
      { path = "raw.bin", output = "\0\255" },
    },
  }, "files")
end

function tests.context_settings_progress_and_diagnostics()
  local plugin = lsdocs.plugin("ctx")
  plugin:transform(nil, function(params, ctx)
    ctx:progress("halfway", 50)
    ctx:diagnostic({ severity = "warning", message = "careful", code = "x" })
    ctx:log("debug", "depth=" .. ctx.settings.depth .. " name=" .. ctx.config.name)
    return params.document
  end)
  local _, events = lsdocs.call(
    plugin,
    "render/transform/each",
    { document = doc() },
    { settings = { depth = 2 }, config = { dir = "/c", name = "t" } }
  )
  same(events, {
    { kind = "progress", message = "halfway", percentage = 50 },
    {
      kind = "diagnostics",
      diagnostics = { { severity = "warning", message = "careful", code = "x" } },
    },
    { kind = "log", level = "debug", message = "depth=2 name=t" },
  }, "events")
end

function tests.errors_carry_their_traceback()
  local plugin = lsdocs.plugin("broken")
  plugin:transform(nil, function()
    error("no good")
  end)
  local ok, err = pcall(lsdocs.call, plugin, "render/transform/each", { document = doc() })
  same(ok, false, "fails")
  err = tostring(err)
  same(err:find("no good", 1, true) ~= nil, true, "message: " .. err)
  same(err:find("traceback", 1, true) ~= nil, true, "traceback: " .. err)
end

function tests.invalid_results_are_reported()
  local plugin = lsdocs.plugin("bad")
  plugin:transform(nil, function()
    return { document = "not a document" }
  end)
  raises(function()
    lsdocs.call(plugin, "render/transform/each", { document = doc() })
  end, "invalid render/transform/each result")
end

function tests.unsupported_values_are_rejected()
  local plugin = lsdocs.plugin("bad")
  plugin:transform(nil, function()
    return { document = print }
  end)
  raises(function()
    lsdocs.call(plugin, "render/transform/each", { document = doc() })
  end, "cannot send a function to lsdocs")
end

function tests.mixed_tables_are_rejected()
  local plugin = lsdocs.plugin("bad")
  plugin:transform(nil, function(params)
    -- selene: allow(mixed_table)
    params.document.metadata = { 1, 2, x = 3 }
    return params.document
  end)
  raises(function()
    lsdocs.call(plugin, "render/transform/each", { document = doc() })
  end, "keys must be strings")
end

function tests.bad_log_level()
  local plugin = lsdocs.plugin("bad")
  plugin:transform(nil, function(_, ctx)
    ctx:log("loud", "x")
  end)
  raises(function()
    lsdocs.call(plugin, "render/transform/each", { document = doc() })
  end, 'unknown log level "loud"')
end

function tests.empty_tables_lists_objects_and_null()
  local plugin = lsdocs.plugin("shapes")
  plugin:transform(nil, function(params)
    local d = params.document
    -- An empty table is a list, except as `metadata`; object() marks a map.
    d.children = { { type = "paragraph", children = {} } }
    d.metadata = {}
    d.metadata.empty_map = lsdocs.object()
    d.metadata.empty_list = {}
    d.metadata.nothing = lsdocs.null
    -- A number with no fraction is an integer (LuaJIT has no integers).
    d.metadata.two = 2.0
    return d
  end)
  local result = lsdocs.call(plugin, "render/transform/each", { document = doc("x") })
  same(#result.document.children[1].children, 0, "children")
  same(next(result.document.metadata.empty_map), nil, "empty map")
  same(result.document.metadata.nothing, nil, "null is nil coming back")
  -- Lua 5.4 prints a float as "2.0"; an integer (or any LuaJIT number) as "2".
  same(tostring(result.document.metadata.two), "2", "integer")
end

function tests.config_files_are_read_at_initialize()
  -- The specs run with the working directory at the crate.
  local plugin =
    lsdocs.plugin("cfg", "1.0", { config_files = { "{name}/settings.toml", "{name}/x.ini" } })
  local seen
  plugin:transform({ "text" }, function(params, ctx)
    seen = ctx.settings
    return params.document
  end)
  local config = { dir = "spec", name = "fixtures" }
  local result = lsdocs.call(plugin, "initialize", {}, { config = config })
  same(result.config_files, { "fixtures/settings.toml", "fixtures/x.ini" }, "config_files")
  lsdocs.call(plugin, "render/transform/each", { document = doc() }, { config = config })
  -- A missing file is nil.
  same(seen, { ["fixtures/settings.toml"] = "depth = 4\n" }, "settings")
end

return tests
