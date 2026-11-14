//! Write lsdocs plugins in Rust.
//!
//! Implement one or more of [`Parser`], [`Transformer`] and [`Renderer`],
//! bundle them into a [`Plugin`], and hand it to [`main`]:
//!
//! ```no_run
//! use lsdocs_plugin_sdk::{Context, Plugin, PluginResult, Transformer};
//! use lsdocs_plugin_sdk::protocol::{TransformApplyParams, TransformApplyResult};
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
//!         params: TransformApplyParams,
//!         ctx: &Context,
//!     ) -> PluginResult<TransformApplyResult> {
//!         ctx.info("shouting").await;
//!         Ok(TransformApplyResult { document: params.document, diagnostics: vec![] })
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
use std::sync::{Arc, OnceLock};

pub use lsdocs_protocol as protocol;
pub use lsdocs_protocol::doctree;
use lsdocs_protocol::doctree::Value;
use lsdocs_protocol::{
    Capabilities, Diagnostic, DiagnosticsParams, InitializeParams, InitializeResult, LogLevel,
    LogParams, PROTOCOL_VERSION, ParseCapability, ParseParams, ParseResult, ProgramInfo,
    ProgressParams, RenderCapability, RenderParams, RenderResult, SiteInfo, TransformApplyParams,
    TransformApplyResult, TransformCapability, TransformCollectParams, TransformCollectResult,
    notification, request,
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
    /// Whether to run the collect pass. If true, [`Transformer::collect`] is
    /// called for every document before any [`Transformer::apply`], and
    /// `apply` receives everything collected.
    const COLLECTS: bool = false;

    /// Node kinds this transformer looks at, e.g. `["heading"]`. The host
    /// skips documents containing none of them. Empty means every document.
    fn nodes(&self) -> Vec<String> {
        Vec::new()
    }

    /// Gathers data from one document. Only called if
    /// [`Transformer::COLLECTS`] is true.
    fn collect(
        &self,
        _params: TransformCollectParams,
        _ctx: &Context,
    ) -> impl Future<Output = PluginResult<TransformCollectResult>> + Send {
        async { Ok(TransformCollectResult::default()) }
    }

    /// Transforms one document.
    fn apply(
        &self,
        params: TransformApplyParams,
        ctx: &Context,
    ) -> impl Future<Output = PluginResult<TransformApplyResult>> + Send;
}

/// Turns documents into output files.
pub trait Renderer: Send + Sync + 'static {
    /// Output formats produced, e.g. `["html"]`.
    fn formats(&self) -> Vec<String>;

    /// Renders one document.
    fn render(
        &self,
        params: RenderParams,
        ctx: &Context,
    ) -> impl Future<Output = PluginResult<RenderResult>> + Send;
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
    fn collects(&self) -> bool;
    fn nodes(&self) -> Vec<String>;
    fn collect<'a>(
        &'a self,
        params: TransformCollectParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<TransformCollectResult>>;
    fn apply<'a>(
        &'a self,
        params: TransformApplyParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<TransformApplyResult>>;
}

impl<T: Transformer> DynTransformer for T {
    fn collects(&self) -> bool {
        T::COLLECTS
    }
    fn nodes(&self) -> Vec<String> {
        Transformer::nodes(self)
    }
    fn collect<'a>(
        &'a self,
        params: TransformCollectParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<TransformCollectResult>> {
        Box::pin(Transformer::collect(self, params, ctx))
    }
    fn apply<'a>(
        &'a self,
        params: TransformApplyParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<TransformApplyResult>> {
        Box::pin(Transformer::apply(self, params, ctx))
    }
}

trait DynRenderer: Send + Sync {
    fn formats(&self) -> Vec<String>;
    fn render<'a>(
        &'a self,
        params: RenderParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderResult>>;
}

impl<T: Renderer> DynRenderer for T {
    fn formats(&self) -> Vec<String> {
        Renderer::formats(self)
    }
    fn render<'a>(
        &'a self,
        params: RenderParams,
        ctx: &'a Context,
    ) -> BoxFuture<'a, PluginResult<RenderResult>> {
        Box::pin(Renderer::render(self, params, ctx))
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

    /// The plugin's name and version.
    pub fn info(&self) -> ProgramInfo {
        ProgramInfo {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    /// What the plugin can do, derived from the parts it has.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            parse: self.parser.as_ref().map(|p| ParseCapability {
                extensions: p.extensions(),
            }),
            transform: self.transformer.as_ref().map(|t| TransformCapability {
                nodes: t.nodes(),
                collect: t.collects(),
            }),
            render: self.renderer.as_ref().map(|r| RenderCapability {
                formats: r.formats(),
            }),
            max_concurrency: self.max_concurrency,
        }
    }

    /// The answer to `initialize`.
    pub fn initialize_result(&self) -> InitializeResult {
        InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            plugin: self.info(),
            capabilities: self.capabilities(),
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
    pub async fn collect(
        &self,
        params: TransformCollectParams,
        ctx: &Context,
    ) -> PluginResult<TransformCollectResult> {
        self.transformer
            .as_ref()
            .ok_or_else(|| missing("transformer"))?
            .collect(params, ctx)
            .await
    }

    /// Transforms a document. Fails if the plugin has no transformer.
    pub async fn apply(
        &self,
        params: TransformApplyParams,
        ctx: &Context,
    ) -> PluginResult<TransformApplyResult> {
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

/// Where a [`Context`] sends [`Event`]s: the RPC connection, or the host
/// directly for in-process plugins.
pub trait EventSink: Send + Sync + 'static {
    /// Delivers one event.
    fn send(&self, event: Event) -> BoxFuture<'_, ()>;
}

/// What a plugin operation can see and do besides its params: the site, the
/// plugin's options, and reporting to the host.
#[derive(Clone)]
pub struct Context {
    events: Arc<dyn EventSink>,
    site: Arc<SiteInfo>,
    options: Arc<Value>,
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("site", &self.site)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Context {
    /// A context for the session started by `initialize`.
    pub fn new(events: Arc<dyn EventSink>, initialize: &InitializeParams) -> Self {
        Self {
            events,
            site: Arc::new(initialize.site.clone()),
            options: Arc::new(initialize.options.clone()),
        }
    }

    /// The site being built.
    pub fn site(&self) -> &SiteInfo {
        &self.site
    }

    /// The plugin's `options` from the site config (null if none).
    pub fn options(&self) -> &Value {
        &self.options
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
            let result = if init_session
                .set(Context::new(Arc::new(PeerSink(peer)), &params))
                .is_ok()
            {
                Ok(init_plugin.initialize_result())
            } else {
                Err(RpcError::new(
                    ErrorCode::INVALID_REQUEST,
                    "already initialized",
                ))
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
        route!(request::TransformCollect, collect);
        route!(request::TransformApply, apply);
    }
    if plugin.renderer.is_some() {
        route!(request::Render, render);
    }
    router
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsdocs_protocol::doctree::{Document, text};
    use lsdocs_rpc::Error;
    use std::sync::Mutex;

    struct Upper;

    impl Transformer for Upper {
        const COLLECTS: bool = true;

        fn nodes(&self) -> Vec<String> {
            vec!["text".into()]
        }

        async fn collect(
            &self,
            params: TransformCollectParams,
            _ctx: &Context,
        ) -> PluginResult<TransformCollectResult> {
            Ok(TransformCollectResult {
                data: serde_json::json!(params.document.children.len()),
                diagnostics: vec![],
            })
        }

        async fn apply(
            &self,
            params: TransformApplyParams,
            ctx: &Context,
        ) -> PluginResult<TransformApplyResult> {
            ctx.info(format!("site is {:?}", ctx.site().title)).await;
            let mut document = params.document;
            for node in &mut document.children {
                if let doctree::Node::Text(t) = node {
                    t.value = t.value.to_uppercase();
                }
            }
            Ok(TransformApplyResult {
                document,
                diagnostics: vec![],
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

    fn init_params() -> InitializeParams {
        InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            host: ProgramInfo {
                name: "test".into(),
                version: "0".into(),
            },
            site: SiteInfo {
                title: Some("Docs".into()),
                base_url: None,
            },
            options: Value::Null,
        }
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
        assert!(transform.collect);
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
                TransformApplyParams {
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
            .request::<request::TransformApply>(&TransformApplyParams {
                document: doc(),
                collected: Value::Null,
            })
            .await;
        assert!(
            matches!(&early, Err(Error::Remote(e)) if e.code == ErrorCode::INVALID_REQUEST),
            "{early:?}"
        );

        let init = host
            .request::<request::Initialize>(&init_params())
            .await
            .unwrap();
        assert_eq!(init.plugin.name, "upper");
        assert_eq!(init.capabilities, plugin().capabilities());
        host.notify::<notification::Initialized>(&()).await.unwrap();

        let collected = host
            .request::<request::TransformCollect>(&TransformCollectParams { document: doc() })
            .await
            .unwrap();
        assert_eq!(collected.data, serde_json::json!(1));

        let applied = host
            .request::<request::TransformApply>(&TransformApplyParams {
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
