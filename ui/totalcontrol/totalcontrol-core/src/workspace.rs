//! Current workspace files.
//!
//! The GUI owns the complete workspace schema; the core only needs the
//! scene inside one and the paths both interfaces resolve.  Keeping the
//! scene access here lets the TUI read the same current workspace the GUI
//! writes without duplicating the GUI's model.
//!
//! A current workspace is `<data>/workspaces/current-<unit-key>.tcws`.
//! Writes are atomic so a failure cannot truncate the live state.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::scene::Scene;

/// XDG data directory, `$XDG_DATA_HOME/totalcontrol` or
/// `~/.local/share/totalcontrol`.
pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("totalcontrol")
}

/// The directory holding current workspace files.
pub fn workspaces_dir() -> PathBuf {
    data_dir().join("workspaces")
}

/// Make a model/serial pair safe for a file name.
pub fn unit_key(model: &str, serial: Option<&str>) -> String {
    let safe = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    };
    let model = safe(model);
    match serial.map(safe).filter(|serial| !serial.is_empty()) {
        Some(serial) => format!("{model}-{serial}"),
        None => model,
    }
}

/// Default file for one unit.
pub fn current_path(unit_key: &str) -> PathBuf {
    workspaces_dir().join(format!("current-{unit_key}.tcws"))
}

/// `$XDG_CONFIG_HOME/totalcontrol/host-settings.json`.
pub fn host_settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("totalcontrol").join("host-settings.json"))
}

/// The current workspace file recorded for the active unit, if the host
/// settings file names one.
pub fn active_current_path() -> Option<PathBuf> {
    let text = std::fs::read_to_string(host_settings_path()?).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let active = value.get("active_unit")?.as_str()?;
    let path = value
        .get("units")?
        .get(active)?
        .get("current_workspace")?
        .as_str()?;
    Some(PathBuf::from(path))
}

/// The scene stored in a `.tcws` file.
pub fn read_scene(path: &Path) -> Option<Scene> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    serde_json::from_value(value.get("scene")?.clone()).ok()
}

/// Replace only the scene of an existing workspace, preserving every other
/// field.  Used by the TUI, which does not own the complete schema.
pub fn write_scene(path: &Path, scene: &Scene) -> Result<(), String> {
    let mut value: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| Value::Object(Default::default()));
    let object = value
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a workspace", path.display()))?;
    object.insert(
        "scene".into(),
        serde_json::to_value(scene).map_err(|error| error.to_string())?,
    );
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    write_atomic(path, &text)
}

/// Write a file through a sibling temporary and a rename.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)
        .map_err(|error| format!("Could not write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not replace {}: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_keys_are_file_safe_and_keep_the_serial_distinct() {
        assert_eq!(unit_key("Fireface UCX", Some("1234")), "Fireface-UCX-1234");
        assert_eq!(unit_key("Fireface UC", None), "Fireface-UC");
        assert_ne!(
            unit_key("Fireface UCX", Some("1")),
            unit_key("Fireface UCX", Some("2"))
        );
        assert!(!unit_key("Fireface UCX", Some("A/B")).contains('/'));
    }

    #[test]
    fn a_scene_can_be_replaced_without_losing_the_rest_of_the_workspace() {
        let dir = std::env::temp_dir().join(format!(
            "totalcontrol-core-workspace-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("current.tcws");
        std::fs::write(
            &path,
            r#"{"schema":1,"name":"Live","scene":{"name":"old"}}"#,
        )
        .unwrap();

        let mut scene = Scene::new("new");
        scene.model = "Fireface UCX".into();
        write_scene(&path, &scene).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["name"], "Live");
        assert_eq!(read_scene(&path).unwrap().name, "new");
        assert_eq!(read_scene(&path).unwrap().model, "Fireface UCX");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
