'use strict'

/**
 * A JavaScript port of the first-party `transform-toc` plugin, type-checked
 * against the package's `.d.ts` (`npm run typecheck`).
 *
 * Collects each document's headings, then inserts a list of links to them
 * after the document's first heading. Headings deeper than the one before
 * them are nested under it. Documents with fewer than two headings are left
 * alone. Option `depth` (default 3) is the deepest level listed.
 *
 * Its output must match the Rust plugin exactly; see test/toc-port.test.js.
 */

/** @typedef {import('..').Node} Node */
/** @typedef {{ id: string, level: number, title: string }} Entry */

/** @type {typeof import('..')} */
const { definePlugin } = require('..')

/**
 * Every node, depth first, parents before children.
 * @param {Node[]} nodes
 * @returns {Generator<Node>}
 */
function* descendants(nodes) {
  for (const node of nodes) {
    yield node
    if ('children' in node) yield* descendants(node.children)
  }
}

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
 * A list of links to `entries`. Each entry is followed by the entries deeper
 * than it, which become a nested list inside its item.
 * @param {Entry[]} entries
 * @returns {Node}
 */
function linkList(entries) {
  /** @type {Node[]} */
  const items = []
  let i = 0
  while (i < entries.length) {
    const entry = entries[i]
    let end = i + 1
    while (end < entries.length && entries[end].level > entry.level) end++
    /** @type {Node[]} */
    const children = [
      {
        type: 'paragraph',
        children: [{ type: 'link', url: `#${entry.id}`, children: [{ type: 'text', value: entry.title }] }],
      },
    ]
    if (end > i + 1) children.push(linkList(entries.slice(i + 1, end)))
    items.push({ type: 'list_item', children })
    i = end
  }
  return { type: 'list', children: items }
}

/**
 * A `custom:toc` node wrapping a (nested) list of links.
 * @param {Entry[]} entries
 * @returns {Node}
 */
function tocNode(entries) {
  return { type: 'custom', kind: 'toc', children: [linkList(entries)] }
}

const plugin = definePlugin({
  name: 'transform-toc-js',
  version: '0.1.0',

  collect(params, ctx) {
    const depth = ctx.options?.depth ?? 3
    /** @type {Entry[]} */
    const headings = []
    for (const node of descendants(params.document.children)) {
      if (node.type === 'heading' && node.id != null && node.level <= depth) {
        headings.push({ id: node.id, level: node.level, title: plainText(node.children) })
      }
    }
    return { headings }
  },

  transform: {
    nodes: ['heading'],
    handler(params) {
      const { document } = params
      const collected = /** @type {Record<string, { headings?: Entry[] }>} */ (params.collected ?? {})
      const entries = collected[document.source]?.headings ?? []
      if (entries.length >= 2) {
        const index = document.children.findIndex((n) => n.type === 'heading')
        document.children.splice(index + 1, 0, tocNode(entries.slice(1)))
      }
      return document
    },
  },
})

module.exports = plugin

if (require.main === module) {
  plugin.serve()
}
