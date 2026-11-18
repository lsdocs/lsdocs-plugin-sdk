//! Write lsdocs plugins in Rust.
//!
//! Implement one or more of [`Parser`], [`Transformer`] and [`Renderer`],
//! bundle them into a [`Plugin`], and hand it to [`main`]:
//!
//! ```no_run
//! use lsdocs_plugin_sdk::{Context, Plugin, PluginResult, Transformer};
//! use lsdocs_plugin_sdk::protocol::{RenderTransformEachParams, RenderTransformEachResult};
//!
//! struct Shout;
//!
//! impl Transformer for Shout {
//!     fn nodes(&self) -> Vec<String> {
//!         vec!["text".into()]
//!     }
//!
//!     async fn apply(
//!         &self,
//!         params: RenderTransformEachParams,
//!         ctx: &Context,
//!     ) -> PluginResult<RenderTransformEachResult> {
//!         ctx.info("shouting").await;
//!         Ok(RenderTransformEachResult { document: params.document, ..Default::default() })
//!     }
//! }
//!
//! fn main() -> std::process::ExitCode {
//!     lsdocs_plugin_sdk::main(Plugin::new("shout", "0.1.0").transformer(Shout))
//! }
//! ```
//!
//! The capabilities sent to the host follow from what the plugin
//! implements. [`main`] speaks the protocol over stdio, or over the pipe in
//! `LSDOCS_PIPE` when the host sets it, and returns when the host says
//! `exit` or closes the connection.
//!
//! The host can also run a [`Plugin`] in-process, calling [`Plugin::parse`]
//! and friends directly with no serialization.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock, PoisonError};

pub mod config;
pub mod links;
pub use lsdocs_protocol as protocol;
pub use lsdocs_protocol::doctree;
use lsdocs_protocol::doctree::Value;
use lsdocs_protocol::{
    Capabilities, ConfigInfo, Diagnostic, DiagnosticsParams, HostParseParams, InitializeParams,
    InitializeResult, LogLevel, LogParams, PROTOCOL_VERSION, ParseCapability, ParseParams,
    ParseResult, ParseTransformAllResult, ParseTransformEachParams, ParseTransformEachResult,
    ProgramInfo, ProgressParams, RenderCapability, RenderParams, RenderResult,
    RenderTransformAllParams, RenderTransformAllResult, RenderTransformEachParams,
    RenderTransformEachResult, SiteInfo, Syntax, TransformCapability, notification, request,
};
/// Tracing target for wire logs; enable it at TRACE level to log every
/// protocol message.
pub use lsdocs_rpc::trace::TARGET as TRACE_TARGET;
use lsdocs_rpc::transport::{self, Transport, pipe};
use lsdocs_rpc::{ErrorCode, Options, Peer, Router, RpcError};

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Result of a plugin operation.
pub type PluginResult<T> = Result<T, PluginError>;

/// A plugin operation failed. Any `std::error::Error` converts into it, so
/// `?` works on most errors.
pub struct PluginError {
    message: String,
}

impl PluginError {
    /// Creates an error with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl<E: std::error::Error> From<E> for PluginError {
    fn from(e: E) -> Self {
        Self::new(e.to_string())
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PluginError({:?})", self.message)
    }
}

// ---------------------------------------------------------------------------
// Plugin traits
// ---------------------------------------------------------------------------

/// Turns source files into documents.
pub trait Parser: Send + Sync + 'static {
    /// File extensions handled, without the dot, e.g. `["md"]`.
    fn extensions(&self) -> Vec<String>;

    /// Parses one file.
    fn parse(
        &self,
        params: ParseParams,
        ctx: &Context,
    ) -> impl Future<Output = PluginResult<ParseResult>> + Send;
}

/// Rewrites documents, optionally using data collected from every document
/// first.
pub trait Transformer: Send + Sync + 'static {
    /// Whether to run the collect pass. If true, [`Transformer::read`] is
    /// called for every document before any [`Transformer::apply`], and
    /// `apply` receives everything collected.
    const READS: bool = false;

    /// Node kinds this transformer looks at, e.g. `["heading"]`. The host
    /// skips documents containing none of them. Empty means every document.
    fn nodes(&self) -> Vec<String> {
        Vec::new()
    }

    /// Gathers data from one document. Only called if
    /// [`Transformer::READS`] is true.
    fn read(
        &self,
        _params: ParseTransformEachParams,
        _ctx: &Context,
    ) -> impl Future<Output = PluginResult<ParseTransformEachResult>> + Send {
        async { Ok(ParseTransformEachResult::default()) }
    }

    /// Generates documents from everything collected (a map from each
    /// document's source to its collected data), once between the two
    /// passes. They join the site: the apply pass, later transformers and
    /// rendering see them like any other document. Only called if
    /// [`Transformer::READS`] is true; none by default.
    fn conclude(
        &self,
        _collected: &Value,
        _ctx: &Context,
    ) -> impl Future<Output = PluginResult<ParseTransformAllResult>> + Send {
        async { Ok(ParseTransformAllResult::default()) }
    }

    /// Transforms one document.
    fn apply(
        &self,
        params: RenderTransformEachParams,
        ctx: &Context,
    ) -> impl Future<Output = PluginResult<RenderTransformEachResult>> + Send;
}

/// Turns documents into output files.
pub trait Renderer: Send + Sync + 'static {
    /// Output formats produced, e.g. `["html"]`.
    fn formats(&self) -> Vec<String>;

    /// Whether to handle `render/transform/all`. If true,
    /// [`Renderer::render_all`] is called once after every document is
    /// rendered.
    const RENDERS_ALL: bool = false;

    /// Renders one document.
    fn render(
        &self,
        params: RenderParams,
        ctx: &Context,
    ) -> impl Future<Output = PluginResult<RenderResult>> + Send;

    /// Produces site-wide files, such as a sitemap, from the list of
    /// rendered pages. Only called if [`Renderer::RENDERS_ALL`] is true.
    fn render_all(
        &self,
        _params: RenderTransformAllParams,
        _ctx: &Context,
    ) -> impl Future<Output = PluginResult<RenderTransformAllResult>> + Send {
        async { Ok(RenderTransformAllResult::default()) }
    }
}

// Object-safe versions of the traits, so a Plugin can hold any of them.

trait DynParser: Send + Sync {
    fn extensions(&self) -> Vec<String>;
    fn parse<'a>(
        &'a self,
        params: ParseParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<ParseResult>>;
}

impl<T: Parser> DynParser for T {
    fn extensions(&self) -> Vec<String> {
        Parser::extensions(self)
    }
    fn parse<'a>(
        &'a self,
        params: ParseParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<ParseResult>> {
        Box::pin(Parser::parse(self, params, ctx))
    }
}

trait DynTransformer: Send + Sync {
    fn reads(&self) -> bool;
    fn nodes(&self) -> Vec<String>;
    fn read<'a>(
        &'a self,
        params: ParseTransformEachParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<ParseTransformEachResult>>;
    fn conclude<'a>(
        &'a self,
        collected: &'a Value,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<ParseTransformAllResult>>;
    fn apply<'a>(
        &'a self,
        params: RenderTransformEachParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderTransformEachResult>>;
}

impl<T: Transformer> DynTransformer for T {
    fn reads(&self) -> bool {
        T::READS
    }
    fn nodes(&self) -> Vec<String> {
        Transformer::nodes(self)
    }
    fn read<'a>(
        &'a self,
        params: ParseTransformEachParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<ParseTransformEachResult>> {
        Box::pin(Transformer::read(self, params, ctx))
    }
    fn conclude<'a>(
        &'a self,
        collected: &'a Value,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<ParseTransformAllResult>> {
        Box::pin(Transformer::conclude(self, collected, ctx))
    }
    fn apply<'a>(
        &'a self,
        params: RenderTransformEachParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderTransformEachResult>> {
        Box::pin(Transformer::apply(self, params, ctx))
    }
}

trait DynRenderer: Send + Sync {
    fn formats(&self) -> Vec<String>;
    fn renders_all(&self) -> bool;
    fn render<'a>(
        &'a self,
        params: RenderParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderResult>>;
    fn render_all<'a>(
        &'a self,
        params: RenderTransformAllParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderTransformAllResult>>;
}

impl<T: Renderer> DynRenderer for T {
    fn formats(&self) -> Vec<String> {
        Renderer::formats(self)
    }
    fn renders_all(&self) -> bool {
        T::RENDERS_ALL
    }
    fn render<'a>(
        &'a self,
        params: RenderParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderResult>> {
        Box::pin(Renderer::render(self, params, ctx))
    }
    fn render_all<'a>(
        &'a self,
        params: RenderTransformAllParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderTransformAllResult>> {
        Box::pin(Renderer::render_all(self, params, ctx))
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// A plugin: a name, a version, and any of a parser, a transformer and a
/// renderer.
pub struct Plugin {
    name: String,
    version: String,
    parser: Option<Box<dyn DynParser>>,
    transformer: Option<Box<dyn DynTransformer>>,
    renderer: Option<Box<dyn DynRenderer>>,
    max_concurrency: Option<u32>,
    cache_key: Option<CacheKey>,
    syntax: Option<SyntaxFn>,
    settings: Option<SettingsFn>,
}

/// Computes a plugin's cache key from its session's setup.
type CacheKey = Box<dyn Fn(&Setup) -> Option<String> + Send + Sync>;

/// Computes the syntax a plugin declares from its session's setup.
type SyntaxFn = Box<dyn Fn(&Setup) -> Syntax + Send + Sync>;

/// Reads a plugin's settings from `.config/`.
type SettingsFn = Box<dyn Fn(&ConfigInfo) -> Result<config::Loaded, String> + Send + Sync>;

/// A session's setup: the `initialize` params and the settings the
/// plugin's loader read (see [`Plugin::settings`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Setup {
    /// The `initialize` params.
    pub params: InitializeParams,
    /// The plugin's settings (null if it reads none).
    pub settings: Value,
    /// The files they came from, for the host's cache keys.
    pub config_files: Vec<String>,
}

impl Setup {
    /// A setup with these settings, as if a loader had read them (for
    /// tests, and plugins built without a loader).
    pub fn with_settings(params: InitializeParams, settings: Value) -> Self {
        Self {
            params,
            settings,
            config_files: Vec::new(),
        }
    }
}

impl fmt::Debug for Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugin")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("capabilities", &self.capabilities())
            .finish()
    }
}

impl Plugin {
    /// A plugin with no capabilities yet.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            parser: None,
            transformer: None,
            renderer: None,
            max_concurrency: None,
            cache_key: None,
            syntax: None,
            settings: None,
        }
    }

    /// Adds a parser.
    pub fn parser(mut self, parser: impl Parser) -> Self {
        self.parser = Some(Box::new(parser));
        self
    }

    /// Adds a transformer.
    pub fn transformer(mut self, transformer: impl Transformer) -> Self {
        self.transformer = Some(Box::new(transformer));
        self
    }

    /// Adds a renderer.
    pub fn renderer(mut self, renderer: impl Renderer) -> Self {
        self.renderer = Some(Box::new(renderer));
        self
    }

    /// Limits how many requests the plugin works on at once.
    pub fn max_concurrency(mut self, n: u32) -> Self {
        self.max_concurrency = Some(n);
        self
    }

    /// Lets the host cache the plugin's results: they depend only on each
    /// request, the site settings, the plugin's name and version and its
    /// settings files. See [`Capabilities::cache`].
    pub fn cacheable(self) -> Self {
        self.cache_key(|_| Some(String::new()))
    }

    /// Lets the host cache the plugin's results, as [`Plugin::cacheable`],
    /// with a key covering any other input the plugin reads, computed from
    /// the session's setup (e.g. a digest of a template file named in the
    /// settings). Returning `None` turns caching off for the session.
    pub fn cache_key(
        mut self,
        key: impl Fn(&Setup) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.cache_key = Some(Box::new(key));
        self
    }

    /// Declares the directives and roles the plugin handles (see
    /// [`Capabilities::syntax`]), computed from the session's setup so
    /// they may depend on the plugin's settings.
    pub fn syntax(mut self, syntax: impl Fn(&Setup) -> Syntax + Send + Sync + 'static) -> Self {
        self.syntax = Some(Box::new(syntax));
        self
    }

    /// Reads the plugin's settings at `initialize`, from the site's
    /// `.config/` directory (see [`config`]). The settings are
    /// [`Context::settings`] and [`Setup::settings`]; an error fails
    /// `initialize`.
    pub fn settings(
        mut self,
        loader: impl Fn(&ConfigInfo) -> Result<config::Loaded, String> + Send + Sync + 'static,
    ) -> Self {
        self.settings = Some(Box::new(loader));
        self
    }

    /// The setup of the session `initialize` starts: its params and the
    /// settings the plugin's loader reads.
    pub fn setup(&self, params: &InitializeParams) -> Result<Setup, String> {
        let loaded = match &self.settings {
            Some(loader) => loader(&params.config)?,
            None => config::Loaded::default(),
        };
        Ok(Setup {
            params: params.clone(),
            settings: loaded.settings,
            config_files: loaded.files,
        })
    }

    /// The plugin's name and version.
    pub fn info(&self) -> ProgramInfo {
        ProgramInfo {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    /// What the plugin can do in the session `initialize` starts: its
    /// [`Plugin::capabilities`] plus its cache key and syntax.
    pub fn capabilities_for(&self, setup: &Setup) -> Capabilities {
        Capabilities {
            cache: self.cache_key.as_ref().and_then(|key| key(setup)),
            syntax: self
                .syntax
                .as_ref()
                .map(|syntax| syntax(setup))
                .unwrap_or_default(),
            ..self.capabilities()
        }
    }

    /// What the plugin can do, derived from the parts it has (without a
    /// cache key or syntax, which depend on the session; see
    /// [`Plugin::capabilities_for`]).
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            parse: self.parser.as_ref().map(|p| ParseCapability {
                extensions: p.extensions(),
            }),
            transform: self.transformer.as_ref().map(|t| TransformCapability {
                nodes: t.nodes(),
                each: t.reads(),
            }),
            render: self.renderer.as_ref().map(|r| RenderCapability {
                formats: r.formats(),
                all: r.renders_all(),
            }),
            max_concurrency: self.max_concurrency,
            cache: None,
            syntax: Syntax::default(),
        }
    }

    /// The answer to `initialize`.
    pub fn initialize_result(&self, setup: &Setup) -> InitializeResult {
        InitializeResult {
            protocol_version: PROTOCOL_VERSION.into(),
            plugin: self.info(),
            capabilities: self.capabilities_for(setup),
            config_files: setup.config_files.clone(),
        }
    }

    /// Parses a file. Fails if the plugin has no parser.
    pub async fn parse(&self, params: ParseParams, ctx: &Context) -> PluginResult<ParseResult> {
        self.parser
            .as_ref()
            .ok_or_else(|| missing("parser"))?
            .parse(params, ctx)
            .await
    }

    /// Runs the collect pass on a document. Fails if the plugin has no
    /// transformer.
    pub async fn read(
        &self,
        params: ParseTransformEachParams,
        ctx: &Context,
    ) -> PluginResult<ParseTransformEachResult> {
        self.transformer
            .as_ref()
            .ok_or_else(|| missing("transformer"))?
            .read(params, ctx)
            .await
    }

    /// Generates documents from everything collected. Fails if the plugin
    /// has no transformer.
    pub async fn conclude(
        &self,
        collected: &Value,
        ctx: &Context,
    ) -> PluginResult<ParseTransformAllResult> {
        self.transformer
            .as_ref()
            .ok_or_else(|| missing("transformer"))?
            .conclude(collected, ctx)
            .await
    }

    /// Transforms a document. Fails if the plugin has no transformer.
    pub async fn apply(
        &self,
        params: RenderTransformEachParams,
        ctx: &Context,
    ) -> PluginResult<RenderTransformEachResult> {
        self.transformer
            .as_ref()
            .ok_or_else(|| missing("transformer"))?
            .apply(params, ctx)
            .await
    }

    /// Renders a document. Fails if the plugin has no renderer.
    pub async fn render(&self, params: RenderParams, ctx: &Context) -> PluginResult<RenderResult> {
        self.renderer
            .as_ref()
            .ok_or_else(|| missing("renderer"))?
            .render(params, ctx)
            .await
    }

    /// Produces site-wide files after rendering. Fails if the plugin has no
    /// renderer.
    pub async fn render_all(
        &self,
        params: RenderTransformAllParams,
        ctx: &Context,
    ) -> PluginResult<RenderTransformAllResult> {
        self.renderer
            .as_ref()
            .ok_or_else(|| missing("renderer"))?
            .render_all(params, ctx)
            .await
    }
}

fn missing(part: &str) -> PluginError {
    PluginError::new(format!("this plugin has no {part}"))
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Something a plugin reports to the host outside of a result.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A log line.
    Log(LogParams),
    /// Progress on long-running work.
    Progress(ProgressParams),
    /// Diagnostics.
    Diagnostics(DiagnosticsParams),
}

/// Where a [`Context`] sends [`Event`]s and requests to the host: the RPC
/// connection, or the host directly for in-process plugins.
pub trait EventSink: Send + Sync + 'static {
    /// Delivers one event.
    fn send(&self, event: Event) -> BoxFuture<'_, ()>;

    /// Asks the host to parse text with the parser of a file extension
    /// (`host/parse`). The default says it cannot.
    fn parse(&self, params: HostParseParams) -> BoxFuture<'_, Result<ParseResult, String>> {
        let _ = params;
        Box::pin(async { Err("parsing through the host is not available here".to_owned()) })
    }
}

/// What a plugin operation can see and do besides its params: the site, the
/// plugin's options, and reporting to the host.
#[derive(Clone)]
pub struct Context {
    events: Arc<dyn EventSink>,
    site: Arc<SiteInfo>,
    config: Arc<ConfigInfo>,
    settings: Arc<Value>,
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("site", &self.site)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl Context {
    /// A context for the session started by `initialize`.
    pub fn new(events: Arc<dyn EventSink>, setup: &Setup) -> Self {
        Self {
            events,
            site: Arc::new(setup.params.site.clone()),
            config: Arc::new(setup.params.config.clone()),
            settings: Arc::new(setup.settings.clone()),
        }
    }

    /// The site being built.
    pub fn site(&self) -> &SiteInfo {
        &self.site
    }

    /// Where the plugin's settings are: the site's `.config/` directory
    /// and the plugin's name in the site config.
    pub fn config(&self) -> &ConfigInfo {
        &self.config
    }

    /// The plugin's settings, as its loader read them (null if none; see
    /// [`Plugin::settings`]).
    pub fn settings(&self) -> &Value {
        &self.settings
    }

    /// Parses `text` with the site's parser for files with `extension`
    /// (`"md"`, `"rst"`, ...), as part of the document `source`: e.g. a doc
    /// comment. The parser gets the site's syntax extensions; the result is
    /// not normalized (no generated ids or link targets).
    pub async fn parse(
        &self,
        extension: impl Into<String>,
        source: impl Into<String>,
        text: impl Into<String>,
    ) -> PluginResult<ParseResult> {
        self.events
            .parse(HostParseParams {
                extension: extension.into(),
                source: source.into(),
                text: text.into(),
            })
            .await
            .map_err(PluginError::new)
    }

    /// Sends a log line to the host.
    pub async fn log(&self, level: LogLevel, message: impl Into<String>) {
        self.events
            .send(Event::Log(LogParams {
                level,
                message: message.into(),
            }))
            .await;
    }

    /// Logs at debug level.
    pub async fn debug(&self, message: impl Into<String>) {
        self.log(LogLevel::Debug, message).await;
    }

    /// Logs at info level.
    pub async fn info(&self, message: impl Into<String>) {
        self.log(LogLevel::Info, message).await;
    }

    /// Logs at warning level.
    pub async fn warn(&self, message: impl Into<String>) {
        self.log(LogLevel::Warn, message).await;
    }

    /// Reports progress.
    pub async fn progress(&self, message: impl Into<String>, percentage: Option<u8>) {
        self.events
            .send(Event::Progress(ProgressParams {
                message: message.into(),
                percentage,
            }))
            .await;
    }

    /// Reports a diagnostic outside of any result.
    pub async fn diagnostic(&self, diagnostic: Diagnostic) {
        self.events
            .send(Event::Diagnostics(DiagnosticsParams {
                diagnostics: vec![diagnostic],
            }))
            .await;
    }
}

/// Sends events to the host as notifications.
struct PeerSink(Peer);

impl EventSink for PeerSink {
    fn send(&self, event: Event) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // If the connection is gone there is no one left to tell.
            let _ = match event {
                Event::Log(p) => self.0.notify::<notification::Log>(&p).await,
                Event::Progress(p) => self.0.notify::<notification::Progress>(&p).await,
                Event::Diagnostics(p) => self.0.notify::<notification::Diagnostics>(&p).await,
            };
        })
    }

    fn parse(&self, params: HostParseParams) -> BoxFuture<'_, Result<ParseResult, String>> {
        Box::pin(async move {
            self.0
                .request::<request::HostParse>(&params)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A relative URL from the document `from` to the file `to`, both given
/// relative to the source directory with `/` separators: e.g. from
/// `guide/a.md` to `index.md` is `../index.md`.
pub fn relative_path(from: &str, to: &str) -> String {
    let from_parts: Vec<&str> = from.split('/').collect();
    let from_dir = &from_parts[..from_parts.len() - 1];
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dir
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = "../".repeat(from_dir.len() - common);
    out.push_str(&to_parts[common..].join("/"));
    out
}

// ---------------------------------------------------------------------------
// Testing
// ---------------------------------------------------------------------------

/// Helpers for testing plugins without a host.
pub mod testing {
    use super::*;
    use lsdocs_protocol::doctree::Document;

    struct Discard;

    impl EventSink for Discard {
        fn send(&self, _event: Event) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    /// A context for `initialize` whose events (logs, progress, notified
    /// diagnostics) are dropped.
    pub fn context(setup: &Setup) -> Context {
        Context::new(Arc::new(Discard), setup)
    }

    /// The setup `plugin` gets for `params`: its loader reads its settings
    /// from `params.config`. Panics if they cannot be read.
    #[allow(clippy::panic)]
    pub fn setup(plugin: &Plugin, params: InitializeParams) -> Setup {
        plugin
            .setup(&params)
            .unwrap_or_else(|e| panic!("cannot read the settings: {e}"))
    }

    /// Answers `host/parse` with the given parser plugins, as the host
    /// would; events are dropped.
    struct WithParsers {
        parsers: Vec<Plugin>,
        syntax: Syntax,
        setup: Setup,
    }

    impl EventSink for WithParsers {
        fn send(&self, _event: Event) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }

        fn parse(&self, params: HostParseParams) -> BoxFuture<'_, Result<ParseResult, String>> {
            Box::pin(async move {
                let parser = self
                    .parsers
                    .iter()
                    .find(|p| {
                        p.capabilities().parse.is_some_and(|cap| {
                            cap.extensions
                                .iter()
                                .any(|e| e.eq_ignore_ascii_case(&params.extension))
                        })
                    })
                    .ok_or_else(|| format!("no parser takes `.{}` files", params.extension))?;
                let ctx = context(&self.setup);
                parser
                    .parse(
                        ParseParams {
                            source: params.source,
                            text: params.text,
                            syntax: self.syntax.clone(),
                        },
                        &ctx,
                    )
                    .await
                    .map_err(|e| e.to_string())
            })
        }
    }

    /// A context for `initialize` whose `host/parse` requests go to
    /// `parsers` (the first taking the extension), with the syntax that
    /// `plugin` and the parsers declare, as the host would give them.
    pub fn context_with_parsers(plugin: &Plugin, setup: &Setup, parsers: Vec<Plugin>) -> Context {
        let mut syntax = plugin.capabilities_for(setup).syntax;
        for parser in &parsers {
            syntax.merge(&parser.capabilities_for(setup).syntax);
        }
        let sink = WithParsers {
            parsers,
            syntax,
            setup: setup.clone(),
        };
        Context::new(Arc::new(sink), setup)
    }

    /// Runs `plugin`'s transformer over `documents` the way the host does:
    /// the collect pass over every document (if it collects), then the
    /// documents it generates, then the apply pass with everything
    /// collected, generated documents included. Returns each transformed
    /// document with the diagnostics its apply pass reported. Unlike the
    /// host, every document is passed, whatever node kinds it contains.
    pub async fn run_transform(
        plugin: &Plugin,
        setup: &Setup,
        documents: Vec<Document>,
    ) -> PluginResult<Vec<(Document, Vec<Diagnostic>)>> {
        run_transform_in(plugin, &context(setup), documents).await
    }

    /// [`run_transform`] with the context `ctx` (e.g. from
    /// [`context_with_parsers`]).
    pub async fn run_transform_in(
        plugin: &Plugin,
        ctx: &Context,
        mut documents: Vec<Document>,
    ) -> PluginResult<Vec<(Document, Vec<Diagnostic>)>> {
        let ctx = ctx.clone();
        let collects = plugin.capabilities().transform.is_some_and(|t| t.each);
        let mut collected = Value::Null;
        if collects {
            let mut data = serde_json::Map::new();
            for document in &documents {
                let result = plugin
                    .read(
                        ParseTransformEachParams {
                            document: document.clone(),
                        },
                        &ctx,
                    )
                    .await?;
                data.insert(document.source.clone(), result.data);
            }
            collected = Value::Object(data);
            let generated = plugin.conclude(&collected, &ctx).await?;
            documents.extend(generated.documents);
        }
        let mut out = Vec::with_capacity(documents.len());
        for document in documents {
            let result = plugin
                .apply(
                    RenderTransformEachParams {
                        document,
                        collected: collected.clone(),
                    },
                    &ctx,
                )
                .await?;
            out.push((result.document, result.diagnostics));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------

/// Runs `plugin` until the host is done with it, then returns an exit code.
/// Use as the body of a plugin's `main`.
pub fn main(plugin: Plugin) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("{}: failed to start: {e}", plugin.name);
            return ExitCode::FAILURE;
        }
    };
    let name = plugin.name.clone();
    match runtime.block_on(serve(plugin)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Serves `plugin` over the pipe named in `LSDOCS_PIPE` if set, otherwise
/// over stdio. Must run inside a tokio runtime.
pub async fn serve(plugin: Plugin) -> Result<(), lsdocs_rpc::Error> {
    let transport = if std::env::var_os(pipe::ENV_VAR).is_some() {
        pipe::connect_from_env().await
    } else {
        transport::stdio()
    }
    .map_err(lsdocs_rpc::Error::Io)?;
    serve_on(plugin, transport).await
}

/// Serves `plugin` over a given transport until the host sends `exit` or
/// closes the connection.
pub async fn serve_on(plugin: Plugin, transport: Transport) -> Result<(), lsdocs_rpc::Error> {
    let options = Options {
        max_concurrency: plugin.max_concurrency.map(|n| n as usize),
        ..Options::default()
    };
    let (_peer, connection) = lsdocs_rpc::connect(transport, router(Arc::new(plugin)), options);
    connection.run().await
}

fn not_initialized() -> RpcError {
    RpcError::new(
        ErrorCode::INVALID_REQUEST,
        "`initialize` has not been called",
    )
}

fn plugin_error(e: PluginError) -> RpcError {
    RpcError::internal(e.message)
}

/// Routes protocol requests to `plugin`. Phase requests are only routed for
/// the parts the plugin has, so others are answered with "method not
/// found".
fn router(plugin: Arc<Plugin>) -> Router {
    let session: Arc<OnceLock<Context>> = Arc::new(OnceLock::new());

    let init_session = session.clone();
    let init_plugin = plugin.clone();
    let mut router = Router::new()
        .request::<request::Initialize, _, _>(move |params, peer| {
            let result = match init_plugin.setup(&params) {
                Err(message) => Err(RpcError::internal(message)),
                Ok(setup) => {
                    if init_session
                        .set(Context::new(Arc::new(PeerSink(peer)), &setup))
                        .is_ok()
                    {
                        Ok(init_plugin.initialize_result(&setup))
                    } else {
                        Err(RpcError::new(
                            ErrorCode::INVALID_REQUEST,
                            "already initialized",
                        ))
                    }
                }
            };
            async move { result }
        })
        .notification::<notification::Initialized, _>(|(), _| {})
        .request::<request::Shutdown, _, _>(|(), _| async { Ok(()) })
        .notification::<notification::Exit, _>(|(), peer| peer.close());

    // Each phase handler: look up the session, then call the plugin.
    macro_rules! route {
        ($req:ty, $method:ident) => {{
            let plugin = plugin.clone();
            let session = session.clone();
            router = router.request::<$req, _, _>(move |params, _| {
                let plugin = plugin.clone();
                let ctx = session.get().cloned();
                async move {
                    let ctx = ctx.ok_or_else(not_initialized)?;
                    plugin.$method(params, &ctx).await.map_err(plugin_error)
                }
            });
        }};
    }

    if plugin.parser.is_some() {
        route!(request::Parse, parse);
    }
    if plugin.transformer.is_some() {
        route!(request::ParseTransformEach, read);
        // The collected data arrives once, not with every apply request;
        // keep it and fill it into each one.
        let collected: Arc<std::sync::Mutex<Arc<Value>>> = Arc::default();
        let store = collected.clone();
        let generator = plugin.clone();
        let generate_session = session.clone();
        router = router.request::<request::ParseTransformAll, _, _>(move |params, _| {
            let data = Arc::new(params.collected);
            *store.lock().unwrap_or_else(PoisonError::into_inner) = Arc::clone(&data);
            let plugin = generator.clone();
            let ctx = generate_session.get().cloned();
            async move {
                let ctx = ctx.ok_or_else(not_initialized)?;
                plugin.conclude(&data, &ctx).await.map_err(plugin_error)
            }
        });
        let plugin = plugin.clone();
        let session = session.clone();
        router = router.request::<request::RenderTransformEach, _, _>(move |mut params, _| {
            let plugin = plugin.clone();
            let ctx = session.get().cloned();
            if params.collected.is_null() {
                params.collected =
                    (**collected.lock().unwrap_or_else(PoisonError::into_inner)).clone();
            }
            async move {
                let ctx = ctx.ok_or_else(not_initialized)?;
                plugin.apply(params, &ctx).await.map_err(plugin_error)
            }
        });
    }
    if plugin.renderer.is_some() {
        route!(request::Render, render);
        route!(request::RenderTransformAll, render_all);
    }
    router
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path("index.md", "guide/a.md"), "guide/a.md");
        assert_eq!(relative_path("guide/a.md", "index.md"), "../index.md");
        assert_eq!(relative_path("guide/a.md", "guide/b.md"), "b.md");
        assert_eq!(relative_path("guide/x/a.md", "ref/b.md"), "../../ref/b.md");
        assert_eq!(relative_path("a.md", "a.md"), "a.md");
    }
    use lsdocs_protocol::doctree::{Document, text};
    use lsdocs_rpc::Error;
    use std::sync::Mutex;

    struct Upper;

    impl Transformer for Upper {
        const READS: bool = true;

        fn nodes(&self) -> Vec<String> {
            vec!["text".into()]
        }

        async fn read(
            &self,
            params: ParseTransformEachParams,
            _ctx: &Context,
        ) -> PluginResult<ParseTransformEachResult> {
            Ok(ParseTransformEachResult {
                data: serde_json::json!(params.document.children.len()),
                diagnostics: vec![],
                dependencies: vec![],
            })
        }

        async fn apply(
            &self,
            params: RenderTransformEachParams,
            ctx: &Context,
        ) -> PluginResult<RenderTransformEachResult> {
            ctx.info(format!("site is {:?}", ctx.site().title)).await;
            let mut document = params.document;
            for node in &mut document.children {
                if let doctree::Node::Text(t) = node {
                    t.value = t.value.to_uppercase();
                }
            }
            Ok(RenderTransformEachResult {
                document,
                diagnostics: vec![],
                dependencies: vec![],
            })
        }
    }

    struct Failing;

    impl Parser for Failing {
        fn extensions(&self) -> Vec<String> {
            vec!["txt".into()]
        }

        async fn parse(&self, _params: ParseParams, _ctx: &Context) -> PluginResult<ParseResult> {
            let n: Result<u8, _> = "not a number".parse();
            n?;
            unreachable!()
        }
    }

    fn plugin() -> Plugin {
        Plugin::new("upper", "1.0.0")
            .transformer(Upper)
            .parser(Failing)
    }

    fn init_params() -> crate::Setup {
        crate::Setup::with_settings(
            InitializeParams {
                protocol_version: PROTOCOL_VERSION.into(),
                host: ProgramInfo {
                    name: "test".into(),
                    version: "0".into(),
                },
                site: SiteInfo {
                    title: Some("Docs".into()),
                    ..SiteInfo::default()
                },
                config: Default::default(),
            },
            Value::Null,
        )
    }

    fn doc() -> Document {
        let mut doc = Document::new("a.txt");
        doc.children.push(text("hello"));
        doc
    }

    #[test]
    fn capabilities_follow_the_parts() {
        let caps = plugin().capabilities();
        assert_eq!(caps.parse.unwrap().extensions, ["txt"]);
        let transform = caps.transform.unwrap();
        assert_eq!(transform.nodes, ["text"]);
        assert!(transform.each);
        assert!(caps.render.is_none());
        assert_eq!(
            Plugin::new("empty", "0").capabilities(),
            Capabilities::default()
        );
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Event>>);

    impl EventSink for Recorder {
        fn send(&self, event: Event) -> BoxFuture<'_, ()> {
            self.0.lock().unwrap().push(event);
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn in_process_calls_go_straight_to_the_parts() {
        let recorder = Arc::new(Recorder::default());
        let ctx = Context::new(recorder.clone(), &init_params());
        let plugin = plugin();
        let result = plugin
            .apply(
                RenderTransformEachParams {
                    document: doc(),
                    collected: Value::Null,
                },
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(result.document.children, [text("HELLO")]);
        assert_eq!(
            recorder.0.lock().unwrap().as_slice(),
            [Event::Log(LogParams {
                level: LogLevel::Info,
                message: "site is Some(\"Docs\")".into()
            })]
        );
        let err = plugin
            .render(
                RenderParams {
                    document: doc(),
                    format: "html".into(),
                },
                &ctx,
            )
            .await
            .unwrap_err();
        assert_eq!(err.message(), "this plugin has no renderer");
    }

    #[tokio::test]
    async fn served_over_rpc() {
        let (host_side, plugin_side) = transport::memory();
        let server = tokio::spawn(serve_on(plugin(), plugin_side));
        let (logs_tx, mut logs) = tokio::sync::mpsc::unbounded_channel();
        let router = Router::new().notification::<notification::Log, _>(move |p, _| {
            let _ = logs_tx.send(p.message);
        });
        let (host, connection) = lsdocs_rpc::connect(host_side, router, Options::default());
        let client = tokio::spawn(connection.run());

        // Phase requests before initialize are refused.
        let early = host
            .request::<request::RenderTransformEach>(&RenderTransformEachParams {
                document: doc(),
                collected: Value::Null,
            })
            .await;
        assert!(
            matches!(&early, Err(Error::Remote(e)) if e.code == ErrorCode::INVALID_REQUEST),
            "{early:?}"
        );

        let init = host
            .request::<request::Initialize>(&init_params().params)
            .await
            .unwrap();
        assert_eq!(init.plugin.name, "upper");
        assert_eq!(init.capabilities, plugin().capabilities());
        host.notify::<notification::Initialized>(&()).await.unwrap();

        let collected = host
            .request::<request::ParseTransformEach>(&ParseTransformEachParams { document: doc() })
            .await
            .unwrap();
        assert_eq!(collected.data, serde_json::json!(1));

        let applied = host
            .request::<request::RenderTransformEach>(&RenderTransformEachParams {
                document: doc(),
                collected: Value::Null,
            })
            .await
            .unwrap();
        assert_eq!(applied.document.children, [text("HELLO")]);
        assert_eq!(logs.recv().await.unwrap(), "site is Some(\"Docs\")");

        // A plugin error becomes an internal error with its message.
        let failed = host
            .request::<request::Parse>(&ParseParams {
                source: "a.txt".into(),
                text: String::new(),
                ..ParseParams::default()
            })
            .await;
        assert!(
            matches!(&failed, Err(Error::Remote(e)) if e.code == ErrorCode::INTERNAL_ERROR && e.message == "invalid digit found in string"),
            "{failed:?}"
        );

        // Phases the plugin lacks are not routed.
        let render = host
            .request::<request::Render>(&RenderParams {
                document: doc(),
                format: "html".into(),
            })
            .await;
        assert!(
            matches!(&render, Err(Error::Remote(e)) if e.code == ErrorCode::METHOD_NOT_FOUND),
            "{render:?}"
        );

        host.request::<request::Shutdown>(&()).await.unwrap();
        host.notify::<notification::Exit>(&()).await.unwrap();
        server.await.unwrap().unwrap();
        client.await.unwrap().unwrap();
    }
}
