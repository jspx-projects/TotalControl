//! Host settings: installation and per-unit operational values that are not
//! workspace state.
//!
//! The current workspace owns the mixer scene, labels, groups, snapshots,
//! layouts and GUI state.  This file owns only the values that must survive
//! before a unit or workspace is known: the active unit, the OSC and engine
//! choices, the tray/startup choices, the last workspace folder, and the
//! per-unit current-workspace file and startup sync direction.
//!
//! `$XDG_CONFIG_HOME/totalcontrol/host-settings.json`.  The file is optional
//! on read: missing or unreadable values fall back to safe defaults, and an
//! older `gui.json` is read once by the migration path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::EngineRateMode;
use crate::osc::OscDialect;
use crate::prefs::SyncDirection;

pub const SCHEMA: u32 = 1;

fn schema() -> u32 {
    SCHEMA
}

fn recv_port() -> u16 {
    9000
}

fn send_port() -> u16 {
    9001
}

/// One unit's operational settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnitSettings {
    /// Canonical model name of the unit, used to pick it at the next start.
    pub model: String,
    /// Serial number when the backend exposes one.
    pub serial: Option<String>,
    /// The unit's current workspace, relative to the data directory when
    /// the default location is used.
    pub current_workspace: Option<PathBuf>,
    pub sync_direction: Option<SyncDirection>,
    pub always_init_device: bool,
}

/// The complete host settings document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSettings {
    #[serde(default = "schema")]
    pub schema: u32,
    /// Key in `units` for the last selected unit.
    pub active_unit: Option<String>,
    pub units: BTreeMap<String, UnitSettings>,
    pub osc_enabled: bool,
    pub osc_mode: OscDialect,
    #[serde(default = "recv_port")]
    pub osc_recv_port: u16,
    #[serde(default = "send_port")]
    pub osc_send_port: u16,
    pub engine_force_rate: Option<u32>,
    pub engine_rate_mode: Option<EngineRateMode>,
    pub engine_force_quantum: Option<u32>,
    pub keep_in_tray: Option<bool>,
    pub workspace_last_dir: Option<PathBuf>,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            active_unit: None,
            units: BTreeMap::new(),
            osc_enabled: false,
            osc_mode: OscDialect::Classic,
            osc_recv_port: recv_port(),
            osc_send_port: send_port(),
            engine_force_rate: None,
            engine_rate_mode: None,
            engine_force_quantum: None,
            keep_in_tray: None,
            workspace_last_dir: None,
        }
    }
}

impl HostSettings {
    pub fn path() -> Option<PathBuf> {
        totalcontrol_core::workspace::host_settings_path()
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(path: &Path, settings: &Self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
        totalcontrol_core::workspace::write_atomic(path, &json)
    }

    /// The unit key named by `active_unit`, when it exists.
    pub fn active(&self) -> Option<(&str, &UnitSettings)> {
        let key = self.active_unit.as_deref()?;
        self.units
            .get_key_value(key)
            .map(|(key, unit)| (key.as_str(), unit))
    }

    /// Remember a connected unit and make it active.
    pub fn set_active(&mut self, key: &str, model: &str, serial: Option<&str>) {
        self.active_unit = Some(key.to_string());
        let unit = self.units.entry(key.to_string()).or_default();
        unit.model = model.to_string();
        unit.serial = serial.map(str::to_string);
    }

    pub fn current_workspace(&self, key: &str) -> Option<PathBuf> {
        self.units
            .get(key)
            .and_then(|unit| unit.current_workspace.clone())
    }

    pub fn set_current_workspace(&mut self, key: &str, path: PathBuf) {
        self.units
            .entry(key.to_string())
            .or_default()
            .current_workspace = Some(path);
    }
}

/// Build host settings from the legacy `gui.json` once, before migration.
/// The caller records the connected unit and current workspace.
pub fn from_legacy(prefs: &crate::prefs::Prefs) -> HostSettings {
    HostSettings {
        osc_enabled: prefs.osc_enabled,
        osc_mode: prefs.osc_mode,
        osc_recv_port: if prefs.osc_recv_port == 0 {
            recv_port()
        } else {
            prefs.osc_recv_port
        },
        osc_send_port: if prefs.osc_send_port == 0 {
            send_port()
        } else {
            prefs.osc_send_port
        },
        engine_force_rate: prefs.engine_force_rate,
        engine_rate_mode: prefs.engine_rate_mode,
        engine_force_quantum: prefs.engine_force_quantum,
        keep_in_tray: prefs.keep_in_tray,
        workspace_last_dir: prefs.workspace_last_dir.clone(),
        ..HostSettings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_uses_defaults() {
        let settings = HostSettings::load(Path::new("/nonexistent/host-settings.json"));
        assert_eq!(settings.schema, SCHEMA);
        assert_eq!(settings.osc_recv_port, 9000);
        assert_eq!(settings.osc_send_port, 9001);
        assert!(settings.units.is_empty());
    }

    #[test]
    fn settings_round_trip_and_follow_the_active_unit() {
        let dir =
            std::env::temp_dir().join(format!("totalcontrol-host-settings-{}", std::process::id()));
        let path = dir.join("host-settings.json");
        let mut settings = HostSettings::default();
        settings.set_active("Fireface-UCX-1", "Fireface UCX", Some("1"));
        settings.set_current_workspace("Fireface-UCX-1", PathBuf::from("current.tcws"));
        settings.engine_force_rate = Some(96_000);
        HostSettings::save(&path, &settings).unwrap();
        let loaded = HostSettings::load(&path);
        assert_eq!(loaded, settings);
        assert_eq!(loaded.active().unwrap().0, "Fireface-UCX-1");
        assert_eq!(
            loaded.current_workspace("Fireface-UCX-1"),
            Some(PathBuf::from("current.tcws"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
