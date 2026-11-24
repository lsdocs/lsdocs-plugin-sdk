//! The Lua port of transform-toc produces the same site as the Rust one.
//!
//! Builds tests/site (at the repository root) with the builtin Rust
//! transformer, then with examples/transform_toc.lua run out-of-process by
//! lsdocs-lua (src/bin, a `lua` with the module built in), over
//! stdio and over a pipe, and compares every output file. Needs the
//! `lsdocs` binary: set LSDOCS_BIN (`nur test lua` does); skipped without.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A TOML literal string (single quotes keep Windows paths intact).
fn toml_str(path: &Path) -> String {
    format!("'{}'", path.display())
}

fn config(toc_plugin: &str) -> String {
    format!(
        r#"
[site]
title = "lsdocs sample"

[build]
source_dir = {}

[[plugins]]
name = "markdown"
builtin = "parser-markdown"

[[plugins]]
name = "toc"
{toc_plugin}

[[plugins]]
name = "html"
builtin = "render-html"
"#,
        toml_str(&root().join("tests").join("site"))
    )
}

fn files(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, base, out);
        } else {
            let name = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(name, std::fs::read(&path).unwrap());
        }
    }
}

fn build(lsdocs: &str, tmp: &Path, label: &str, toc_plugin: &str) -> BTreeMap<String, Vec<u8>> {
    let site = tmp.join(label);
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("lsdocs.toml"), config(toc_plugin)).unwrap();
    // Not the default depth (3), so the port must read its settings.
    let settings = site.join(".config/lsdocs/toc/settings.toml");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, "depth = 2\ninline = true\n").unwrap();
    let output = Command::new(lsdocs)
        .arg("build")
        .arg("--config")
        .arg(site.join("lsdocs.toml"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{label}: {stderr}");
    assert!(
        !stderr.contains("warning") && !stderr.contains("error"),
        "{label}: {stderr}"
    );
    let mut out = BTreeMap::new();
    let built = site.join("site");
    files(&built, &built, &mut out);
    out
}

#[test]
fn lua_port_matches_rust() {
    let Ok(lsdocs) = std::env::var("LSDOCS_BIN") else {
        eprintln!("skipped: LSDOCS_BIN is not set");
        return;
    };
    let lua = PathBuf::from(env!("CARGO_BIN_EXE_lsdocs-lua"));
    let port = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/transform_toc.lua");
    let tmp = std::env::temp_dir().join(format!("lsdocs-lua-toc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    let rust = build(&lsdocs, &tmp, "rust", r#"builtin = "transform-toc""#);
    let index = String::from_utf8_lossy(&rust["index.html"]);
    assert!(index.contains(r#"<div class="lsd-toc">"#), "{index}");

    for transport in ["stdio", "pipe"] {
        let plugin = format!(
            "command = [{}, {}]\ntransport = \"{transport}\"",
            toml_str(&lua),
            toml_str(&port)
        );
        let lua_site = build(&lsdocs, &tmp, &format!("lua-{transport}"), &plugin);
        assert_eq!(
            lua_site.keys().collect::<Vec<_>>(),
            rust.keys().collect::<Vec<_>>(),
            "files over {transport}"
        );
        for (name, bytes) in &rust {
            assert!(
                &lua_site[name] == bytes,
                "{name} differs over {transport}:\n--- rust\n{}\n--- lua\n{}",
                String::from_utf8_lossy(bytes),
                String::from_utf8_lossy(&lua_site[name])
            );
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
