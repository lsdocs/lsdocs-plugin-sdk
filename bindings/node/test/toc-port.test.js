'use strict'

// The JS port of transform-toc produces the same site as the Rust one, over
// stdio, a pipe, and `runtime = "node"`. Needs the `lsdocs` binary: set
// LSDOCS_BIN (`nur test node` does).

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const ROOT = path.resolve(__dirname, '../../..')
const SAMPLE_DOCS = path.join(ROOT, 'examples', 'sample-site', 'docs')
const PORT = path.resolve(__dirname, '..', 'examples', 'transform-toc.js')
const LSDOCS = process.env.LSDOCS_BIN

// Literal (single-quoted) TOML strings keep Windows paths intact.
const tomlStr = (value) => `'${value}'`

function config(tocPlugin) {
  return [
    '[site]',
    'title = "lsdocs sample"',
    '',
    '[build]',
    `source_dir = ${tomlStr(SAMPLE_DOCS)}`,
    '',
    '[[plugins]]',
    'name = "markdown"',
    'builtin = "parser-markdown"',
    '',
    '[[plugins]]',
    'name = "toc"',
    'options = { depth = 2 }',
    tocPlugin,
    '',
    '[[plugins]]',
    'name = "html"',
    'builtin = "render-html"',
    '',
  ].join('\n')
}

function readTree(dir, prefix = '', out = {}) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const rel = prefix ? `${prefix}/${entry.name}` : entry.name
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) readTree(full, rel, out)
    else out[rel] = fs.readFileSync(full)
  }
  return out
}

function build(tmp, label, tocPlugin, env = {}) {
  const site = path.join(tmp, label)
  fs.mkdirSync(site)
  fs.writeFileSync(path.join(site, 'lsdocs.toml'), config(tocPlugin))
  const proc = spawnSync(LSDOCS, ['build', '--config', path.join(site, 'lsdocs.toml')], {
    env: { ...process.env, ...env },
    encoding: 'utf8',
    timeout: 120_000,
  })
  assert.equal(proc.status, 0, `${label} build failed:\n${proc.stderr}`)
  assert.ok(!/warning|error/.test(proc.stderr), proc.stderr)
  return readTree(path.join(site, 'site'))
}

function assertSame(actual, expected, how) {
  assert.deepEqual(Object.keys(actual).sort(), Object.keys(expected).sort())
  for (const name of Object.keys(expected)) {
    assert.ok(actual[name].equals(expected[name]), `${name} differs ${how}`)
  }
}

test('JS port matches the Rust transformer', { skip: !LSDOCS && 'LSDOCS_BIN is not set' }, () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'lsdocs-node-'))
  try {
    const rust = build(tmp, 'rust', 'builtin = "transform-toc"')
    assert.ok(rust['index.html'].includes('<div class="toc">'))

    const command = `command = [${tomlStr(process.execPath)}, ${tomlStr(PORT)}]`
    for (const transport of ['stdio', 'pipe']) {
      const js = build(tmp, `js-${transport}`, `${command}\ntransport = "${transport}"`)
      assertSame(js, rust, `over ${transport}`)
    }

    // `runtime = "node"` runs `node <entry>`; LSDOCS_NODE picks this Node.js.
    const runtime = build(tmp, 'js-runtime', `runtime = "node"\nentry = ${tomlStr(PORT)}`, {
      LSDOCS_NODE: process.execPath,
    })
    assertSame(runtime, rust, 'with runtime = "node"')
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true })
  }
})
