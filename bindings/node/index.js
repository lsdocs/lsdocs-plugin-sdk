'use strict'

/**
 * Write lsdocs plugins in JavaScript or TypeScript.
 *
 *     const { definePlugin } = require('@lsdocs/plugin')
 *
 *     const plugin = definePlugin({
 *       name: 'shout',
 *       version: '0.1.0',
 *       transform: {
 *         nodes: ['text'],
 *         handler(params, ctx) {
 *           for (const node of params.document.children) {
 *             if (node.type === 'text') node.value = node.value.toUpperCase()
 *           }
 *           return params.document
 *         },
 *       },
 *     })
 *
 *     if (require.main === module) plugin.serve()
 *
 * Handlers get the request params (shaped like the protocol schema) and a
 * context, and return the result, or a Promise of it. Shorthand results:
 * `parse` and `transform` may return just the document, `read` any value
 * (taken as `data`), `render` a string or bytes (taken as `output`). The
 * protocol itself is handled in Rust.
 */

const native = require('./native.js')

const LEVELS = new Set(['error', 'warn', 'info', 'debug', 'trace'])

/** Builds the context passed to handlers; events are recorded in `events`. */
function makeContext(site, options, events) {
  return {
    site,
    options,
    log(level, message) {
      if (!LEVELS.has(level)) {
        throw new Error(`unknown log level ${JSON.stringify(level)}`)
      }
      events.push({ kind: 'log', level, message: String(message) })
    },
    debug(message) {
      this.log('debug', message)
    },
    info(message) {
      this.log('info', message)
    },
    warn(message) {
      this.log('warn', message)
    },
    progress(message, percentage) {
      const event = { kind: 'progress', message: String(message) }
      if (percentage !== undefined && percentage !== null) event.percentage = percentage
      events.push(event)
    },
    diagnostic(diagnostic) {
      events.push({ kind: 'diagnostics', diagnostics: [diagnostic] })
    },
  }
}

/** JSON with bytes (Buffer, Uint8Array) as arrays of byte values. */
function toJson(value) {
  return JSON.stringify(value, (_key, v) => {
    if (v instanceof Uint8Array) return Array.from(v)
    if (v && v.type === 'Buffer' && Array.isArray(v.data)) return v.data
    return v
  })
}

/** Adapts a user handler to the native `(json) => Promise<json>` shape. */
function wrap(handler) {
  if (typeof handler !== 'function') {
    throw new TypeError('handler must be a function')
  }
  return async (input) => {
    const { params, site, options } = JSON.parse(input)
    const events = []
    try {
      const result = await handler(params, makeContext(site, options, events))
      return toJson({ result: result === undefined ? null : result, events })
    } catch (err) {
      const error = err instanceof Error ? err.stack || String(err) : String(err)
      return toJson({ error, events })
    }
  }
}

/** Accepts `fn` or `{ handler: fn, ...settings }`. */
function split(spec, what) {
  if (typeof spec === 'function') return [spec, {}]
  if (spec && typeof spec.handler === 'function') return [spec.handler, spec]
  throw new TypeError(`${what} must be a function or { handler }`)
}

class Plugin {
  constructor(definition) {
    const { name, version = '0.0.0', maxConcurrency = 1 } = definition
    if (typeof name !== 'string' || !name) throw new TypeError('name is required')
    this.name = name
    this.version = version
    this.native = new native.NativePlugin(name, version, maxConcurrency)

    if (definition.parse) {
      const [handler, { extensions }] = split(definition.parse, 'parse')
      if (!Array.isArray(extensions)) throw new TypeError('parse.extensions is required')
      this.native.setParse(extensions, wrap(handler))
    }
    if (definition.read) {
      const [handler] = split(definition.read, 'read')
      this.native.setRead(wrap(handler))
    }
    if (definition.transform) {
      const [handler, { nodes = [] }] = split(definition.transform, 'transform')
      this.native.setApply(nodes, wrap(handler))
    }
    if (definition.render) {
      const [handler, { formats }] = split(definition.render, 'render')
      if (!Array.isArray(formats)) throw new TypeError('render.formats is required')
      this.native.setRender(formats, wrap(handler))
    }
    if (definition.renderAll) {
      if (!definition.render) throw new TypeError('renderAll needs render')
      const [handler] = split(definition.renderAll, 'renderAll')
      this.native.setRenderAll(wrap(handler))
    }
  }

  /** The capabilities reported to the host. */
  get capabilities() {
    return JSON.parse(this.native.capabilities())
  }

  /**
   * Runs the plugin until the host is done with it, over stdin/stdout or the
   * pipe in `LSDOCS_PIPE`. From here on, writes to stdout (including
   * `console.log`) go to stderr so they cannot corrupt the protocol stream.
   */
  serve() {
    process.stdout.write = process.stderr.write.bind(process.stderr)
    return native.serve(this.native)
  }
}

/** Defines a plugin. */
function definePlugin(definition) {
  return new Plugin(definition)
}

module.exports = { definePlugin, Plugin, PROTOCOL_VERSION: native.PROTOCOL_VERSION }
