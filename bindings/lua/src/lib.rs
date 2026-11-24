//! Lua bindings for the lsdocs plugin SDK: the `lsdocs_plugin` Lua module.
//!
//! ```lua
//! local lsdocs = require("lsdocs_plugin")
//! local plugin = lsdocs.plugin("shout", "0.1.0")
//!
//! plugin:transform({ "text" }, function(params, ctx)
//!   local doc = params.document
//!   for _, node in ipairs(doc.children) do
//!     if node.type == "text" then node.value = node.value:upper() end
//!   end
//!   return doc -- shorthand for { document = doc }
//! end)
//!
//! plugin:serve()
//! ```
//!
//! A plugin is a [`DynamicPlugin`] whose handlers are Lua functions; the
//! protocol (framing, handshake, cancellation, shutdown) stays in Rust.
//!
//! Values cross as plain Lua tables. Into Lua, JSON `null` is `nil`. Out of
//! Lua, see [`to_json`]: an empty table is an empty list (except as
//! `metadata`), `lsdocs_plugin.object(t)` marks a table as a map, and
//! `lsdocs_plugin.null` is `null`.
//!
//! Threads: handlers must be `Send + Sync`, and Lua values are neither
//! (mlua's `send` feature does not build in module mode). So a handler
//! holds only the registry key of its Lua function, and finds the Lua state
//! through a thread-local that `serve()` and `call()` set: they run the
//! protocol on a single-threaded runtime on their own thread, where every
//! handler then runs, one at a time. The events a handler sends
//! (`ctx:log`, ...) are queued and sent once it returns.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lsdocs_plugin_sdk::protocol::{
    ConfigInfo, Diagnostic, DiagnosticsParams, InitializeParams, LogLevel, LogParams,
    PROTOCOL_VERSION, ParseParams, ParseTransformEachParams, ProgressParams, RenderParams,
    RenderTransformAllParams, RenderTransformEachParams, SiteInfo,
};
use lsdocs_plugin_sdk::{BoxFuture, Context, Event, EventSink};
use lsdocs_plugin_sdk_bindings::{DynamicPlugin, Handler, Value as Json};
use mlua::{
    Error, Function, Lua, LuaSerdeExt, MultiValue, Result, Table, UserData, UserDataFields,
    UserDataMethods, Value, WeakLua, serde::SerializeOptions,
};

/// The `__name` of the metatable `lsdocs_plugin.object` sets.
const OBJECT: &str = "lsdocs_plugin.object";

/// Tables nested deeper than this are taken for a cycle.
const MAX_DEPTH: usize = 512;

/// Into Lua: `nil` for `null`, and arrays without the array metatable, so
/// values are plain tables.
fn to_lua(lua: &Lua, value: &Json) -> Result<Value> {
    let options = SerializeOptions::new()
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false)
        .set_array_metatable(false);
    lua.to_value_with(value, options)
}

fn is_null(value: &Value) -> bool {
    matches!(value, Value::LightUserData(p) if p.0.is_null())
}

/// Out of Lua, as JSON:
///
/// - `nil` and `lsdocs_plugin.null` are `null`.
/// - A number with no fraction is an integer (LuaJIT has no integers;
///   protocol fields such as a heading's `level` must be integers).
/// - A string is a string if it is UTF-8, else a list of its bytes (how
///   binary `output` reads from JSON).
/// - A table with keys `1..n` is a list; an empty table is an empty list,
///   except as the value of `metadata` (a map in the document tree) or
///   when marked with `lsdocs_plugin.object`; other tables are maps with
///   string keys.
///
/// # Errors
///
/// On values JSON cannot hold: functions, userdata, NaN, keys that are
/// not strings, or tables nested too deep (a cycle).
pub fn to_json(value: &Value) -> Result<Json> {
    to_json_at(value, None, 0)
}

fn to_json_at(value: &Value, key: Option<&str>, depth: usize) -> Result<Json> {
    if depth > MAX_DEPTH {
        return Err(Error::runtime("tables nested too deep (a cycle?)"));
    }
    Ok(match value {
        Value::Nil => Json::Null,
        v if is_null(v) => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(n) => Json::from(*n),
        Value::Number(f) => number(*f)?,
        Value::String(s) => match s.to_str() {
            Ok(s) => Json::String(s.to_owned()),
            Err(_) => Json::Array(s.as_bytes().iter().map(|&b| Json::from(b)).collect()),
        },
        Value::Table(t) => table(t, key, depth)?,
        other => {
            return Err(Error::runtime(format!(
                "cannot send a {} to lsdocs",
                other.type_name()
            )));
        }
    })
}

fn number(f: f64) -> Result<Json> {
    // 2^53: every integer up to it is exact in an f64.
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if f.fract() == 0.0 && f.abs() <= EXACT {
        #[allow(clippy::cast_possible_truncation)]
        return Ok(Json::from(f as i64));
    }
    serde_json::Number::from_f64(f)
        .map(Json::Number)
        .ok_or_else(|| Error::runtime("NaN and infinity cannot be sent to lsdocs"))
}

fn is_object(t: &Table) -> bool {
    t.metatable()
        .and_then(|m| m.raw_get::<Option<String>>("__name").ok().flatten())
        .is_some_and(|name| name == OBJECT)
}

fn table(t: &Table, key: Option<&str>, depth: usize) -> Result<Json> {
    let pairs: Vec<(Value, Value)> = t.pairs::<Value, Value>().collect::<Result<_>>()?;
    let object = is_object(t);
    if pairs.is_empty() {
        return Ok(if object || key == Some("metadata") {
            Json::Object(serde_json::Map::new())
        } else {
            Json::Array(Vec::new())
        });
    }
    let len = t.raw_len();
    if !object && len == pairs.len() {
        let mut items = Vec::with_capacity(len);
        for i in 1..=len {
            let item: Value = t.raw_get(i)?;
            items.push(to_json_at(&item, None, depth + 1)?);
        }
        return Ok(Json::Array(items));
    }
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        let Value::String(k) = k else {
            return Err(Error::runtime(format!(
                "a table sent to lsdocs has a {} key; keys must be strings, \
                 or the table a list (keys 1..n)",
                k.type_name()
            )));
        };
        let k = k.to_str()?.to_owned();
        let v = to_json_at(&v, Some(&k), depth + 1)?;
        map.insert(k, v);
    }
    Ok(Json::Object(map))
}

thread_local! {
    /// The Lua state handlers on this thread run in, while `serve()` or
    /// `call()` runs on it.
    static STATE: RefCell<Option<WeakLua>> = const { RefCell::new(None) };
}

/// Runs `f` with `lua` as the state of this thread's handlers.
fn with_state<T>(lua: &Lua, f: impl FnOnce() -> T) -> T {
    let previous = STATE.with(|s| s.replace(Some(lua.weak())));
    let out = f();
    STATE.with(|s| *s.borrow_mut() = previous);
    out
}

fn current_state() -> std::result::Result<Lua, String> {
    STATE
        .with(|s| s.borrow().as_ref().and_then(WeakLua::try_upgrade))
        .ok_or_else(|| {
            "no Lua state on this thread: handlers run from serve() or call()".to_owned()
        })
}

/// Wraps a Lua function `(params, ctx) -> result` as a handler (see the
/// module docs on threads).
fn lua_handler(lua: &Lua, func: Function) -> Result<Handler> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let key = format!(
        "lsdocs_plugin.handler.{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    lua.set_named_registry_value(&key, func)?;
    let key = Arc::new(key);
    Ok(Arc::new(
        move |value: Json, ctx: Context| -> BoxFuture<'static, std::result::Result<Json, String>> {
            let key = key.clone();
            Box::pin(async move {
                let events: Events = Arc::default();
                let result = current_state().and_then(|lua| {
                    let run = || -> Result<Json> {
                        let func: Function = lua.named_registry_value(&key)?;
                        let params = to_lua(&lua, &value)?;
                        let cx = LuaContext {
                            ctx: ctx.clone(),
                            events: events.clone(),
                        };
                        let result: Value = func.call((params, cx))?;
                        to_json(&result)
                    };
                    run().map_err(|e| e.to_string())
                });
                let queued =
                    std::mem::take(&mut *events.lock().unwrap_or_else(PoisonError::into_inner));
                for event in queued {
                    send(&ctx, event).await;
                }
                result
            })
        },
    ))
}

/// Events a handler sent, to send once it returns.
type Events = Arc<Mutex<Vec<Event>>>;

async fn send(ctx: &Context, event: Event) {
    match event {
        Event::Log(p) => ctx.log(p.level, p.message).await,
        Event::Progress(p) => ctx.progress(p.message, p.percentage).await,
        Event::Diagnostics(d) => {
            for diagnostic in d.diagnostics {
                ctx.diagnostic(diagnostic).await;
            }
        }
    }
}

/// A plugin definition, made by `lsdocs_plugin.plugin(name, version,
/// options)`.
struct LuaPlugin {
    inner: DynamicPlugin,
}

fn strings(list: Option<Vec<String>>) -> Vec<String> {
    list.unwrap_or_default()
}

impl UserData for LuaPlugin {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.inner.name.clone()));
        fields.add_field_method_get("version", |_, this| Ok(this.inner.version.clone()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut(
            "parse",
            |lua, this, (extensions, func): (Vec<String>, Function)| {
                this.inner.parse = Some((extensions, lua_handler(lua, func)?));
                Ok(())
            },
        );
        methods.add_method_mut("read", |lua, this, func: Function| {
            this.inner.read = Some(lua_handler(lua, func)?);
            Ok(())
        });
        methods.add_method_mut(
            "transform",
            |lua, this, (nodes, func): (Option<Vec<String>>, Function)| {
                this.inner.apply = Some((strings(nodes), lua_handler(lua, func)?));
                Ok(())
            },
        );
        methods.add_method_mut(
            "render",
            |lua, this, (formats, func): (Vec<String>, Function)| {
                this.inner.render = Some((formats, lua_handler(lua, func)?));
                Ok(())
            },
        );
        methods.add_method_mut("render_all", |lua, this, func: Function| {
            this.inner.render_all = Some(lua_handler(lua, func)?);
            Ok(())
        });
        methods.add_method("capabilities", |lua, this, ()| {
            let caps = this.inner.clone().into_plugin().capabilities();
            let caps = serde_json::to_value(caps).map_err(Error::external)?;
            to_lua(lua, &caps)
        });
        methods.add_method("serve", |lua, this, ()| {
            let plugin = this.inner.clone().into_plugin();
            let rt = runtime()?;
            with_state(lua, || rt.block_on(lsdocs_plugin_sdk::serve(plugin)))
                .map_err(Error::external)
        });
    }
}

/// What a handler receives alongside its params.
struct LuaContext {
    ctx: Context,
    events: Events,
}

impl LuaContext {
    fn queue(&self, event: Event) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }

    fn log(&self, level: &str, message: String) -> Result<()> {
        let level: LogLevel = serde_json::from_value(Json::String(level.into()))
            .map_err(|_| Error::runtime(format!("unknown log level {level:?}")))?;
        self.queue(Event::Log(LogParams { level, message }));
        Ok(())
    }
}

impl UserData for LuaContext {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        // The site being built: `title`, `base_url`, `root`.
        fields.add_field_method_get("site", |lua, this| {
            let site = serde_json::to_value(this.ctx.site()).map_err(Error::external)?;
            to_lua(lua, &site)
        });
        // Each of the plugin's `config_files` (as resolved) mapped to its
        // text; a missing file is nil.
        fields.add_field_method_get("settings", |lua, this| to_lua(lua, this.ctx.settings()));
        // `dir` (the site's `.config/`) and `name` (the plugin's name in
        // the site config).
        fields.add_field_method_get("config", |lua, this| {
            let config = serde_json::to_value(this.ctx.config()).map_err(Error::external)?;
            to_lua(lua, &config)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("log", |_, this, (level, message): (String, String)| {
            this.log(&level, message)
        });
        for level in ["debug", "info", "warn", "error"] {
            methods.add_method(level, move |_, this, message: String| {
                this.log(level, message)
            });
        }
        methods.add_method(
            "progress",
            |_, this, (message, percentage): (String, Option<u8>)| {
                this.queue(Event::Progress(ProgressParams {
                    message,
                    percentage,
                }));
                Ok(())
            },
        );
        methods.add_method("diagnostic", |_, this, diagnostic: Value| {
            let diagnostic: Diagnostic = serde_json::from_value(to_json(&diagnostic)?)
                .map_err(|e| Error::runtime(format!("invalid diagnostic: {e}")))?;
            this.queue(Event::Diagnostics(DiagnosticsParams {
                diagnostics: vec![diagnostic],
            }));
            Ok(())
        });
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::runtime(format!("failed to start: {e}")))
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

fn event_value(event: Event) -> serde_json::Result<Json> {
    let (kind, value) = match event {
        Event::Log(p) => ("log", serde_json::to_value(p)),
        Event::Progress(p) => ("progress", serde_json::to_value(p)),
        Event::Diagnostics(p) => ("diagnostics", serde_json::to_value(p)),
    };
    let mut value = value?;
    value["kind"] = Json::String(kind.into());
    Ok(value)
}

fn to_result<T: serde::Serialize>(
    result: std::result::Result<T, lsdocs_plugin_sdk::PluginError>,
) -> std::result::Result<Json, String> {
    let value = result.map_err(|e| e.to_string())?;
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn decode<T: serde::de::DeserializeOwned>(
    method: &str,
    value: Json,
) -> std::result::Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("invalid {method} params: {e}"))
}

fn field<T: serde::de::DeserializeOwned + Default>(
    options: Option<&Table>,
    name: &str,
) -> Result<Option<T>> {
    let Some(options) = options else {
        return Ok(None);
    };
    let value: Value = options.get(name)?;
    if value.is_nil() {
        return Ok(None);
    }
    serde_json::from_value(to_json(&value)?)
        .map(Some)
        .map_err(|e| Error::runtime(format!("invalid {name}: {e}")))
}

/// Binary output (a list of byte values in JSON) as a Lua string.
fn output_as_string(lua: &Lua, json: &Json, table: &Table) -> Result<()> {
    if let Some(Json::Array(bytes)) = json.get("output") {
        let bytes: Vec<u8> = bytes
            .iter()
            .filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
            .collect();
        table.set("output", lua.create_string(&bytes)?)?;
    }
    Ok(())
}

/// `lsdocs_plugin.call(plugin, method, params, options)`: runs one of the
/// plugin's handlers in-process, as the host would, and returns `result,
/// events`. For tests. `options`: `site`, `settings` (used as given;
/// otherwise the plugin reads its `config_files`) and `config`.
fn call(
    lua: &Lua,
    plugin: &LuaPlugin,
    method: &str,
    params: Value,
    options: Option<Table>,
) -> Result<MultiValue> {
    let params = to_json(&params)?;
    let options = options.as_ref();
    let site: SiteInfo = field(options, "site")?.unwrap_or_default();
    let config: ConfigInfo = field(options, "config")?.unwrap_or_default();
    let settings: Option<Json> = field(options, "settings")?;
    let init = InitializeParams {
        protocol_version: PROTOCOL_VERSION.into(),
        site,
        config,
        ..InitializeParams::default()
    };
    let sdk_plugin = plugin.inner.clone().into_plugin();
    let setup = match settings {
        Some(settings) => lsdocs_plugin_sdk::Setup::with_settings(init, settings),
        None => sdk_plugin.setup(&init).map_err(Error::runtime)?,
    };
    let recorder = Arc::new(Recorder::default());
    let ctx = Context::new(recorder.clone(), &setup);
    let rt = runtime()?;
    let result = with_state(lua, || {
        rt.block_on(async {
            match method {
                "initialize" => to_result(Ok(sdk_plugin.initialize_result(&setup))),
                "parse" => to_result(
                    sdk_plugin
                        .parse(decode::<ParseParams>(method, params)?, &ctx)
                        .await,
                ),
                "parse/transform/each" => to_result(
                    sdk_plugin
                        .read(decode::<ParseTransformEachParams>(method, params)?, &ctx)
                        .await,
                ),
                "render/transform/each" => to_result(
                    sdk_plugin
                        .apply(decode::<RenderTransformEachParams>(method, params)?, &ctx)
                        .await,
                ),
                "render" => to_result(
                    sdk_plugin
                        .render(decode::<RenderParams>(method, params)?, &ctx)
                        .await,
                ),
                "render/transform/all" => to_result(
                    sdk_plugin
                        .render_all(decode::<RenderTransformAllParams>(method, params)?, &ctx)
                        .await,
                ),
                other => Err(format!("unknown method {other:?}")),
            }
        })
    });
    let result = result.map_err(Error::runtime)?;
    let result_lua = to_lua(lua, &result)?;
    // Render output back as a string (it is bytes).
    if let Value::Table(t) = &result_lua {
        if method == "render" {
            output_as_string(lua, &result, t)?;
        }
        if method == "render/transform/all"
            && let (Some(Json::Array(files)), Ok(Some(lua_files))) =
                (result.get("files"), t.get::<Option<Table>>("files"))
        {
            for (i, file) in files.iter().enumerate() {
                if let Some(lua_file) = lua_files.get::<Option<Table>>(i + 1)? {
                    output_as_string(lua, file, &lua_file)?;
                }
            }
        }
    }
    let events: Vec<Json> =
        std::mem::take(&mut *recorder.0.lock().unwrap_or_else(PoisonError::into_inner))
            .into_iter()
            .map(event_value)
            .collect::<serde_json::Result<_>>()
            .map_err(Error::external)?;
    Ok(MultiValue::from_vec(vec![
        result_lua,
        to_lua(lua, &Json::Array(events))?,
    ]))
}

/// The module table.
///
/// # Errors
///
/// If Lua cannot allocate the table or its functions.
pub fn open(lua: &Lua) -> Result<Table> {
    let m = lua.create_table()?;
    m.set("version", env!("CARGO_PKG_VERSION"))?;
    m.set("PROTOCOL_VERSION", PROTOCOL_VERSION)?;
    m.set("null", Value::NULL)?;
    m.set(
        "plugin",
        lua.create_function(
            |_, (name, version, options): (String, Option<String>, Option<Table>)| {
                let config_files: Vec<String> =
                    field(options.as_ref(), "config_files")?.unwrap_or_default();
                Ok(LuaPlugin {
                    inner: DynamicPlugin {
                        name,
                        version: version.unwrap_or_else(|| "0.0.0".to_owned()),
                        // Handlers share one Lua state: one at a time.
                        max_concurrency: Some(1),
                        config_files,
                        ..DynamicPlugin::default()
                    },
                })
            },
        )?,
    )?;
    let object_mt = lua.create_table()?;
    object_mt.set("__name", OBJECT)?;
    m.set(
        "object",
        lua.create_function(move |lua, t: Option<Table>| {
            let t = match t {
                Some(t) => t,
                None => lua.create_table()?,
            };
            t.set_metatable(Some(object_mt.clone()))?;
            Ok(t)
        })?,
    )?;
    m.set(
        "call",
        lua.create_function(
            |lua,
             (plugin, method, params, options): (
                mlua::UserDataRef<LuaPlugin>,
                String,
                Value,
                Option<Table>,
            )| { call(lua, &plugin, &method, params, options) },
        )?,
    )?;
    Ok(m)
}

/// The entry point an external interpreter calls (`luaopen_lsdocs_plugin`).
#[cfg(feature = "module")]
#[mlua::lua_module(name = "lsdocs_plugin")]
fn luaopen(lua: &Lua) -> Result<Table> {
    open(lua)
}
