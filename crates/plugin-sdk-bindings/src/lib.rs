//! Value-based plugin handlers, for language bindings.
//!
//! Bindings (Python, Node) describe a plugin as a [`DynamicPlugin`]: a name,
//! a version, and handlers that take and return JSON-like [`Value`]s. This
//! crate turns that into an SDK [`Plugin`], so the handshake, dispatch,
//! capabilities, cancellation and shutdown all stay in Rust and behave the
//! same in every language.
//!
//! Handlers receive the request params as a value shaped like the protocol
//! schema (e.g. `{"source": ..., "text": ...}` for `parse`) and return the
//! result the same way. For convenience they may also return less:
//!
//! | Handler | May return instead of the full result |
//! |---|---|
//! | parse, apply | the document alone (an object with `children` but no `document`) |
//! | collect | any value, taken as `data` |
//! | render | a string, taken as `output`, with `extension` set to the format |
//! | render/site | a list, taken as `files` |
//!
//! Binary output (`output` of `render` and of each `render/site` file) may be
//! a string or bytes; bindings pass bytes as a list of byte values.

use std::fmt;
use std::sync::Arc;

use lsdocs_plugin_sdk::protocol::{
    ParseParams, ParseResult, RenderParams, RenderResult, RenderSiteParams, RenderSiteResult,
    TransformApplyParams, TransformApplyResult, TransformCollectParams, TransformCollectResult,
};
pub use lsdocs_plugin_sdk::{BoxFuture, Context, Event, Plugin};
use lsdocs_plugin_sdk::{Parser, PluginError, PluginResult, Renderer, Transformer};
use serde::Serialize;
use serde::de::DeserializeOwned;
pub use serde_json::Value;

/// A handler: params in, result out, or an error message.
pub type Handler =
    Arc<dyn Fn(Value, Context) -> BoxFuture<'static, Result<Value, String>> + Send + Sync>;

/// A plugin described by value-based handlers.
#[derive(Clone, Default)]
pub struct DynamicPlugin {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Most requests handled at once; bindings to single-threaded runtimes
    /// (Python with the GIL, Node) should set 1.
    pub max_concurrency: Option<u32>,
    /// File extensions and the `parse` handler.
    pub parse: Option<(Vec<String>, Handler)>,
    /// The `transform/collect` handler; only used with `apply`.
    pub collect: Option<Handler>,
    /// Node kinds selected and the `transform/apply` handler.
    pub apply: Option<(Vec<String>, Handler)>,
    /// Output formats and the `render` handler.
    pub render: Option<(Vec<String>, Handler)>,
    /// The `render/site` handler; only used with `render`.
    pub render_site: Option<Handler>,
}

impl fmt::Debug for DynamicPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynamicPlugin")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("parse", &self.parse.as_ref().map(|(e, _)| e))
            .field("collect", &self.collect.is_some())
            .field("apply", &self.apply.as_ref().map(|(n, _)| n))
            .field("render", &self.render.as_ref().map(|(f, _)| f))
            .field("render_site", &self.render_site.is_some())
            .finish()
    }
}

impl DynamicPlugin {
    /// Builds the SDK plugin.
    pub fn into_plugin(self) -> Plugin {
        let mut plugin = Plugin::new(self.name, self.version);
        if let Some(n) = self.max_concurrency {
            plugin = plugin.max_concurrency(n);
        }
        if let Some((extensions, handler)) = self.parse {
            plugin = plugin.parser(DynParser {
                extensions,
                handler,
            });
        }
        if let Some((nodes, apply)) = self.apply {
            plugin = match self.collect {
                Some(collect) => plugin.transformer(Collecting(DynTransformer {
                    nodes,
                    collect: Some(collect),
                    apply,
                })),
                None => plugin.transformer(DynTransformer {
                    nodes,
                    collect: None,
                    apply,
                }),
            };
        }
        if let Some((formats, handler)) = self.render {
            let renderer = DynRenderer { formats, handler };
            plugin = match self.render_site {
                Some(site) => plugin.renderer(SiteRenderer(renderer, site)),
                None => plugin.renderer(renderer),
            };
        }
        plugin
    }
}

/// Calls a handler with typed params and decodes its typed result, after
/// `normalize` has filled in any shorthand.
async fn call<P, R>(
    handler: &Handler,
    method: &str,
    params: &P,
    ctx: &Context,
    normalize: impl FnOnce(Value) -> Value,
) -> PluginResult<R>
where
    P: Serialize,
    R: DeserializeOwned,
{
    let params = serde_json::to_value(params)
        .map_err(|e| PluginError::new(format!("failed to encode {method} params: {e}")))?;
    let result = handler(params, ctx.clone())
        .await
        .map_err(PluginError::new)?;
    serde_json::from_value(normalize(result))
        .map_err(|e| PluginError::new(format!("invalid {method} result: {e}")))
}

/// Wraps a bare document as `{"document": ...}`.
fn wrap_document(value: Value) -> Value {
    match &value {
        Value::Object(map) if !map.contains_key("document") && map.contains_key("children") => {
            serde_json::json!({ "document": value })
        }
        _ => value,
    }
}

struct DynParser {
    extensions: Vec<String>,
    handler: Handler,
}

impl Parser for DynParser {
    fn extensions(&self) -> Vec<String> {
        self.extensions.clone()
    }

    async fn parse(&self, params: ParseParams, ctx: &Context) -> PluginResult<ParseResult> {
        let source = params.source.clone();
        call(&self.handler, "parse", &params, ctx, |v| {
            let mut v = wrap_document(v);
            // The document's source defaults to the file being parsed.
            if let Some(doc) = v.get_mut("document").and_then(Value::as_object_mut) {
                doc.entry("source").or_insert(Value::String(source));
            }
            v
        })
        .await
    }
}

struct DynTransformer {
    nodes: Vec<String>,
    collect: Option<Handler>,
    apply: Handler,
}

impl DynTransformer {
    async fn do_collect(
        &self,
        params: TransformCollectParams,
        ctx: &Context,
    ) -> PluginResult<TransformCollectResult> {
        let Some(handler) = &self.collect else {
            return Ok(TransformCollectResult::default());
        };
        call(handler, "transform/collect", &params, ctx, |v| match &v {
            Value::Object(map) if map.contains_key("data") => v,
            _ => serde_json::json!({ "data": v }),
        })
        .await
    }

    async fn do_apply(
        &self,
        params: TransformApplyParams,
        ctx: &Context,
    ) -> PluginResult<TransformApplyResult> {
        call(&self.apply, "transform/apply", &params, ctx, wrap_document).await
    }
}

impl Transformer for DynTransformer {
    fn nodes(&self) -> Vec<String> {
        self.nodes.clone()
    }

    async fn apply(
        &self,
        params: TransformApplyParams,
        ctx: &Context,
    ) -> PluginResult<TransformApplyResult> {
        self.do_apply(params, ctx).await
    }
}

/// A transformer that runs the collect pass. `Transformer::COLLECTS` is a
/// constant, so collecting and non-collecting handlers need distinct types.
struct Collecting(DynTransformer);

impl Transformer for Collecting {
    const COLLECTS: bool = true;

    fn nodes(&self) -> Vec<String> {
        self.0.nodes.clone()
    }

    async fn collect(
        &self,
        params: TransformCollectParams,
        ctx: &Context,
    ) -> PluginResult<TransformCollectResult> {
        self.0.do_collect(params, ctx).await
    }

    async fn apply(
        &self,
        params: TransformApplyParams,
        ctx: &Context,
    ) -> PluginResult<TransformApplyResult> {
        self.0.do_apply(params, ctx).await
    }
}

struct DynRenderer {
    formats: Vec<String>,
    handler: Handler,
}

impl Renderer for DynRenderer {
    fn formats(&self) -> Vec<String> {
        self.formats.clone()
    }

    async fn render(&self, params: RenderParams, ctx: &Context) -> PluginResult<RenderResult> {
        let format = params.format.clone();
        call(&self.handler, "render", &params, ctx, |v| match v {
            Value::String(output) => serde_json::json!({ "output": output, "extension": format }),
            Value::Object(mut map) => {
                map.entry("extension").or_insert(Value::String(format));
                Value::Object(map)
            }
            other => other,
        })
        .await
    }
}

/// A renderer that also handles `render/site`. `Renderer::SITE` is a
/// constant, so it needs its own type, like [`Collecting`].
struct SiteRenderer(DynRenderer, Handler);

impl Renderer for SiteRenderer {
    const SITE: bool = true;

    fn formats(&self) -> Vec<String> {
        self.0.formats()
    }

    async fn render(&self, params: RenderParams, ctx: &Context) -> PluginResult<RenderResult> {
        self.0.render(params, ctx).await
    }

    async fn render_site(
        &self,
        params: RenderSiteParams,
        ctx: &Context,
    ) -> PluginResult<RenderSiteResult> {
        call(&self.1, "render/site", &params, ctx, |v| match v {
            Value::Array(files) => serde_json::json!({ "files": files }),
            Value::Null => serde_json::json!({}),
            other => other,
        })
        .await
    }
}

/// Builds a [`Handler`] from a synchronous closure. Useful for tests and
/// for bindings whose calls are synchronous.
pub fn handler<F>(f: F) -> Handler
where
    F: Fn(Value, Context) -> Result<Value, String> + Send + Sync + 'static,
{
    let f = Arc::new(f);
    Arc::new(move |value, ctx| {
        let f = f.clone();
        Box::pin(async move { f(value, ctx) })
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use lsdocs_plugin_sdk::doctree::{Document, text};
    use lsdocs_plugin_sdk::protocol::{InitializeParams, SiteInfo};
    use lsdocs_plugin_sdk::{BoxFuture as Fut, EventSink};
    use serde_json::json;

    struct Ignore;
    impl EventSink for Ignore {
        fn send(&self, _event: Event) -> Fut<'_, ()> {
            Box::pin(async {})
        }
    }

    fn ctx() -> Context {
        Context::new(
            Arc::new(Ignore),
            &InitializeParams {
                site: SiteInfo {
                    title: Some("T".into()),
                    ..SiteInfo::default()
                },
                ..InitializeParams::default()
            },
        )
    }

    fn doc() -> Document {
        let mut doc = Document::new("a.txt");
        doc.children.push(text("hi"));
        doc
    }

    fn sample() -> DynamicPlugin {
        DynamicPlugin {
            name: "dyn".into(),
            version: "1.0.0".into(),
            max_concurrency: Some(1),
            parse: Some((
                vec!["txt".into()],
                // Returns just a document, without `source`.
                handler(|params, _| {
                    Ok(json!({"children": [{"type": "text", "value": params["text"]}]}))
                }),
            )),
            collect: Some(handler(|params, _| {
                Ok(json!(
                    params["document"]["children"].as_array().unwrap().len()
                ))
            })),
            apply: Some((
                vec!["text".into()],
                handler(|params, _| {
                    let mut doc = params["document"].clone();
                    doc["children"][0]["value"] = json!(params["collected"].to_string());
                    Ok(json!({"document": doc}))
                }),
            )),
            render: Some((
                vec!["txt".into()],
                handler(|params, ctx| {
                    Ok(json!(format!(
                        "{}: {}",
                        ctx.site().title.as_deref().unwrap_or(""),
                        params["document"]["children"][0]["value"].as_str().unwrap()
                    )))
                }),
            )),
            render_site: None,
        }
    }

    #[test]
    fn capabilities_come_from_the_handlers() {
        let caps = sample().into_plugin().capabilities();
        assert_eq!(caps.parse.unwrap().extensions, ["txt"]);
        let t = caps.transform.unwrap();
        assert!(t.collect);
        assert_eq!(t.nodes, ["text"]);
        assert_eq!(caps.render.unwrap().formats, ["txt"]);
        assert_eq!(caps.max_concurrency, Some(1));

        let no_collect = DynamicPlugin {
            collect: None,
            ..sample()
        };
        assert!(
            !no_collect
                .into_plugin()
                .capabilities()
                .transform
                .unwrap()
                .collect
        );
    }

    #[tokio::test]
    async fn render_site_handlers() {
        use lsdocs_plugin_sdk::protocol::RenderedPage;
        let plugin = DynamicPlugin {
            render_site: Some(handler(|params, _| {
                let count = params["pages"].as_array().map_or(0, Vec::len);
                // A bare list of files; one as text, one as bytes.
                Ok(json!([
                    {"path": "pages.txt", "output": format!("{count} pages")},
                    {"path": "raw.bin", "output": [0, 255]},
                ]))
            })),
            ..sample()
        };
        let plugin = plugin.into_plugin();
        assert!(plugin.capabilities().render.unwrap().site);
        assert!(!sample().into_plugin().capabilities().render.unwrap().site);
        let result = plugin
            .render_site(
                RenderSiteParams {
                    format: "txt".into(),
                    pages: vec![RenderedPage {
                        source: "a.txt".into(),
                        path: "a.txt".into(),
                        ..RenderedPage::default()
                    }],
                },
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.files[0].path, "pages.txt");
        assert_eq!(result.files[0].output, b"1 pages");
        assert_eq!(result.files[1].output, [0, 255]);
    }

    #[tokio::test]
    async fn handlers_run_with_shorthand_results() {
        let plugin = sample().into_plugin();
        let ctx = ctx();

        let parsed = plugin
            .parse(
                ParseParams {
                    source: "a.txt".into(),
                    text: "hello".into(),
                },
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(parsed.document.source, "a.txt", "source is filled in");
        assert_eq!(parsed.document.children, [text("hello")]);

        let collected = plugin
            .collect(TransformCollectParams { document: doc() }, &ctx)
            .await
            .unwrap();
        assert_eq!(collected.data, json!(1));

        let applied = plugin
            .apply(
                TransformApplyParams {
                    document: doc(),
                    collected: json!({"a.txt": 1}),
                },
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(applied.document.children, [text(r#"{"a.txt":1}"#)]);

        let rendered = plugin
            .render(
                RenderParams {
                    document: doc(),
                    format: "txt".into(),
                },
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(rendered.output, b"T: hi");
        assert_eq!(rendered.extension, "txt");
    }

    #[tokio::test]
    async fn bad_results_and_handler_errors_are_reported() {
        let plugin = DynamicPlugin {
            parse: Some((
                vec!["txt".into()],
                handler(|_, _| Ok(json!({"document": 5}))),
            )),
            render: Some((vec!["txt".into()], handler(|_, _| Err("boom".into())))),
            ..sample()
        }
        .into_plugin();
        let ctx = ctx();
        let err = plugin
            .parse(
                ParseParams {
                    source: "a.txt".into(),
                    text: String::new(),
                },
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.message().starts_with("invalid parse result:"), "{err}");
        let err = plugin
            .render(
                RenderParams {
                    document: doc(),
                    format: "txt".into(),
                },
                &ctx,
            )
            .await
            .unwrap_err();
        assert_eq!(err.message(), "boom");
    }
}
