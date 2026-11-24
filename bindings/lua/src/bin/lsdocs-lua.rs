//! A `lua` with `lsdocs_plugin` built in: `lsdocs-lua <script> [args...]` runs the
//! script in an embedded Lua 5.4 (or LuaJIT, with `--features
//! luajit,vendored`), with `require("lsdocs_plugin")` resolving to this
//! crate and `arg` set as `lua` sets it. For running Lua plugins without a
//! Lua installed; the toc port test runs the transform-toc port with it.

use std::process::ExitCode;

use mlua::{Lua, Table};

fn run(script: &str, args: &[String]) -> mlua::Result<()> {
    let lua = Lua::new();
    let preload: Table = lua.globals().get::<Table>("package")?.get("preload")?;
    preload.set(
        "lsdocs_plugin",
        lua.create_function(|lua, ()| lsdocs_plugin_lua::open(lua))?,
    )?;
    let arg = lua.create_table()?;
    arg.set(0, script)?;
    for (i, a) in args.iter().enumerate() {
        arg.set(i + 1, a.as_str())?;
    }
    lua.globals().set("arg", arg)?;
    let source = std::fs::read(script).map_err(mlua::Error::external)?;
    lua.load(source).set_name(format!("@{script}")).exec()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((script, rest)) = args.split_first() else {
        eprintln!("usage: lsdocs-lua <script> [args...]");
        return ExitCode::from(2);
    };
    match run(script, rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lsdocs-lua: {e}");
            ExitCode::FAILURE
        }
    }
}
