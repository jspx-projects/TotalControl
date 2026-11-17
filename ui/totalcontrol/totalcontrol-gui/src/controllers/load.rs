//! The definition loader: search order, validation and the embedded
//! fallback. See `TOTALCONTROL-CONTROLLER-SPEC.md` §6–7.

use std::path::PathBuf;

use super::map::ControllerMap;

/// The shipped Mackie definition, embedded so a relocated binary without a
/// data directory still finds it.
pub const EMBEDDED_MACKIE: &str =
    include_str!("../../../../../data/midi-controllers/Mackie_MIDI.json");

/// The shipped ARC USB definition, embedded like the Mackie one.
pub const EMBEDDED_ARC: &str =
    include_str!("../../../../../data/midi-controllers/RME_ARC_USB_MIDI.json");

/// One loaded protocol, or the reason it is unavailable.
#[derive(Debug, Clone)]
pub struct LoadedProtocol {
    pub map: Option<ControllerMap>,
    pub file_name: String,
    pub path: String,
    pub error: Option<String>,
}

impl LoadedProtocol {
    pub fn id(&self) -> &str {
        self.map.as_ref().map(|m| m.id.as_str()).unwrap_or("")
    }

    pub fn name(&self) -> &str {
        match &self.map {
            Some(m) => &m.name,
            None => &self.file_name,
        }
    }
}

/// The user data directory for controller definitions.
pub fn user_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs_home().map(|h| h.join(".local/share")))?;
    Some(base.join("totalcontrol").join("midi-controllers"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The installed data directory, compiled in from the build's datadir.
pub fn data_dir() -> PathBuf {
    PathBuf::from(option_env!("TOTALCONTROL_DATADIR").unwrap_or("/usr/local/share"))
        .join("totalcontrol")
        .join("midi-controllers")
}

/// The search order of §6.2: override env, user, installed.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(env) = std::env::var_os("TOTALCONTROL_CONTROLLERS") {
        dirs.push(PathBuf::from(env));
    }
    if let Some(user) = user_dir() {
        dirs.push(user);
    }
    dirs.push(data_dir());
    dirs
}

/// Load every definition. First match by file name wins; the embedded
/// definitions fill in when no directory holds them. A malformed file is
/// listed as unavailable with the reason.
pub fn load_all() -> Vec<LoadedProtocol> {
    let mut by_name: Vec<(String, String, String)> = Vec::new(); // name, path, text
    for dir in search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        names.sort();
        for path in names {
            let Some(file_name) = path.file_name().map(|n| n.to_string_lossy().into_owned())
            else {
                continue;
            };
            if by_name.iter().any(|(n, _, _)| *n == file_name) {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => by_name.push((
                    file_name,
                    path.to_string_lossy().into_owned(),
                    text,
                )),
                Err(err) => by_name.push((
                    file_name,
                    path.to_string_lossy().into_owned(),
                    format!("{{\"__read_error\": \"{err}\"}}"),
                )),
            }
        }
    }
    // The embedded fallback.
    if !by_name.iter().any(|(n, _, _)| n == "Mackie_MIDI.json") {
        by_name.push((
            "Mackie_MIDI.json".into(),
            "embedded".into(),
            EMBEDDED_MACKIE.into(),
        ));
    }
    if !by_name.iter().any(|(n, _, _)| n == "RME_ARC_USB_MIDI.json") {
        by_name.push((
            "RME_ARC_USB_MIDI.json".into(),
            "embedded".into(),
            EMBEDDED_ARC.into(),
        ));
    }

    let mut loaded: Vec<LoadedProtocol> = Vec::new();
    for (file_name, path, text) in by_name {
        if text.contains("__read_error") {
            loaded.push(LoadedProtocol {
                map: None,
                file_name,
                path,
                error: Some("cannot read the file".into()),
            });
            continue;
        }
        match ControllerMap::parse(&text).and_then(|m| {
            let mut m = m;
            m.file_name = file_name.clone();
            m.path = path.clone();
            m.validate()
        }) {
            Ok(map) => loaded.push(LoadedProtocol {
                map: Some(map),
                file_name,
                path,
                error: None,
            }),
            Err(error) => {
                log::warn!("controller map {file_name}: {error}");
                loaded.push(LoadedProtocol {
                    map: None,
                    file_name,
                    path,
                    error: Some(error),
                });
            }
        }
    }
    // Duplicate ids: the first file wins, the rest are unavailable.
    let mut seen_ids: Vec<String> = Vec::new();
    for entry in &mut loaded {
        if let Some(map) = &entry.map {
            if seen_ids.contains(&map.id) {
                entry.error = Some(format!("duplicate protocol id '{}'", map.id));
                entry.map = None;
            } else {
                seen_ids.push(map.id.clone());
            }
        }
    }
    loaded.sort_by(|a, b| a.name().cmp(b.name()));
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env-override tests share one process environment.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_embedded_mackie_loads() {
        let map = ControllerMap::parse(EMBEDDED_MACKIE)
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(map.id, "mackie_control");
    }

    #[test]
    fn the_env_override_wins() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("tc-controllers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A user file with the shipped name overrides it.
        let text = EMBEDDED_MACKIE.replace("\"version\": \"1.0\"", "\"version\": \"9.9\"");
        std::fs::write(dir.join("Mackie_MIDI.json"), text).unwrap();
        std::env::set_var("TOTALCONTROL_CONTROLLERS", &dir);
        let loaded = load_all();
        std::env::remove_var("TOTALCONTROL_CONTROLLERS");
        let mackie = loaded
            .iter()
            .find(|p| p.id() == "mackie_control")
            .expect("mackie loads");
        assert_eq!(mackie.map.as_ref().unwrap().version, "9.9");
        assert!(mackie.path.starts_with(dir.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_file_is_unavailable_with_a_reason() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("tc-controllers-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Broken.json"), "{ not json").unwrap();
        std::env::set_var("TOTALCONTROL_CONTROLLERS", &dir);
        let loaded = load_all();
        std::env::remove_var("TOTALCONTROL_CONTROLLERS");
        let broken = loaded
            .iter()
            .find(|p| p.file_name == "Broken.json")
            .expect("listed");
        assert!(broken.map.is_none());
        assert!(broken.error.is_some());
        // The embedded Mackie still loads.
        assert!(loaded.iter().any(|p| p.id() == "mackie_control"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}