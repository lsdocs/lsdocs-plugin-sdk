'use strict'

// Every golden fixture decodes with @msgpack/msgpack to its JSON twin: a
// reader outside Rust and serde that would expose structs encoded as
// positional arrays.

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const { decode } = require('@msgpack/msgpack')

const FIXTURES = path.resolve(__dirname, '../../../../fixtures')

function* files(dir) {
  const entries = fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))
  for (const entry of entries) {
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) yield* files(full)
    else if (entry.name.endsWith('.msgpack')) yield full
  }
}

// MessagePack `bin` becomes a list of byte values, as serde writes it to JSON.
function normalize(value) {
  if (value instanceof Uint8Array) return Array.from(value)
  if (Array.isArray(value)) return value.map(normalize)
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, normalize(v)]))
  }
  return value
}

const all = [...files(FIXTURES)]

test('fixtures exist', () => {
  assert.ok(all.length > 0, `no fixtures under ${FIXTURES}; run "nur gen"`)
})

for (const file of all) {
  test(path.relative(FIXTURES, file).replaceAll('\\', '/'), () => {
    const decoded = decode(fs.readFileSync(file))
    const expected = JSON.parse(fs.readFileSync(file.replace(/\.msgpack$/, '.json'), 'utf8'))
    assert.deepEqual(normalize(decoded), expected)
  })
}
