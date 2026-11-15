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
  | ({ type: 'link'; url: string; title?: string; target?: LinkTarget } & Parent)
  | ({ type: 'image'; url: string; alt?: string; title?: string } & NodeBase)
  | ({ type: 'line_break' } & NodeBase)
  | ({ type: 'admonition'; kind: string; title?: string } & Parent)
  | ({ type: 'definition_list' } & Parent)
  | ({ type: 'definition_term' } & Parent)
  | ({ type: 'definition_description' } & Parent)
  | ({ type: 'footnote_reference'; label: string } & NodeBase)
  | ({ type: 'footnote_definition'; label: string } & Parent)
  | ({ type: 'math'; value: string; display?: boolean } & NodeBase)
  | ({ type: 'figure'; id?: string } & Parent)
  | ({ type: 'caption' } & Parent)
  | ({ type: 'anchor'; id: string } & NodeBase)
  | ({ type: 'custom'; kind: string; attrs?: Record<string, unknown> } & Parent)

/**
 * Where a cross-reference link points inside the site: a document (source
 * path) and/or an anchor, or an unresolved `label`.
 */
export interface LinkTarget {
  document?: string
  anchor?: string
  label?: string
}

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
  /** Absolute path of the source directory, which document sources are relative to. */
  source_dir?: string
  /** Every source file (documents and assets), relative to the source directory, sorted. */
  files?: string[]
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
/** A page written by the render phase. */
export interface RenderedPage {
  source: string
  /** Output path, relative to the output directory. */
  path: string
  metadata?: Record<string, unknown>
}
export interface RenderSiteParams {
  format: string
  pages: RenderedPage[]
}
/** A site-wide file to write, e.g. a sitemap. */
export interface OutputFile {
  path: string
  output: string | Uint8Array
}
export interface RenderSiteResult {
  files?: OutputFile[]
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
  /**
   * Called once after every page is rendered; returns site-wide files (or
   * just the list of them). Needs `render`.
   */
  renderSite?:
    | Handler<RenderSiteParams, RenderSiteResult | OutputFile[]>
    | { handler: Handler<RenderSiteParams, RenderSiteResult | OutputFile[]> }
}

export interface Capabilities {
  parse?: { extensions: string[] }
  transform?: { nodes?: string[]; collect: boolean }
  render?: { formats: string[]; site?: boolean }
  max_concurrency?: number
  cache?: string
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
