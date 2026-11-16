# @lsdocs/plugin

Write [lsdocs](../../../ROADMAP.md) plugins in JavaScript or TypeScript.

```js
const { definePlugin } = require('@lsdocs/plugin')

const plugin = definePlugin({
  name: 'shout',
  version: '0.1.0',
  transform: {
    nodes: ['text'],
    handler(params) {
      for (const node of params.document.children) {
        if (node.type === 'text') node.value = node.value.toUpperCase()
      }
      return params.document
    },
  },
})

if (require.main === module) plugin.serve()
```

Use it from `lsdocs.toml`:

```toml
[[plugins]]
name = "shout"
runtime = "node"
entry = "shout.js"
```

Handlers may be `async`. Types for params, results and document nodes are in
`index.d.ts`. `@lsdocs/plugin/testing` runs handlers in-process for unit
tests. `examples/transform-toc.js` is a full plugin: a port of the
first-party table-of-contents transformer, type-checked against the types.

Context events (`ctx.log`, `ctx.progress`, `ctx.diagnostic`) reach the host
when the handler finishes.

## Development

```sh
nur test node   # from the repository root: builds the addon, type-checks, runs node --test
nur gen node    # regenerates native.js / native.d.ts after changing the Rust API
```
