//! Native part of the `@lsdocs/plugin` npm package.
//!
//! The JavaScript API lives in `index.js`; this module provides what it
//! wraps:
//!
//! - `NativePlugin`: a plugin definition holding JS handler functions.
//! - `serve(plugin)`: runs the plugin until the host is done (a Promise).
//! - `call(plugin, method, params, site, options)`: runs one handler
//!   in-process with a fake host, for tests (a Promise).
//!
//! Values cross between JS and Rust as JSON strings. `index.js` wraps each
//! user handler as `(input: string) => Promise<string>`, where `input` is
//! `{"params", "site", "options"}` and the reply is
//! `{"result", "events"}` or `{"error", "events"}`. Events (log lines,
//! progress, diagnostics) recorded during the call are sent to the host once
//! the handler finishes.

use std::sync::{Arc, Mutex, PoisonError};

use lsdocs_plugin_sdk::protocol::{
    Diagnostic, InitializeParams, LogLevel, PROTOCOL_VERSION as VERSION, ParseParams, RenderParams,
    RenderSiteParams, SiteInfo, TransformApplyParams, TransformCollectParams,
};
use lsdocs_plugin_sdk::{BoxFuture, Context, Event, EventSink, PluginError};
use lsdocs_sdk_dynamic::{DynamicPlugin, Handler, Value};
use napi::bindgen_prelude::{Function, Promise, PromiseRaw};
use napi::threadsafe_function::ThreadsafeFunction;
use napi::{Env, Error, Result, Status};
use napi_derive::napi;
use serde::Deserialize;

/// Protocol version spoken by this package.
#[napi]
pub const PROTOCOL_VERSION: &str = VERSION;

/// A JS handler, callable from any thread. Weak, so it does not keep the
/// Node.js event loop alive by itself.
type JsHandler = ThreadsafeFunction<String, Promise<String>, String, Status, false, true>;

/// The reply `index.js` sends back from a handler.
#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    events: Vec<JsEvent>,
}

/// An event recorded by the JS context object.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsEvent {
    Log {
        level: LogLevel,
        message: String,
    },
    Progress {
        message: String,
        #[serde(default)]
        percentage: Option<u8>,
    },
    Diagnostics {
        diagnostics: Vec<Diagnostic>,
    },
}

fn js_handler(tsfn: JsHandler) -> Handler {
    let tsfn = Arc::new(tsfn);
    Arc::new(
        move |params: Value,
              ctx: Context|
              -> BoxFuture<'static, std::result::Result<Value, String>> {
            let tsfn = tsfn.clone();
            Box::pin(async move {
                let input = serde_json::json!({
                    "params": params,
                    "site": ctx.site(),
                    "options": ctx.options(),
                })
                .to_string();
                let promise = tsfn
                    .call_async_catch(input)
                    .await
                    .map_err(|e| e.reason.clone())?;
                let output = promise.await.map_err(|e| e.reason.clone())?;
                let reply: Reply = serde_json::from_str(&output)
                    .map_err(|e| format!("invalid reply from handler: {e}"))?;
                for event in reply.events {
                    match event {
                        JsEvent::Log { level, message } => ctx.log(level, message).await,
                        JsEvent::Progress {
                            message,
                            percentage,
                        } => ctx.progress(message, percentage).await,
                        JsEvent::Diagnostics { diagnostics } => {
                            for d in diagnostics {
                                ctx.diagnostic(d).await;
                            }
                        }
                    }
                }
                match reply.error {
                    Some(error) => Err(error),
                    None => Ok(reply.result),
                }
            })
        },
    )
}

fn build(handler: Function<'_, String, Promise<String>>) -> Result<Handler> {
    let tsfn: JsHandler = handler
        .build_threadsafe_function::<String>()
        .weak::<true>()
        .build()?;
    Ok(js_handler(tsfn))
}

fn to_napi(message: impl std::fmt::Display) -> Error {
    Error::new(Status::GenericFailure, message.to_string())
}

/// A plugin definition. Use `definePlugin` from the package rather than
/// this class.
#[napi]
pub struct NativePlugin {
    inner: DynamicPlugin,
}

#[napi]
impl NativePlugin {
    /// A plugin with no handlers yet; `max_concurrency` defaults to 1.
    #[napi(constructor)]
    pub fn new(name: String, version: String, max_concurrency: Option<u32>) -> Self {
        Self {
            inner: DynamicPlugin {
                name,
                version,
                max_concurrency: Some(max_concurrency.unwrap_or(1)),
                ..DynamicPlugin::default()
            },
        }
    }

    /// Sets the parser for files with these extensions.
    #[napi]
    pub fn set_parse(
        &mut self,
        extensions: Vec<String>,
        handler: Function<'_, String, Promise<String>>,
    ) -> Result<()> {
        self.inner.parse = Some((extensions, build(handler)?));
        Ok(())
    }

    /// Sets the collect-pass handler.
    #[napi]
    pub fn set_collect(&mut self, handler: Function<'_, String, Promise<String>>) -> Result<()> {
        self.inner.collect = Some(build(handler)?);
        Ok(())
    }

    /// Sets the transformer for documents containing these node kinds.
    #[napi]
    pub fn set_apply(
        &mut self,
        nodes: Vec<String>,
        handler: Function<'_, String, Promise<String>>,
    ) -> Result<()> {
        self.inner.apply = Some((nodes, build(handler)?));
        Ok(())
    }

    /// Sets the renderer for these output formats.
    #[napi]
    pub fn set_render(
        &mut self,
        formats: Vec<String>,
        handler: Function<'_, String, Promise<String>>,
    ) -> Result<()> {
        self.inner.render = Some((formats, build(handler)?));
        Ok(())
    }

    /// Sets the `render/site` handler, which returns site-wide files.
    #[napi]
    pub fn set_render_site(
        &mut self,
        handler: Function<'_, String, Promise<String>>,
    ) -> Result<()> {
        self.inner.render_site = Some(build(handler)?);
        Ok(())
    }

    /// The capabilities reported to the host, as JSON.
    #[napi]
    pub fn capabilities(&self) -> Result<String> {
        serde_json::to_string(&self.inner.clone().into_plugin().capabilities()).map_err(to_napi)
    }
}

/// Serves `plugin` over stdio (or `LSDOCS_PIPE`). Resolves when the host is
/// done with it.
#[napi]
pub fn serve<'env>(env: &'env Env, plugin: &NativePlugin) -> Result<PromiseRaw<'env, ()>> {
    let plugin = plugin.inner.clone().into_plugin();
    env.spawn_future(async move { lsdocs_plugin_sdk::serve(plugin).await.map_err(to_napi) })
}

/// Records events for `call`.
#[derive(Default)]
struct Recorder(Mutex<Vec<Event>>);

impl EventSink for Recorder {
    fn send(&self, event: Event) -> BoxFuture<'_, ()> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        Box::pin(async {})
    }
}

fn event_value(event: Event) -> serde_json::Result<Value> {
    let (kind, value) = match event {
        Event::Log(p) => ("log", serde_json::to_value(p)),
        Event::Progress(p) => ("progress", serde_json::to_value(p)),
        Event::Diagnostics(p) => ("diagnostics", serde_json::to_value(p)),
    };
    let mut value = value?;
    value["kind"] = Value::String(kind.into());
    Ok(value)
}

fn to_json<T: serde::Serialize>(
    result: std::result::Result<T, PluginError>,
) -> std::result::Result<Value, String> {
    let value = result.map_err(|e| e.to_string())?;
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn decode<T: serde::de::DeserializeOwned>(
    method: &str,
    value: Value,
) -> std::result::Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("invalid {method} params: {e}"))
}

/// Calls one of the plugin's handlers in-process, as the host would.
/// Arguments are JSON; resolves to JSON `{"result", "events"}`. For tests;
/// see `@lsdocs/plugin/testing`.
#[napi]
pub fn call<'env>(
    env: &'env Env,
    plugin: &NativePlugin,
    method: String,
    params: String,
    site: Option<String>,
    options: Option<String>,
) -> Result<PromiseRaw<'env, String>> {
    let parse = |what: &str, json: Option<String>| -> Result<Value> {
        match json {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| to_napi(format!("invalid {what}: {e}")))
            }
            None => Ok(Value::Null),
        }
    };
    let params = parse("params", Some(params))?;
    let site: SiteInfo = match parse("site", site)? {
        Value::Null => SiteInfo::default(),
        v => serde_json::from_value(v).map_err(|e| to_napi(format!("invalid site: {e}")))?,
    };
    let init = InitializeParams {
        protocol_version: VERSION.into(),
        site,
        options: parse("options", options)?,
        ..InitializeParams::default()
    };
    let plugin = plugin.inner.clone().into_plugin();
    env.spawn_future(async move {
        let recorder = Arc::new(Recorder::default());
        let ctx = Context::new(recorder.clone(), &init);
        let result = match method.as_str() {
            "initialize" => to_json(Ok(plugin.initialize_result(&init))),
            "parse" => match decode::<ParseParams>("parse", params) {
                Ok(p) => to_json(plugin.parse(p, &ctx).await),
                Err(e) => Err(e),
            },
            "transform/collect" => {
                match decode::<TransformCollectParams>("transform/collect", params) {
                    Ok(p) => to_json(plugin.collect(p, &ctx).await),
                    Err(e) => Err(e),
                }
            }
            "transform/apply" => match decode::<TransformApplyParams>("transform/apply", params) {
                Ok(p) => to_json(plugin.apply(p, &ctx).await),
                Err(e) => Err(e),
            },
            "render" => match decode::<RenderParams>("render", params) {
                Ok(p) => to_json(plugin.render(p, &ctx).await),
                Err(e) => Err(e),
            },
            "render/site" => match decode::<RenderSiteParams>("render/site", params) {
                Ok(p) => to_json(plugin.render_site(p, &ctx).await),
                Err(e) => Err(e),
            },
            other => Err(format!("unknown method {other:?}")),
        }
        .map_err(to_napi)?;
        let events: Vec<Value> =
            std::mem::take(&mut *recorder.0.lock().unwrap_or_else(PoisonError::into_inner))
                .into_iter()
                .map(event_value)
                .collect::<serde_json::Result<_>>()
                .map_err(to_napi)?;
        Ok(serde_json::json!({ "result": result, "events": events }).to_string())
    })
}
