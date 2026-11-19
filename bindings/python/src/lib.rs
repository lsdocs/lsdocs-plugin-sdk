//! Native part of the `lsdocs_plugin` Python package.
//!
//! The Python-facing API lives in `python/lsdocs_plugin/__init__.py`; this
//! module provides the pieces it wraps:
//!
//! - `NativePlugin`: a plugin definition holding Python callables.
//! - `Context`: what handlers receive alongside their params.
//! - `serve(plugin)`: runs the plugin until the host is done with it.
//! - `call(plugin, method, params, ...)`: runs one handler in-process with a
//!   fake host, for tests.
//!
//! Handlers run on a blocking thread holding the GIL, one at a time
//! (`max_concurrency` defaults to 1), while the protocol runs in Rust with
//! the GIL released.

use std::sync::{Arc, Mutex, PoisonError};

use lsdocs_plugin_sdk::protocol::{
    Diagnostic, InitializeParams, LogLevel, PROTOCOL_VERSION, ParseParams,
    ParseTransformEachParams, RenderParams, RenderTransformAllParams, RenderTransformEachParams,
    SiteInfo,
};
use lsdocs_plugin_sdk::{BoxFuture, Context, Event, EventSink};
use lsdocs_plugin_sdk_bindings::{DynamicPlugin, Handler, Value};
use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};

/// Converts a Python value to JSON-like data. Bytes become a list of byte
/// values, which is how the protocol's binary fields read from JSON.
fn to_value(obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(b) = obj.cast::<PyBool>() {
        return Ok(Value::Bool(b.is_true()));
    }
    if obj.is_instance_of::<PyInt>() {
        if let Ok(n) = obj.extract::<i64>() {
            return Ok(n.into());
        }
        if let Ok(n) = obj.extract::<u64>() {
            return Ok(n.into());
        }
        return Err(PyValueError::new_err("integer out of range"));
    }
    if let Ok(f) = obj.cast::<PyFloat>() {
        return serde_json::Number::from_f64(f.value())
            .map(Value::Number)
            .ok_or_else(|| PyValueError::new_err("NaN and infinity are not supported"));
    }
    if let Ok(s) = obj.cast::<PyString>() {
        return Ok(Value::String(s.to_str()?.to_owned()));
    }
    if let Ok(b) = obj.cast::<PyBytes>() {
        return Ok(Value::Array(
            b.as_bytes().iter().map(|&x| x.into()).collect(),
        ));
    }
    if let Ok(d) = obj.cast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in d.iter() {
            let key = k
                .cast::<PyString>()
                .map_err(|_| PyTypeError::new_err("dictionary keys must be strings"))?
                .to_str()?
                .to_owned();
            map.insert(key, to_value(&v)?);
        }
        return Ok(Value::Object(map));
    }
    if let Ok(l) = obj.cast::<PyList>() {
        return l
            .iter()
            .map(|v| to_value(&v))
            .collect::<PyResult<_>>()
            .map(Value::Array);
    }
    if let Ok(t) = obj.cast::<PyTuple>() {
        return t
            .iter()
            .map(|v| to_value(&v))
            .collect::<PyResult<_>>()
            .map(Value::Array);
    }
    Err(PyTypeError::new_err(format!(
        "cannot send a value of type {} to lsdocs",
        obj.get_type().name()?
    )))
}

fn to_py<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
    pythonize::pythonize(py, value).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Formats a Python exception with its traceback, for the host to show.
fn describe(py: Python<'_>, err: PyErr) -> String {
    let traceback = err
        .traceback(py)
        .and_then(|tb| tb.format().ok())
        .unwrap_or_default();
    format!("{err}\n{traceback}").trim_end().to_owned()
}

/// Wraps a Python callable `(params, ctx) -> result` as a handler.
fn py_handler(func: Py<PyAny>) -> Handler {
    let func = Arc::new(func);
    Arc::new(
        move |value: Value, ctx: Context| -> BoxFuture<'static, Result<Value, String>> {
            let func = func.clone();
            Box::pin(async move {
                let handle = tokio::runtime::Handle::current();
                tokio::task::spawn_blocking(move || {
                    Python::attach(|py| {
                        let params = to_py(py, &value).map_err(|e| describe(py, e))?;
                        let ctx =
                            Py::new(py, PyContext { ctx, handle }).map_err(|e| describe(py, e))?;
                        let result = func.call1(py, (params, ctx)).map_err(|e| describe(py, e))?;
                        to_value(result.bind(py)).map_err(|e| describe(py, e))
                    })
                })
                .await
                .map_err(|e| format!("handler task failed: {e}"))?
            })
        },
    )
}

/// A plugin definition. Use `lsdocs_plugin.Plugin` rather than this class.
#[pyclass(module = "lsdocs_plugin._native")]
struct NativePlugin {
    inner: DynamicPlugin,
}

#[pymethods]
impl NativePlugin {
    #[new]
    #[pyo3(signature = (name, version, max_concurrency = Some(1), config_files = Vec::new()))]
    fn new(
        name: String,
        version: String,
        max_concurrency: Option<u32>,
        config_files: Vec<String>,
    ) -> Self {
        Self {
            inner: DynamicPlugin {
                name,
                version,
                max_concurrency,
                config_files,
                ..DynamicPlugin::default()
            },
        }
    }

    fn set_parse(&mut self, extensions: Vec<String>, func: Py<PyAny>) {
        self.inner.parse = Some((extensions, py_handler(func)));
    }

    fn set_read(&mut self, func: Py<PyAny>) {
        self.inner.read = Some(py_handler(func));
    }

    fn set_apply(&mut self, nodes: Vec<String>, func: Py<PyAny>) {
        self.inner.apply = Some((nodes, py_handler(func)));
    }

    fn set_render(&mut self, formats: Vec<String>, func: Py<PyAny>) {
        self.inner.render = Some((formats, py_handler(func)));
    }

    fn set_render_all(&mut self, func: Py<PyAny>) {
        self.inner.render_all = Some(py_handler(func));
    }

    /// The capabilities the plugin will report to the host.
    fn capabilities<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let caps = self.inner.clone().into_plugin().capabilities();
        let caps = serde_json::to_value(caps).map_err(|e| PyValueError::new_err(e.to_string()))?;
        to_py(py, &caps)
    }
}

/// What a handler receives alongside its params.
#[pyclass(name = "Context", module = "lsdocs_plugin._native")]
struct PyContext {
    ctx: Context,
    handle: tokio::runtime::Handle,
}

impl PyContext {
    fn send(&self, py: Python<'_>, event: Event) {
        let ctx = self.ctx.clone();
        py.detach(|| {
            self.handle.block_on(async move {
                match event {
                    Event::Log(p) => ctx.log(p.level, p.message).await,
                    Event::Progress(p) => ctx.progress(p.message, p.percentage).await,
                    Event::Diagnostics(d) => {
                        for diagnostic in d.diagnostics {
                            ctx.diagnostic(diagnostic).await;
                        }
                    }
                }
            });
        });
    }
}

#[pymethods]
impl PyContext {
    /// The site being built: `title`, `base_url`, `root`.
    #[getter]
    fn site<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let site = serde_json::to_value(self.ctx.site())
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        to_py(py, &site)
    }

    /// The plugin's settings: each of its `config_files` (as resolved)
    /// mapped to its text, or None if missing.
    #[getter]
    fn settings<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, self.ctx.settings())
    }

    /// Where the settings are: `dir` (the site's `.config/`) and `name`
    /// (the plugin's name in the site config).
    #[getter]
    fn config<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let config = serde_json::to_value(self.ctx.config())
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        to_py(py, &config)
    }

    /// Sends a log line: level is "error", "warn", "info", "debug" or "trace".
    fn log(&self, py: Python<'_>, level: &str, message: String) -> PyResult<()> {
        let level: LogLevel = serde_json::from_value(Value::String(level.into()))
            .map_err(|_| PyValueError::new_err(format!("unknown log level {level:?}")))?;
        self.send(
            py,
            Event::Log(lsdocs_plugin_sdk::protocol::LogParams { level, message }),
        );
        Ok(())
    }

    fn debug(&self, py: Python<'_>, message: String) -> PyResult<()> {
        self.log(py, "debug", message)
    }

    fn info(&self, py: Python<'_>, message: String) -> PyResult<()> {
        self.log(py, "info", message)
    }

    fn warn(&self, py: Python<'_>, message: String) -> PyResult<()> {
        self.log(py, "warn", message)
    }

    /// Reports progress, optionally with a percentage (0-100).
    #[pyo3(signature = (message, percentage = None))]
    fn progress(&self, py: Python<'_>, message: String, percentage: Option<u8>) {
        self.send(
            py,
            Event::Progress(lsdocs_plugin_sdk::protocol::ProgressParams {
                message,
                percentage,
            }),
        );
    }

    /// Reports a diagnostic dict: `severity`, `message`, and optionally
    /// `source`, `span` and `code`.
    fn diagnostic(&self, py: Python<'_>, diagnostic: Bound<'_, PyAny>) -> PyResult<()> {
        let diagnostic: Diagnostic = serde_json::from_value(to_value(&diagnostic)?)
            .map_err(|e| PyValueError::new_err(format!("invalid diagnostic: {e}")))?;
        self.send(
            py,
            Event::Diagnostics(lsdocs_plugin_sdk::protocol::DiagnosticsParams {
                diagnostics: vec![diagnostic],
            }),
        );
        Ok(())
    }
}

fn runtime() -> PyResult<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| PyRuntimeError::new_err(format!("failed to start: {e}")))
}

/// Serves `plugin` over stdio (or `LSDOCS_PIPE`) until the host is done.
#[pyfunction]
fn serve(py: Python<'_>, plugin: &NativePlugin) -> PyResult<()> {
    let plugin = plugin.inner.clone().into_plugin();
    let rt = runtime()?;
    py.detach(|| rt.block_on(lsdocs_plugin_sdk::serve(plugin)))
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))
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
    result: Result<T, lsdocs_plugin_sdk::PluginError>,
) -> Result<Value, String> {
    let value = result.map_err(|e| e.to_string())?;
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn decode<T: serde::de::DeserializeOwned>(method: &str, value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("invalid {method} params: {e}"))
}

/// Calls one of the plugin's handlers in-process, as the host would, and
/// returns `(result, events)`. For tests; see `lsdocs_plugin.testing`.
#[pyfunction]
#[pyo3(signature = (plugin, method, params = None, site = None, settings = None, config = None))]
fn call<'py>(
    py: Python<'py>,
    plugin: &NativePlugin,
    method: &str,
    params: Option<Bound<'py, PyAny>>,
    site: Option<Bound<'py, PyAny>>,
    settings: Option<Bound<'py, PyAny>>,
    config: Option<Bound<'py, PyAny>>,
) -> PyResult<(Bound<'py, PyAny>, Bound<'py, PyAny>)> {
    let params = match params {
        Some(p) => to_value(&p)?,
        None => Value::Null,
    };
    let site: SiteInfo = match site {
        Some(s) => serde_json::from_value(to_value(&s)?)
            .map_err(|e| PyValueError::new_err(format!("invalid site: {e}")))?,
        None => SiteInfo::default(),
    };
    let config: lsdocs_plugin_sdk::protocol::ConfigInfo = match config {
        Some(c) => serde_json::from_value(to_value(&c)?)
            .map_err(|e| PyValueError::new_err(format!("invalid config: {e}")))?,
        None => Default::default(),
    };
    let init = InitializeParams {
        protocol_version: PROTOCOL_VERSION.into(),
        site,
        config,
        ..InitializeParams::default()
    };
    let plugin = plugin.inner.clone().into_plugin();
    // Settings given are used as they are; otherwise the plugin reads its
    // `config_files`, as it would at `initialize`.
    let setup = match settings {
        Some(s) => lsdocs_plugin_sdk::Setup::with_settings(init, to_value(&s)?),
        None => plugin.setup(&init).map_err(PyRuntimeError::new_err)?,
    };
    let recorder = Arc::new(Recorder::default());
    let ctx = Context::new(recorder.clone(), &setup);
    let method_name = method.to_owned();
    let rt = runtime()?;
    let result: Result<Value, String> = py.detach(|| {
        rt.block_on(async {
            match method_name.as_str() {
                "initialize" => to_json(Ok(plugin.initialize_result(&setup))),
                "parse" => to_json(
                    plugin
                        .parse(decode::<ParseParams>("parse", params)?, &ctx)
                        .await,
                ),
                "parse/transform/each" => to_json(
                    plugin
                        .read(
                            decode::<ParseTransformEachParams>("parse/transform/each", params)?,
                            &ctx,
                        )
                        .await,
                ),
                "render/transform/each" => to_json(
                    plugin
                        .apply(
                            decode::<RenderTransformEachParams>("render/transform/each", params)?,
                            &ctx,
                        )
                        .await,
                ),
                "render" => to_json(
                    plugin
                        .render(decode::<RenderParams>("render", params)?, &ctx)
                        .await,
                ),
                "render/transform/all" => to_json(
                    plugin
                        .render_all(
                            decode::<RenderTransformAllParams>("render/transform/all", params)?,
                            &ctx,
                        )
                        .await,
                ),
                other => Err(format!("unknown method {other:?}")),
            }
        })
    });
    let result = result.map_err(PyRuntimeError::new_err)?;
    let result_py = to_py(py, &result)?;
    // Give render output back as bytes.
    let as_bytes = |dict: &Bound<'_, PyDict>, value: &Value| -> PyResult<()> {
        if let Some(Value::Array(bytes)) = value.get("output") {
            let bytes: Vec<u8> = bytes
                .iter()
                .filter_map(|b| b.as_u64().map(|b| b as u8))
                .collect();
            dict.set_item("output", PyBytes::new(py, &bytes))?;
        }
        Ok(())
    };
    if method == "render"
        && let Ok(dict) = result_py.cast::<PyDict>()
    {
        as_bytes(dict, &result)?;
    }
    if method == "render/transform/all"
        && let Ok(dict) = result_py.cast::<PyDict>()
        && let Some(files) = dict.get_item("files")?
        && let Ok(files) = files.cast::<PyList>()
        && let Some(Value::Array(values)) = result.get("files")
    {
        for (file, value) in files.iter().zip(values) {
            if let Ok(file) = file.cast::<PyDict>() {
                as_bytes(file, value)?;
            }
        }
    }
    let events: Vec<Value> =
        std::mem::take(&mut *recorder.0.lock().unwrap_or_else(PoisonError::into_inner))
            .into_iter()
            .map(event_value)
            .collect::<serde_json::Result<_>>()
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok((result_py, to_py(py, &Value::Array(events))?))
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<NativePlugin>()?;
    m.add_class::<PyContext>()?;
    m.add_function(wrap_pyfunction!(serve, m)?)?;
    m.add_function(wrap_pyfunction!(call, m)?)?;
    m.add("PROTOCOL_VERSION", PROTOCOL_VERSION)?;
    Ok(())
}
