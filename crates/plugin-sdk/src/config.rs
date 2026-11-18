//! Reading a plugin's settings from the site's `.config/` directory.
//!
//! The host says where `.config/` is ([`ConfigInfo`]); what a plugin keeps
//! there, and in what format, is its own business. The recommended layout
//! is `<vendor>/<name>/`, `name` being the plugin's name in the site
//! config, so that two instances of a plugin keep separate settings:
//! [`recommended`] builds such a path for lsdocs' own plugins, and
//! [`toml_file`] reads one TOML file. A plugin registers a loader with
//! [`Plugin::settings`](crate::Plugin::settings); it runs once, at
//! `initialize`.

use std::path::Path;

use lsdocs_protocol::ConfigInfo;
use serde_json::Value;

/// A plugin's settings, as its loader read them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    /// The settings, as the plugin wants them (null for none).
    pub settings: Value,
    /// The files read (relative to `.config/`, or absolute), including
    /// ones looked for and not found: the host's cache keys cover them.
    pub files: Vec<String>,
}

/// `lsdocs/<name>/<file>`: where lsdocs' own plugins keep `file`.
pub fn recommended(config: &ConfigInfo, file: &str) -> String {
    format!("lsdocs/{}/{file}", config.name)
}

/// Reads `<.config>/<relative>` as TOML. A missing file is no settings
/// (null), still listed in [`Loaded::files`]; a file that is not TOML is
/// an error naming it.
pub fn toml_file(config: &ConfigInfo, relative: &str) -> Result<Loaded, String> {
    let path = Path::new(&config.dir).join(relative);
    let settings = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let table: toml::Table =
                toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::to_value(table).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    Ok(Loaded {
        settings,
        files: vec![relative.to_owned()],
    })
}

/// lsdocs' own plugins' loader: `lsdocs/<name>/settings.toml`, as TOML.
pub fn settings_toml(config: &ConfigInfo) -> Result<Loaded, String> {
    toml_file(config, &recommended(config, "settings.toml"))
}

/// [`settings_toml`], checked against `T` (unknown or bad keys are an
/// error naming the file): a loader for plugins with a settings type.
pub fn settings_toml_as<T: serde::de::DeserializeOwned + Default>(
    config: &ConfigInfo,
) -> Result<Loaded, String> {
    let loaded = settings_toml(config)?;
    parse::<T>(&loaded.settings, &shown(config, &loaded))?;
    Ok(loaded)
}

/// [`settings_toml`], with only `keys` allowed (any other is an error
/// naming the file): a loader for plugins reading a few keys.
pub fn settings_toml_keys(config: &ConfigInfo, keys: &[&str]) -> Result<Loaded, String> {
    let loaded = settings_toml(config)?;
    if let Value::Object(table) = &loaded.settings
        && let Some(unknown) = table.keys().find(|k| !keys.contains(&k.as_str()))
    {
        return Err(format!(
            "{}: unknown setting `{unknown}` (known: {})",
            shown(config, &loaded),
            keys.join(", ")
        ));
    }
    Ok(loaded)
}

/// The settings file's path, for messages.
fn shown(config: &ConfigInfo, loaded: &Loaded) -> String {
    let file = loaded.files.first().map(String::as_str).unwrap_or_default();
    Path::new(&config.dir).join(file).display().to_string()
}

/// Reads `settings` (as a loader returned them) into a type, null being
/// its default; an unknown or bad key is an error naming `file`.
pub fn parse<T: serde::de::DeserializeOwned + Default>(
    settings: &Value,
    file: &str,
) -> Result<T, String> {
    if settings.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(settings.clone()).map_err(|e| format!("{file}: {e}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn reads_toml_and_lists_missing_files() {
        let dir = std::env::temp_dir().join(format!("lsdocs-config-{}", std::process::id()));
        let config = ConfigInfo {
            dir: dir.to_string_lossy().into_owned(),
            name: "x".into(),
        };
        let file = recommended(&config, "settings.toml");
        assert_eq!(file, "lsdocs/x/settings.toml");
        let missing = toml_file(&config, &file).unwrap();
        assert_eq!(missing.settings, Value::Null);
        assert_eq!(missing.files, ["lsdocs/x/settings.toml"]);
        std::fs::create_dir_all(dir.join("lsdocs/x")).unwrap();
        std::fs::write(dir.join(&file), "depth = 2\n").unwrap();
        assert_eq!(
            toml_file(&config, &file).unwrap().settings,
            serde_json::json!({"depth": 2})
        );
        std::fs::write(dir.join(&file), "depth = \n").unwrap();
        let err = toml_file(&config, &file).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("settings.toml"), "{err}");
    }

    #[test]
    fn unknown_keys_name_the_file() {
        let dir = std::env::temp_dir().join(format!("lsdocs-config-k-{}", std::process::id()));
        let config = ConfigInfo {
            dir: dir.to_string_lossy().into_owned(),
            name: "y".into(),
        };
        std::fs::create_dir_all(dir.join("lsdocs/y")).unwrap();
        std::fs::write(dir.join("lsdocs/y/settings.toml"), "depht = 2\n").unwrap();
        let err = settings_toml_keys(&config, &["depth"]).unwrap_err();
        #[derive(serde::Deserialize, Default)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct S {
            depth: Option<u32>,
        }
        let typed = settings_toml_as::<S>(&config).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            err.contains("settings.toml") && err.contains("`depht`"),
            "{err}"
        );
        assert!(
            typed.contains("settings.toml") && typed.contains("depht"),
            "{typed}"
        );
    }
}
