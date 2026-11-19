'use strict'

/**
 * A JavaScript port of the first-party `transform-toc` plugin, type-checked
 * against the package's `.d.ts` (`npm run typecheck`).
 *
 * Collects each document's section tree and writes it to the document's
 * `toc` metadata: `[{id, title, level, children}]`. A first section with a
 * level 1 heading is the page title and is left out. With the setting
 * `inline = true` it also inserts a nested list of links to the sections
 * into the document's first section, right after its title. Documents with
 * nothing to list are left alone. Setting `depth` (default 3) is the
 * deepest heading level listed.
 *
 * Its output must match the Rust plugin exactly; see test/toc-port.test.js.
 */

/** @typedef {import('..').Node} Node */
/** @typedef {{ id: string, title: string, level: number, children: Entry[] }} Entry */

/** @type {typeof import('..')} */
const { definePlugin } = require('..')

/**
 * @param {Node[]} nodes
 * @returns {string}
 */
function plainText(nodes) {
  return nodes
    .map((node) => {
      if (node.type === 'text' || node.type === 'inline_code') return node.value
      if (node.type === 'image') return node.alt ?? ''
      return 'children' in node ? plainText(node.children) : ''
    })
    .join('')
}

/**
 * The sections among `nodes` as entries, leaving out sections whose title
 * is deeper than `depth`.
 * @param {Node[]} nodes
 * @param {number} depth
 * @returns {Entry[]}
 */
function entries(nodes, depth) {
  /** @type {Entry[]} */
  const out = []
  for (const node of nodes) {
    if (node.type !== 'section' || node.id == null) continue
    const [heading, ...rest] = node.children
    if (heading?.type !== 'heading' || heading.level > depth) continue
    out.push({
      id: node.id,
      title: plainText(heading.children),
      level: heading.level,
      children: entries(rest, depth),
    })
  }
  return out
}

/**
 * A list of links to `items`, with each item's children nested inside it.
 * @param {Entry[]} items
 * @returns {Node}
 */
function linkList(items) {
  return {
    type: 'list',
    children: items.map((item) => {
      /** @type {Node[]} */
      const children = [
        {
          type: 'paragraph',
          children: [{ type: 'link', url: `#${item.id}`, children: [{ type: 'text', value: item.title }] }],
        },
      ]
      if (item.children.length > 0) children.push(linkList(item.children))
      return /** @type {Node} */ ({ type: 'list_item', children })
    }),
  }
}

// Its settings are lsdocs' own plugin's: `.config/lsdocs/<name>/settings.toml`.
const SETTINGS = 'lsdocs/{name}/settings.toml'

/**
 * The `depth` in a settings file's text (one `depth = N` line).
 * @param {string | null | undefined} text
 */
function depthIn(text) {
  const match = /^\s*depth\s*=\s*(\d+)\s*$/m.exec(text ?? '')
  return match ? Number(match[1]) : 3
}

/**
 * Whether a settings file's text has `inline = true`.
 * @param {string | null | undefined} text
 */
function inlineIn(text) {
  return /^\s*inline\s*=\s*true\s*$/m.test(text ?? '')
}

const plugin = definePlugin({
  name: 'transform-toc-js',
  version: '0.1.0',
  configFiles: [SETTINGS],

  read(params, ctx) {
    const depth = depthIn(ctx.settings?.[SETTINGS.replace('{name}', ctx.config.name)])
    return { sections: entries(params.document.children, depth) }
  },

  transform: {
    nodes: ['section'],
    handler(params, ctx) {
      const { document } = params
      const collected = /** @type {Record<string, { sections?: Entry[] }>} */ (params.collected ?? {})
      const sections = collected[document.source]?.sections ?? []
      if (sections.length > 0) {
        const first = sections[0]
        const rest = sections.slice(1)
        const listed = [...(first.level === 1 ? first.children : [first]), ...rest]
        if (listed.length > 0) document.metadata = { ...document.metadata, toc: listed }
        const inline = inlineIn(ctx.settings?.[SETTINGS.replace('{name}', ctx.config.name)])
        const items = [...first.children, ...rest]
        const title = document.children.find((n) => n.type === 'section')
        if (inline && items.length > 0 && title && 'children' in title) {
          const at = title.children[0]?.type === 'heading' ? 1 : 0
          title.children.splice(at, 0, { type: 'custom', kind: 'toc', children: [linkList(items)] })
        }
      }
      return document
    },
  },
})

module.exports = plugin

if (require.main === module) {
  plugin.serve()
}
