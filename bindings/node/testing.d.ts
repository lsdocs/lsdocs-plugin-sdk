import type { Document, Plugin, RenderedPage, SiteInfo } from './index'

export interface CallOptions {
  site?: SiteInfo
  options?: unknown
}

export interface Event {
  kind: 'log' | 'progress' | 'diagnostics'
  [key: string]: unknown
}

export interface Reply<R = any> {
  result: R
  events: Event[]
}

export declare function call(
  plugin: Plugin,
  method: 'initialize' | 'parse' | 'transform/collect' | 'transform/apply' | 'render' | 'render/site',
  params?: unknown,
  options?: CallOptions,
): Promise<Reply>
export declare function parse(plugin: Plugin, source: string, text: string, options?: CallOptions): Promise<Reply>
export declare function collect(plugin: Plugin, document: Document, options?: CallOptions): Promise<Reply>
export declare function apply(
  plugin: Plugin,
  document: Document,
  collected?: Record<string, unknown> | null,
  options?: CallOptions,
): Promise<Reply>
export declare function render(
  plugin: Plugin,
  document: Document,
  format?: string,
  options?: CallOptions,
): Promise<Reply<{ output: Buffer; extension: string }>>
export declare function renderSite(
  plugin: Plugin,
  pages: RenderedPage[],
  format?: string,
  options?: CallOptions,
): Promise<Reply<{ files?: Array<{ path: string; output: Buffer }> }>>
