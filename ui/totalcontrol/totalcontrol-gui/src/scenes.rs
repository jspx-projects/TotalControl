//! Legacy snapshot slot files.
//!
//! `Mix <n>.json` is a migration input for the one-time import into the
//! current workspace.  It is not read or written after migration.

use std::path::PathBuf;
use totalcontrol_core::Scene;

pub fn scenes_dir() -> PathBuf {
    totalcontrol_core::scene::scenes_dir()
}

pub fn load_scene_file(name: &str) -> Option<Scene> {
    let content = std::fs::read_to_string(scenes_dir().join(format!("{}.json", name))).ok()?;
    Scene::from_json(&content).ok()
}
