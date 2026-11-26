# AGENTS.md

Guidance for AI agents (and humans) working in lsdocs-plugin-sdk, one repository of
the lsdocs organization (https://github.com/lsdocs). It was split from the
lsdocs monorepo at commit d71dfa3; its history before that is the
monorepo's, filtered to these files. README.md says what it is and lists
its known limitations and TODO items.

## Working across repositories

Other lsdocs repositories are git dependencies
(`{ git = "https://github.com/lsdocs/<repo>", branch = "main" }`);
`Cargo.lock` pins their revisions (`nur bump [crate]` moves them).

- `nur link ../<repo>` builds against a local checkout (a `[patch]` in the
  untracked `.cargo/config.toml`); `nur unlink` undoes it.
- With `LSDOCS_ORG_DIR` set (e.g. `C:/dev`), Cargo fetches the
  organization's repositories from the local clones there (their
  committed `main`) instead of GitHub.
- `nur check-dupes` fails if a crate of the organization is built twice;
  `nur check-git-deps` fails on a dependency that is not a path inside
  this repository or a git dependency on the repository holding it
  (`scripts/org.nuon`), and on a committed `[patch]` table. `nur msrv`
  checks each crate on its own `rust-version`; `nur deny` runs
  cargo-deny (both are CI jobs of their own).

## Dev tasks

Dev tasks are nushell scripts run with `nur` (`nurfile`); `nur --list`
shows them. `nur ci` runs what CI runs: run it before committing. Tools:
`mise install` (see `.config/mise.toml`); Rust from rustup.

## Conventions

- Commits: Conventional Commits, `type(scope): summary`.
- Line endings: LF everywhere; scripts that write files write LF.
- Comments and docs: ASCII only.
- MSRV is per crate (`rust-version`); the workspace's is the base.
- Dependencies are declared in `[workspace.dependencies]` and used with
  `workspace = true`.
- Lints: the workspace lints apply to every crate; `missing_docs` is on;
  `unsafe_code` is denied.
- No panics outside tests: clippy denies `unwrap_used`, `expect_used` and
  `panic` in library, binary and example code. Tests opt in per module
  (`#![allow(clippy::unwrap_used)]`).
- Licensing: `Apache-2.0 OR MIT`.
- Publishing: merging release-plz's release PR publishes the bumped
  crates to crates.io (`.github/workflows/release.yml`) (not those
  with `publish = false`); a `lsdocs-plugin-python/v*` tag publishes
  the Python package to PyPI, a `lsdocs-plugin-node/v*` tag the
  Node.js packages to npm, and a `lsdocs-plugin-lua/v*` tag attaches the
  Lua modules and the rockspec to its GitHub release and uploads the
  rockspec to luarocks.org (`tag.yml`).
- Record what is left open in README.md's TODO section, and what the code
  does not do (or does differently) under Known limitations.

## This repository

| Path | What |
|---|---|
| `crates/rpc` | msgpack-rpc envelope, framing, `Peer`/`Router`/`Connection`, transports, stdout protection. |
| `crates/plugin-sdk` | `Parser`/`Transformer`/`Renderer` traits, `Plugin`, `Context`, `serve()`/`main()`. |
| `crates/plugin-sdk-bindings` | Value-based handlers -> SDK `Plugin`; shared by the bindings. |
| `bindings/python` | `lsdocs-plugin`: pyo3 extension + Python package (`python/lsdocs_plugin`), pytest suite. |
| `bindings/node` | `@lsdocs/plugin`: napi-rs addon + JS package (`index.js`, `index.d.ts`), `node:test` suite. |
| `bindings/lua` | `lsdocs-plugin` for Lua: mlua module `lsdocs_plugin`, Lua specs (`spec/`) run in an embedded Lua, `lsdocs-lua` (a `lua` with the module built in). |
| `tests/site` | The site the toc ports render (the bindings' `examples/`). |

Tasks: `nur test python`, `nur test node` and `nur test lua` build each
package and run its tests; `nur gen node` regenerates `bindings/node/native.{js,d.ts}`
after changing its `#[napi]` API (generated: never hand-edit them).
`unsafe_code` is denied except the stdout-protection syscalls in
`crates/rpc/src/transport.rs`.

### Bindings

- **All protocol behavior lives in Rust** (`rpc`, `plugin-sdk`,
  `plugin-sdk-bindings`). Bindings only convert values and adapt idioms;
  don't reimplement handshake, dispatch or capability logic in Python or
  JS.
- Python: handlers run on worker threads holding the GIL; `async def`
  handlers run on one shared asyncio loop (`_aio.py`). `max_concurrency`
  defaults to 1. Type stubs are hand-written (`_native.pyi`). A uv
  workspace is rooted at `pyproject.toml` (lockfile and venv at the root;
  `.venv` on Windows, `.venv-<os>` elsewhere).
- Node: values cross as JSON strings; handlers are weak threadsafe
  functions so they don't hold the event loop open. `index.d.ts` is
  hand-written; napi only generates `native.d.ts`.
- Each binding has a port of `transform-toc` (`examples/`) that must
  render `tests/site` **byte-identically** to the Rust plugin
  (lsdocs-transform-toc). Keep them in step when transform-toc changes.
- The toc port tests need an `lsdocs` binary: `LSDOCS_BIN`, or the tasks
  install lsdocs from git with this checkout's crates patched in. The
  fixture tests read lsdocs-protocol's `fixtures/` from the checkout
  Cargo uses (`LSDOCS_FIXTURES`, set by the tasks).
- `nur test node` builds the addon with plain cargo and copies it (napi's
  CLI fails on WSL's `/mnt/c`); run `nur gen node` on Windows or a native
  Linux filesystem.

### Testing gotchas

- `crates/rpc/tests/suite.rs` spawns the `echo` example as a child
  process. A filtered run (`cargo test --test suite`) does not rebuild
  examples; run `cargo build -p lsdocs-rpc --examples` first, or `nur test`.
