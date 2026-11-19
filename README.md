# lsdocs-plugin-sdk

Part of [lsdocs](https://github.com/lsdocs), a static-site generator whose
parsers, transformers and renderers are plugins.

The SDKs for writing lsdocs plugins:

| Path | What |
|---|---|
| `crates/plugin-sdk` | `lsdocs-plugin-sdk`: the Rust SDK (`Parser`, `Transformer`, `Renderer`, `Plugin`, `Context`, `serve()`, settings from `.config/`, testing helpers). |
| `crates/rpc` | `lsdocs-rpc`: msgpack-rpc envelope, framing, `Peer` / `Router` / `Connection`, transports (stdio, named pipe / unix socket), stdout protection. |
| `crates/plugin-sdk-bindings` | `lsdocs-plugin-sdk-bindings`: value-based handlers turned into an SDK `Plugin`, shared by the bindings. |
| `bindings/python` | `lsdocs-plugin` for Python (pyo3 extension + package `lsdocs_plugin`). |
| `bindings/node` | `@lsdocs/plugin` for Node.js (napi-rs addon + `index.js`, `index.d.ts`). |

All protocol behavior lives in Rust; the bindings convert values and
adapt idioms.

## Known limitations

- The Python and Node.js SDKs cannot declare syntax extensions (directives
  and roles); `Plugin::syntax` is Rust only.
- The Python and Node.js SDKs cannot generate documents.
- Node.js: context events (logs, progress, diagnostics) reach the host
  when the handler finishes, not as they happen.
- Python and Node.js types (`TypedDict`s in `_native.pyi`, `index.d.ts`)
  are written by hand, not generated from the protocol's schemas.
- Free-threaded Python builds have never been tried.

## TODO

- [ ] Syntax declarations from Python and Node.js plugins.
- [ ] Generated documents (`parse/transform/all`) from Python and Node.js
      plugins (only the Rust SDK has `Transformer::conclude`).
- [ ] Node.js: send context events as they happen.
- [ ] `TypedDict`s (Python) and `.d.ts` types (Node.js) generated from
      lsdocs-protocol's `schema/`, and type-checked against its fixtures.
- [ ] The binding packages declare their protocol version (now a string)
      in their package metadata.
- [ ] Free-threaded Python builds; consider a higher default
      `max_concurrency` there.
