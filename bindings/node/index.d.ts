/**
 * Write lsdocs plugins in JavaScript or TypeScript. See index.js for an
 * example. Types follow the protocol schemas in `schema/`.
 */

/** A byte range in the source text (end exclusive). */
export interface Span {
  start: number
  end: number
}

interface NodeBase {
  span?: Span
}

interface Parent extends NodeBase {
  children: Node[]
}

/** A node in the document tree, tagged by `type`. */
export type Node =
  | ({ type: 'section'; id?: string } & Parent)
  | ({ type: 'heading'; level: number; id?: string } & Parent)
  | ({ type: 'paragraph' } & Parent)
  | ({ type: 'list'; ordered?: boolean; start?: number } & Parent)
  | ({ type: 'list_item'; checked?: boolean } & Parent)
  | ({ type: 'block_quote' } & Parent)
  | ({ type: 'code_block'; language?: string; info?: string; value: string } & NodeBase)
  | ({ type: 'table'; align?: Array<'none' | 'left' | 'center' | 'right'> } & Parent)
  | ({ type: 'table_row'; header?: boolean } & Parent)
  | ({ type: 'table_cell' } & Parent)
  | ({ type: 'thematic_break' } & NodeBase)
  | ({ type: 'raw'; format: string; value: string } & NodeBase)
  | ({ type: 'text'; value: string } & NodeBase)
  | ({ type: 'emphasis' } & Parent)
  | ({ type: 'strong' } & Parent)
  | ({ type: 'inline_code'; value: string } & NodeBase)
  | ({ type: 'link'; url: string; title?: string } & Parent)
  | ({ type: 'image'; url: string; alt?: string; title?: string } & NodeBase)
  | ({ type: 'line_break' } & NodeBase)
  | ({ type: 'custom'; kind: string; attrs?: Record<string, unknown> } & Parent)

/** A parsed source document. */
export interface Document {
  source: string
  metadata?: Record<string, unknown>
  children: Node[]
}

export interface Diagnostic {
  severity: 'error' | 'warning' | 'info' | 'hint'
  message: string
  source?: string
  span?: Span
  code?: string
}

export interface SiteInfo {
  title?: string
  base_url?: string
  /** Absolute path of the site directory; resolve option paths against it. */
  root?: string
}

/** What a handler receives alongside its params. */
export interface Context {
  readonly site: SiteInfo
  /** The plugin's `options` from the site config (null if absent). */
  readonly options: any
  log(level: 'error' | 'warn' | 'info' | 'debug' | 'trace', message: string): void
  debug(message: string): void
  info(message: string): void
  warn(message: string): void
  progress(message: string, percentage?: number): void
  diagnostic(diagnostic: Diagnostic): void
}

export interface ParseParams {
  source: string
  text: string
}
export interface ParseResult {
  document: Document
  diagnostics?: Diagnostic[]
}
export interface CollectParams {
  document: Document
}
export interface CollectResult {
  data: unknown
  diagnostics?: Diagnostic[]
}
export interface ApplyParams {
  document: Document
  /** Collected data from every document, keyed by source; null without a collect pass. */
  collected: Record<string, unknown> | null
}
export interface ApplyResult {
  document: Document
  diagnostics?: Diagnostic[]
}
export interface RenderParams {
  document: Document
  format: string
}
export interface RenderResult {
  output: string | Uint8Array
  extension?: string
  diagnostics?: Diagnostic[]
}

type MaybePromise<T> = T | Promise<T>
type Handler<P, R> = (params: P, ctx: Context) => MaybePromise<R>

/** A plugin's definition. Each phase is optional. */
export interface PluginDefinition {
  name: string
  version?: string
  /** Requests handled at once (default 1). */
  maxConcurrency?: number
  parse?: { extensions: string[]; handler: Handler<ParseParams, ParseResult | Document> }
  collect?:
    | Handler<CollectParams, CollectResult | unknown>
    | { handler: Handler<CollectParams, CollectResult | unknown> }
  transform?:
    | Handler<ApplyParams, ApplyResult | Document>
    | { nodes?: string[]; handler: Handler<ApplyParams, ApplyResult | Document> }
  render?: {
    formats: string[]
    handler: Handler<RenderParams, RenderResult | string | Uint8Array>
  }
}

export interface Capabilities {
  parse?: { extensions: string[] }
  transform?: { nodes?: string[]; collect: boolean }
  render?: { formats: string[] }
  max_concurrency?: number
}

export declare class Plugin {
  constructor(definition: PluginDefinition)
  readonly name: string
  readonly version: string
  readonly capabilities: Capabilities
  /** Runs the plugin until the host is done with it. */
  serve(): Promise<void>
}

export declare function definePlugin(definition: PluginDefinition): Plugin

/** Protocol version spoken by this package. */
export declare const PROTOCOL_VERSION: number
