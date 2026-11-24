-- A Lua port of the first-party `transform-toc` plugin (and of the Python
-- package's examples/transform_toc.py).
--
-- Collects each document's section tree and writes it to the document's
-- `toc` metadata: `{ { id, title, level, children } }`. A first section
-- with a level 1 heading is the page title and is left out. With the
-- setting `inline = true` it also inserts a nested list of links to the
-- sections into the document's first section, right after its title.
-- Documents with nothing to list are left alone. The setting `depth`
-- (default 3) is the deepest heading level listed.
--
-- Its output must match the Rust plugin exactly; see tests/toc_port.rs.
local lsdocs = require("lsdocs_plugin")

-- Its settings are lsdocs' own plugin's: `.config/lsdocs/<name>/settings.toml`.
local SETTINGS = "lsdocs/{name}/settings.toml"
local plugin = lsdocs.plugin("transform-toc-lua", "0.1.0", { config_files = { SETTINGS } })

-- The `key = value` lines of a settings file (integers, booleans, quoted
-- strings), which is all this plugin's settings use; Lua has no TOML
-- parser built in.
local function parse_settings(text)
  local out = {}
  for line in (text or ""):gmatch("[^\r\n]+") do
    local key, value = line:match("^%s*([%w_]+)%s*=%s*(.-)%s*$")
    if key and not line:match("^%s*#") then
      value = value:gsub("%s+#.*$", "")
      if value == "true" or value == "false" then
        out[key] = value == "true"
      elseif value:match('^".*"$') then
        out[key] = value:sub(2, -2)
      else
        out[key] = tonumber(value)
      end
    end
  end
  return out
end

local function settings(ctx)
  local path = SETTINGS:gsub("{name}", ctx.config.name)
  return parse_settings(ctx.settings[path])
end

local function plain_text(nodes)
  local out = {}
  for _, node in ipairs(nodes) do
    if node.type == "text" or node.type == "inline_code" then
      out[#out + 1] = node.value
    elseif node.type == "image" then
      out[#out + 1] = node.alt or ""
    else
      out[#out + 1] = plain_text(node.children or {})
    end
  end
  return table.concat(out)
end

-- The sections among `nodes` (from index `from`) as `{ id, title, level,
-- children }` entries, leaving out sections whose title is deeper than
-- `depth`.
local function entries(nodes, depth, from)
  local out = {}
  for i = from or 1, #nodes do
    local node = nodes[i]
    local children = node.children or {}
    local heading = children[1]
    if
      node.type == "section"
      and node.id ~= nil
      and heading ~= nil
      and heading.type == "heading"
      and heading.level <= depth
    then
      out[#out + 1] = {
        id = node.id,
        title = plain_text(heading.children or {}),
        level = heading.level,
        children = entries(children, depth, 2),
      }
    end
  end
  return out
end

plugin:read(function(params, ctx)
  return { sections = entries(params.document.children, settings(ctx).depth or 3) }
end)

-- A list of links to `items`, with each item's children nested inside it.
local function link_list(items)
  local out = {}
  for _, item in ipairs(items) do
    local children = {
      {
        type = "paragraph",
        children = {
          {
            type = "link",
            url = "#" .. item.id,
            children = { { type = "text", value = item.title } },
          },
        },
      },
    }
    if #item.children > 0 then
      children[#children + 1] = link_list(item.children)
    end
    out[#out + 1] = { type = "list_item", children = children }
  end
  return { type = "list", children = out }
end

local function concat(a, b, from)
  local out = {}
  for _, v in ipairs(a) do
    out[#out + 1] = v
  end
  for i = from or 1, #b do
    out[#out + 1] = b[i]
  end
  return out
end

plugin:transform({ "section" }, function(params, ctx)
  local document = params.document
  local collected = params.collected or {}
  local sections = (collected[document.source] or {}).sections or {}
  if #sections > 0 then
    local first = sections[1]
    local toc = concat(first.level == 1 and first.children or { first }, sections, 2)
    if #toc > 0 then
      document.metadata = document.metadata or {}
      document.metadata.toc = toc
    end
    local items = concat(first.children, sections, 2)
    local title
    for _, node in ipairs(document.children) do
      if node.type == "section" then
        title = node
        break
      end
    end
    if settings(ctx).inline and #items > 0 and title ~= nil then
      local children = title.children or {}
      title.children = children
      local at = (children[1] and children[1].type == "heading") and 2 or 1
      table.insert(children, at, { type = "custom", kind = "toc", children = { link_list(items) } })
    end
  end
  return document
end)

plugin:serve()
