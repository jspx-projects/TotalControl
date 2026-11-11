//! Legacy mixer-layout slot files.
//!
//! `Layout <n>.json` is a migration input for the one-time import into the
//! current workspace.  It is not read or written after migration.

use std::collections::HashSet;
use std::path::PathBuf;

use totalcontrol_core::ChannelId;

fn layouts_dir() -> PathBuf {
    let dir = crate::scenes::scenes_dir()
        .parent()
        .map(|p| p.join("layouts"))
        .unwrap_or_else(|| PathBuf::from("layouts"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn load_layout_file(slot: u8) -> Option<HashSet<ChannelId>> {
    let content =
        std::fs::read_to_string(layouts_dir().join(format!("Layout {slot}.json"))).ok()?;
    serde_json::from_str(&content).ok()
}
