'use strict'

/**
 * Run a plugin's handlers in-process, as the host would, for tests:
 *
 *     const testing = require('@lsdocs/plugin/testing')
 *     const { result, events } = await testing.parse(plugin, 'a.txt', 'hello')
 *
 * `result` is normalized to the full protocol shape; `events` lists the log
 * lines, progress and diagnostics the handler sent, each with a `kind`.
 * Render output (and each `render/site` file's output) is returned as a
 * Buffer.
 */

const native = require('./native.js')

const json = (value) => (value === undefined ? undefined : JSON.stringify(value))

async function call(plugin, method, params = null, { site, options } = {}) {
  const reply = JSON.parse(await native.call(plugin.native, method, JSON.stringify(params), json(site), json(options)))
  const toBuffer = (value) => {
    if (Array.isArray(value?.output)) value.output = Buffer.from(value.output)
  }
  if (method === 'render') toBuffer(reply.result)
  if (method === 'render/site') (reply.result.files ?? []).forEach(toBuffer)
  return reply
}

const parse = (plugin, source, text, opts) => call(plugin, 'parse', { source, text }, opts)
const collect = (plugin, document, opts) => call(plugin, 'transform/collect', { document }, opts)
const apply = (plugin, document, collected = null, opts) =>
  call(plugin, 'transform/apply', { document, collected }, opts)
const render = (plugin, document, format = 'html', opts) => call(plugin, 'render', { document, format }, opts)
const renderSite = (plugin, pages, format = 'html', opts) => call(plugin, 'render/site', { format, pages }, opts)

module.exports = { call, parse, collect, apply, render, renderSite }
