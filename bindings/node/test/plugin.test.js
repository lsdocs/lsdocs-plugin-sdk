'use strict'

// The plugin API, exercised in-process through @lsdocs/plugin/testing.

const test = require('node:test')
const assert = require('node:assert/strict')
const { definePlugin, PROTOCOL_VERSION } = require('..')
const testing = require('../testing')

const text = (value) => ({ type: 'text', value })
const doc = (...lines) => ({ source: 'a.txt', children: lines.map(text) })

function makePlugin() {
  return definePlugin({
    name: 'sample',
    version: '1.2.3',
    parse: {
      extensions: ['txt'],
      // Bare document shorthand, without `source`.
      handler: (params) => ({ children: params.text.split('\n').map(text) }),
    },
    collect: (params) => params.document.children.length,
    transform: {
      nodes: ['text'],
      async handler(params, ctx) {
        ctx.info(`applying with ${JSON.stringify(params.collected)}`)
        await new Promise((resolve) => setTimeout(resolve, 5))
        for (const node of params.document.children) node.value = node.value.toUpperCase()
        return { document: params.document, diagnostics: [{ severity: 'hint', message: 'shouted' }] }
      },
    },
    render: {
      formats: ['txt', 'bin'],
      handler(params, ctx) {
        const joined = params.document.children.map((n) => n.value).join('\n')
        if (params.format === 'bin') {
          return { output: Buffer.concat([Buffer.from(joined), Buffer.from([0, 255])]), extension: 'dat' }
        }
        return `${ctx.site.title}: ${joined}`
      },
    },
  })
}

test('capabilities follow the handlers', () => {
  assert.deepEqual(makePlugin().capabilities, {
    parse: { extensions: ['txt'] },
    transform: { nodes: ['text'], collect: true },
    render: { formats: ['txt', 'bin'] },
    max_concurrency: 1,
  })
  assert.deepEqual(definePlugin({ name: 'empty' }).capabilities, { max_concurrency: 1 })
})

test('initialize', async () => {
  const { result } = await testing.call(makePlugin(), 'initialize')
  assert.equal(result.protocol_version, PROTOCOL_VERSION)
  assert.deepEqual(result.plugin, { name: 'sample', version: '1.2.3' })
})

test('parse shorthand fills in the document', async () => {
  const { result, events } = await testing.parse(makePlugin(), 'notes/a.txt', 'one\ntwo')
  assert.equal(result.document.source, 'notes/a.txt')
  assert.deepEqual(result.document.children, [text('one'), text('two')])
  assert.deepEqual(events, [])
})

test('collect and async apply', async () => {
  const plugin = makePlugin()
  const collected = await testing.collect(plugin, doc('a', 'b'))
  assert.deepEqual(collected.result, { data: 2 })

  const { result, events } = await testing.apply(plugin, doc('a', 'b'), { 'a.txt': 2 })
  assert.deepEqual(
    result.document.children.map((n) => n.value),
    ['A', 'B'],
  )
  assert.deepEqual(result.diagnostics, [{ severity: 'hint', message: 'shouted' }])
  assert.deepEqual(events, [{ kind: 'log', level: 'info', message: 'applying with {"a.txt":2}' }])
})

test('render strings and bytes', async () => {
  const plugin = makePlugin()
  const text = await testing.render(plugin, doc('hi'), 'txt', { site: { title: 'Site' } })
  assert.deepEqual(text.result, { output: Buffer.from('Site: hi'), extension: 'txt' })
  const bin = await testing.render(plugin, doc('hi'), 'bin')
  assert.deepEqual(bin.result, { output: Buffer.from([104, 105, 0, 255]), extension: 'dat' })
})

test('context options, progress and diagnostics', async () => {
  const plugin = definePlugin({
    name: 'ctx',
    transform(params, ctx) {
      ctx.progress('halfway', 50)
      ctx.diagnostic({ severity: 'warning', message: 'careful', code: 'x' })
      ctx.log('debug', `depth=${ctx.options.depth}`)
      return params.document
    },
  })
  const { events } = await testing.apply(plugin, doc(), null, { options: { depth: 2 } })
  assert.deepEqual(events, [
    { kind: 'progress', message: 'halfway', percentage: 50 },
    { kind: 'diagnostics', diagnostics: [{ severity: 'warning', message: 'careful', code: 'x' }] },
    { kind: 'log', level: 'debug', message: 'depth=2' },
  ])
})

test('errors carry their stack', async () => {
  const plugin = definePlugin({
    name: 'broken',
    transform() {
      throw new RangeError('no good')
    },
  })
  await assert.rejects(testing.apply(plugin, doc()), (err) => {
    assert.match(err.message, /^RangeError: no good/)
    assert.match(err.message, /plugin\.test\.js/)
    return true
  })
})

test('invalid results are reported', async () => {
  const plugin = definePlugin({ name: 'bad', transform: () => ({ document: 'not a document' }) })
  await assert.rejects(testing.apply(plugin, doc()), /invalid transform\/apply result/)
})

test('bad definitions are rejected', () => {
  assert.throws(() => definePlugin({}), /name is required/)
  assert.throws(() => definePlugin({ name: 'x', parse: { handler() {} } }), /parse.extensions is required/)
  assert.throws(() => definePlugin({ name: 'x', render: 5 }), /render must be a function or \{ handler \}/)
})

test('bad log level', async () => {
  const plugin = definePlugin({ name: 'bad', transform: (_p, ctx) => ctx.log('loud', 'x') })
  await assert.rejects(testing.apply(plugin, doc()), /unknown log level "loud"/)
})
