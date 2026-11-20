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
    7001
}

fn send_port() -> u16 {
    9001
}

fn send_host() -> std::net::IpAddr {
    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

fn ovr_samples() -> u8 {
    3
}

/// Full-scale samples that trigger OVR, as exposed by TotalMix FX.
pub const OVR_SAMPLES_MIN: u8 = 1;
pub const OVR_SAMPLES_MAX: u8 = 15;

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

/// One controller option value: a checkbox boolean or a choice string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControllerValue {
    Bool(bool),
    Choice(String),
}

/// One device's controller choice: input and output enable plus the
/// protocol id. The key is `device:<i>` for the open unit's ports and
/// `<card_id>:<i>` for a separate USB card.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceChoice {
    pub input: bool,
    pub output: bool,
    pub protocol: String,
}

/// The generic MIDI controller settings (Settings > MIDI and OSC, MIDI
/// part). Option values are keyed by protocol id and option key;
/// assignments by protocol id and control id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControllerSettings {
    /// Master enable of the MIDI part.
    pub enabled: bool,
    pub show_log: bool,
    pub devices: BTreeMap<String, DeviceChoice>,
    pub values: BTreeMap<String, BTreeMap<String, ControllerValue>>,
    pub assignments: BTreeMap<String, BTreeMap<String, String>>,
}

impl ControllerSettings {
    pub fn device(&self, key: &str) -> DeviceChoice {
        self.devices.get(key).cloned().unwrap_or_default()
    }

    pub fn values_for(&self, protocol: &str) -> BTreeMap<String, ControllerValue> {
        self.values.get(protocol).cloned().unwrap_or_default()
    }

    pub fn assignments_for(&self, protocol: &str) -> BTreeMap<String, String> {
        self.assignments.get(protocol).cloned().unwrap_or_default()
    }

    /// Fold the legacy `mackie` block into the generic settings once.
    pub fn migrate_mackie(&mut self, legacy: &crate::mackie::MackieConfig) {
        if !self.devices.is_empty() {
            return;
        }
        self.enabled = legacy.enabled;
        for i in 0..crate::mackie::PORTS {
            self.devices.insert(
                format!("device:{i}"),
                DeviceChoice {
                    input: legacy.in_enabled[i],
                    output: legacy.out_enabled[i],
                    protocol: "mackie_control".into(),
                },
            );
        }
        let bools = [
            ("protocol", legacy.protocol),
            ("full_lcd", legacy.full_lcd),
            ("send_names", legacy.send_names),
            ("stereo_to_mono", legacy.stereo_to_mono),
            ("submix_any_row", legacy.submix_any_row),
            ("background_disable", legacy.background_disable),
            ("send_levels", legacy.send_levels),
            ("peak_hold", legacy.peak_hold),
            ("submix_linked", legacy.submix_linked),
        ];
        let entry = self.values.entry("mackie_control".into()).or_default();
        for (key, value) in bools {
            entry.insert(key.into(), ControllerValue::Bool(value));
        }
    }
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
    /// Feedback destination of the OSC surface.  Loopback keeps the
    /// receive socket on loopback; any other address binds it on all
    /// interfaces.
    #[serde(default = "send_host")]
    pub osc_send_host: std::net::IpAddr,
    /// The generic MIDI controller settings (Settings > MIDI and OSC, MIDI
    /// part).
    pub controllers: ControllerSettings,
    /// The legacy Mackie block: read once and folded into `controllers`.
    #[serde(default, skip_serializing)]
    pub mackie: Option<crate::mackie::MackieConfig>,
    pub engine_force_rate: Option<u32>,
    pub engine_rate_mode: Option<EngineRateMode>,
    pub engine_force_quantum: Option<u32>,
    /// Consecutive full-scale samples that trigger OVR.  This is a global
    /// meter preference, shared by every channel and unit.
    #[serde(default = "ovr_samples")]
    pub ovr_full_scale_samples: u8,
    pub keep_in_tray: Option<bool>,
    /// Opt-in host audio performance mode: holds a PM-QoS CPU latency
    /// constraint while the card streams (`audio_perf`).
    #[serde(default)]
    pub audio_performance: bool,
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
            osc_send_host: send_host(),
            controllers: ControllerSettings::default(),
            mackie: None,
            engine_force_rate: None,
            engine_rate_mode: None,
            engine_force_quantum: None,
            ovr_full_scale_samples: ovr_samples(),
            keep_in_tray: None,
            audio_performance: false,
            workspace_last_dir: None,
        }
    }
}

impl HostSettings {
    pub fn path() -> Option<PathBuf> {
        totalcontrol_core::workspace::host_settings_path()
    }

    pub fn load(path: &Path) -> Self {
        let mut settings: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if let Some(legacy) = settings.mackie.take() {
            settings.controllers.migrate_mackie(&legacy);
        }
        settings
    }

    /// The global OVR trigger count, clamped to the TotalMix FX range.
    pub fn ovr_sample_count(&self) -> u8 {
        self.ovr_full_scale_samples
            .clamp(OVR_SAMPLES_MIN, OVR_SAMPLES_MAX)
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
        assert_eq!(settings.osc_recv_port, 7001);
        assert_eq!(settings.osc_send_port, 9001);
        assert_eq!(settings.ovr_sample_count(), 3);
        assert!(settings.units.is_empty());
    }

    #[test]
    fn the_global_ovr_sample_count_is_clamped() {
        let mut settings = HostSettings::default();
        settings.ovr_full_scale_samples = 0;
        assert_eq!(settings.ovr_sample_count(), OVR_SAMPLES_MIN);
        settings.ovr_full_scale_samples = 99;
        assert_eq!(settings.ovr_sample_count(), OVR_SAMPLES_MAX);
    }

    #[test]
    fn the_controller_settings_round_trip() {
        let mut settings = HostSettings::default();
        settings.controllers.enabled = true;
        settings.controllers.devices.insert(
            "device:0".into(),
            DeviceChoice { input: true, output: true, protocol: "mackie_control".into() },
        );
        settings.controllers.values.entry("mackie_control".into()).or_default().insert(
            "full_lcd".into(),
            ControllerValue::Bool(false),
        );
        settings.controllers.assignments.entry("rme_arc_usb".into()).or_default().insert(
            "key1".into(),
            "dim".into(),
        );
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: HostSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.controllers, settings.controllers);
        // An older file without the controllers block keeps the defaults.
        let legacy: HostSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.controllers, ControllerSettings::default());
        assert_eq!(legacy.osc_recv_port, 7001);
    }

    #[test]
    fn the_legacy_mackie_block_migrates_once() {
        let legacy = crate::mackie::MackieConfig {
            enabled: true,
            in_enabled: [true, false],
            out_enabled: [true, true],
            full_lcd: false,
            send_levels: true,
            ..crate::mackie::MackieConfig::default()
        };
        let json = format!("{{\"mackie\": {}}}", serde_json::to_string(&legacy).unwrap());
        let mut settings: HostSettings = serde_json::from_str(&json).unwrap();
        if let Some(old) = settings.mackie.take() {
            settings.controllers.migrate_mackie(&old);
        }
        assert!(settings.controllers.enabled);
        assert_eq!(settings.controllers.device("device:0").input, true);
        assert_eq!(settings.controllers.device("device:1").input, false);
        assert_eq!(settings.controllers.device("device:0").protocol, "mackie_control");
        let values = settings.controllers.values_for("mackie_control");
        assert_eq!(values.get("full_lcd"), Some(&ControllerValue::Bool(false)));
        assert_eq!(values.get("send_levels"), Some(&ControllerValue::Bool(true)));
        assert_eq!(values.get("protocol"), Some(&ControllerValue::Bool(true)));
        // The migrated file no longer carries the legacy block.
        let rewritten = serde_json::to_string(&settings).unwrap();
        assert!(!rewritten.contains("\"mackie\""));
        // A second migration does not overwrite existing choices.
        settings.controllers.migrate_mackie(&crate::mackie::MackieConfig::default());
        assert!(settings.controllers.enabled);
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
        settings.ovr_full_scale_samples = 5;
        HostSettings::save(&path, &settings).unwrap();
        let loaded = HostSettings::load(&path);
        assert_eq!(loaded, settings);
        assert_eq!(loaded.ovr_sample_count(), 5);
        assert_eq!(loaded.active().unwrap().0, "Fireface-UCX-1");
        assert_eq!(
            loaded.current_workspace("Fireface-UCX-1"),
            Some(PathBuf::from("current.tcws"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
