//! Controller definition files: types, validation and the decode table.
//!
//! A definition describes one controller model as data. The engine in
//! [`super::engine`] runs every file through the same path. The format is
//! `TOTALCONTROL-CONTROLLER-SPEC.md` §4.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use super::expr::Expr;

pub const SCHEMA: u32 = 1;

/// The action vocabulary, fixed by the engine (§4.9).
pub const ACTIONS: &[&str] = &[
    "none",
    "snapshot",
    "dim",
    "mono",
    "talkback",
    "speaker_b",
    "external_input",
    "recall_volume",
    "mute_main",
    "global_mute",
    "global_solo",
    "mute_group",
    "solo_group",
    "fader_group",
    "cue_main",
    "select_cue_main",
    "cue_phones",
    "mute_phones",
    "encoder_target",
    "encoder_target_volume",
    "layout_preset",
    "toggle_window",
    "strip_volume",
    "strip_pan",
    "strip_pan_center",
    "strip_mute",
    "strip_solo",
    "strip_select",
    "row_fader",
    "master_volume",
    "master_mute",
    "master_solo",
    "input_gain",
    "navigate",
    "trim_mode",
    "trim_on",
    "trim_off",
    "submix_select",
    "submix_select_index",
];

/// Actions the model does not carry yet: listed, marked unavailable, inert.
pub const UNAVAILABLE_ACTIONS: &[&str] =
    &["mute_group", "solo_group", "fader_group", "toggle_window", "mute_phones"];

/// The engine behaviours an option may name (§4.11).
pub const BEHAVIOURS: &[&str] = &[
    "protocol_support",
    "full_lcd",
    "send_user_names",
    "send_level_messages",
    "peak_hold",
    "stereo_to_mono",
    "submix_any_row",
    "submix_linked",
    "background_disable",
    "led_feedback",
    "encoder_target",
    "encoder_step",
    "encoder_follows_selection",
    "recall_double_press",
    "enable_diag_leds",
    "snapshot_blink_store",
];

/// The feedback state vocabulary (§4.9.2).
pub const STATES: &[&str] = &[
    "action_state",
    "strip.mute",
    "strip.solo",
    "strip.select",
    "strip.rec",
    "strip.fader",
    "strip.pan",
    "master.fader",
    "main.dim",
    "main.talkback",
    "main.mono",
    "main.speaker_b",
    "main.mute",
    "main.solo",
    "main.rude_solo",
    "nav.trim",
    "meter",
    "lcd",
    "loopback",
];

pub const NAVIGATE_ARGS: &[&str] = &[
    "row_up",
    "row_down",
    "bank_left",
    "bank_right",
    "channel_left",
    "channel_right",
];

pub const ENCODER_TARGETS: &[&str] =
    &["main", "phones1", "phones2", "phones3", "phones4", "none"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Remote,
    Surface,
    Keys,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Note,
    Cc,
    CcPair,
    PitchBend,
    ChannelPressure,
    ProgramChange,
    Sysex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Note,
    Cc,
    Channel,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Press,
    Momentary,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Press
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Expand {
    pub axis: Axis,
    pub count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModeOption {
    pub key: String,
    pub value: String,
}

/// A constant, an option reference or an address expression (§4.3).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Int(i64),
    Text(String),
    OptionRef { option: String },
    Expr { expr: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Control {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub kind: Kind,
    #[serde(default)]
    pub note: Option<u8>,
    #[serde(default)]
    pub message: Option<u8>,
    #[serde(default)]
    pub channel: Option<u8>,
    #[serde(default)]
    pub channel_range: Option<(u8, u8)>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub value_byte: Option<usize>,
    #[serde(default)]
    pub expand: Option<Expand>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub press: Option<u8>,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub mode_option: Option<ModeOption>,
    /// The control works while `protocol_support` is off (the simple CC
    /// path of the Mackie document).
    #[serde(default)]
    pub simple: bool,
    pub action: String,
    #[serde(default)]
    pub argument: Option<Argument>,
}

impl Control {
    /// The press threshold: at or above it the control is pressed.
    pub fn press_threshold(&self) -> u8 {
        self.press.unwrap_or(64)
    }

    /// Whether the control fires value events rather than press/release.
    pub fn is_value(&self) -> bool {
        self.value.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum NoteAddress {
    Num(u8),
    Text(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Address {
    #[serde(default)]
    pub note: Option<NoteAddress>,
    #[serde(default)]
    pub cc: Option<u8>,
    #[serde(default)]
    pub channel: Option<u8>,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub expand: Option<Expand>,
    #[serde(default)]
    pub index_nibble: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Feedback {
    pub id: String,
    #[serde(default)]
    pub enabled_by: Option<String>,
    pub kind: Kind,
    pub address: Address,
    pub state: String,
    pub value: String,
    /// Full state after connect; the engine realizes this by clearing the
    /// diff cache, so every active entry re-emits on the first refresh.
    #[allow(dead_code)]
    #[serde(default = "default_true")]
    pub sync: bool,
    #[serde(default)]
    pub rate_hz: Option<f32>,
    #[serde(default)]
    pub period_ms: Option<u64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DefaultValue {
    Bool(bool),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    Checkbox,
    Radio,
    Listbox,
    Text,
    Button,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OptionEntry {
    pub key: String,
    pub label: String,
    /// Tooltip text; schema 1 rendering shows the label only.
    #[allow(dead_code)]
    #[serde(default)]
    pub help: Option<String>,
    pub form: Form,
    #[serde(default)]
    pub group: Option<String>,
    /// Row break; schema 1 renders one entry per row.
    #[allow(dead_code)]
    #[serde(default, rename = "break")]
    pub break_row: bool,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub default: Option<DefaultValue>,
    #[serde(default)]
    pub requires: Option<String>,
    #[serde(default)]
    pub maps_to: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Assignment {
    #[serde(default)]
    pub enabled_by: Option<String>,
    pub controls: Vec<String>,
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Numeric {
    /// `absolute`, `relative`, `text` or `constant`; validated at load.
    #[allow(dead_code)]
    pub kind: String,
    #[serde(default)]
    pub encoding: Option<String>,
    /// `low_first` (default) or `high_first` for 14-bit values.
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub max_step: Option<u8>,
    #[serde(default)]
    pub value: Option<u8>,
}

pub const NUMERIC_ENCODINGS: &[&str] = &[
    "absolute_7bit",
    "absolute_14bit",
    "relative_7bit",
    "relative_signed_bit",
    "ring_position",
    "mackie_meter_nibble",
    "ascii",
];

#[derive(Debug, Clone, Deserialize)]
pub struct BoolValue {
    /// `note` or `cc`; validated at load.
    #[allow(dead_code)]
    pub kind: String,
    pub velocity: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoolEncoding {
    pub on: BoolValue,
    pub off: BoolValue,
    /// The third state, reserved for pending-store blink.
    #[allow(dead_code)]
    #[serde(default)]
    pub blink: Option<BoolValue>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortMatch {
    #[serde(default)]
    pub card_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PortBlock {
    #[serde(default = "default_port_source")]
    pub source: String,
    #[serde(default, rename = "match")]
    pub match_: Option<PortMatch>,
    /// Declared device count; discovery enumerates the actual nodes.
    #[allow(dead_code)]
    #[serde(default = "default_devices")]
    pub devices: usize,
    /// Declared direction; the worker opens both directions when enabled.
    #[allow(dead_code)]
    #[serde(default = "default_direction")]
    pub direction: String,
}

fn default_port_source() -> String {
    "device".into()
}
fn default_devices() -> usize {
    1
}
fn default_direction() -> String {
    "duplex".into()
}

impl Default for PortBlock {
    fn default() -> Self {
        Self {
            source: default_port_source(),
            match_: None,
            devices: 1,
            direction: default_direction(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extra {
    #[serde(default)]
    pub handshake: Option<String>,
    /// Protocol documentation data, carried for the definition's readers.
    #[allow(dead_code)]
    #[serde(default)]
    pub tables: serde_json::Value,
}

/// One expanded control instance: a control plus its expansion index and
/// concrete address.
#[derive(Debug, Clone)]
pub struct Expanded {
    pub control: usize,
    pub index: usize,
    pub channel: Option<u8>,
    pub note: Option<u8>,
    pub cc: Option<u8>,
}

/// The message class of a decode key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Note,
    Cc,
    PitchBend,
    ChannelPressure,
    ProgramChange,
}

/// A decoded message address: class, channel (None = any) and note/CC.
pub type DecodeKey = (Class, Option<u8>, u8);

/// A parsed and validated definition with its derived decode table.
#[derive(Debug, Clone, Deserialize)]
pub struct ControllerMap {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub shape: Shape,
    #[serde(default, rename = "match")]
    pub match_fragments: Vec<String>,
    /// Strip count of a surface; the engine reads the expanded controls.
    #[allow(dead_code)]
    #[serde(default)]
    pub channels: usize,
    /// Port declaration metadata; discovery enumerates the actual nodes.
    #[allow(dead_code)]
    #[serde(default)]
    pub port: PortBlock,
    pub options: Vec<OptionEntry>,
    #[serde(default)]
    pub assignment: Option<Assignment>,
    pub controls: Vec<Control>,
    #[serde(default)]
    pub feedback: Vec<Feedback>,
    #[serde(default)]
    pub numeric: BTreeMap<String, Numeric>,
    #[serde(default)]
    pub boolean_encodings: BTreeMap<String, BoolEncoding>,
    #[serde(default)]
    pub extra: Extra,

    /// Derived by [`ControllerMap::validate`]; not part of the file.
    #[serde(skip)]
    pub expanded: Vec<Expanded>,
    #[serde(skip)]
    pub decode: HashMap<DecodeKey, usize>,
    #[serde(skip)]
    pub sysex_patterns: Vec<(usize, Vec<Option<u8>>)>,
    #[serde(skip)]
    pub exprs: HashMap<usize, Expr>,
    /// The file name and resolved path, set by the loader.
    #[serde(skip)]
    pub file_name: String,
    #[serde(skip)]
    pub path: String,
}

impl ControllerMap {
    /// Parse the JSON text. Unknown keys are logged and ignored; structural
    /// errors are returned.
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("JSON: {e}"))?;
        warn_unknown_keys(&value);
        let map: Self = serde_json::from_value(value).map_err(|e| format!("format: {e}"))?;
        Ok(map)
    }

    /// Validate against §4 and build the decode table. Returns every error
    /// found, so a contributor sees all of them at once.
    pub fn validate(mut self) -> Result<Self, String> {
        let mut errors: Vec<String> = Vec::new();
        if self.schema != SCHEMA {
            errors.push(format!(
                "schema {} is not supported (engine schema {SCHEMA})",
                self.schema
            ));
        }
        if self.id.is_empty() || self.name.is_empty() || self.version.is_empty() {
            errors.push("id, name and version are required".into());
        }
        if !matches!(self.port.source.as_str(), "device" | "card") {
            errors.push(format!("unknown port.source '{}'", self.port.source));
        }
        if let Some(h) = &self.extra.handshake {
            if h != "mackie" {
                errors.push(format!("unknown handshake '{h}'"));
            }
        }

        // Options.
        let mut option_keys: Vec<&str> = Vec::new();
        for opt in &self.options {
            if option_keys.contains(&opt.key.as_str()) {
                errors.push(format!("duplicate option key '{}'", opt.key));
            }
            option_keys.push(&opt.key);
            match opt.form {
                Form::Checkbox => {
                    if !matches!(opt.default, Some(DefaultValue::Bool(_))) {
                        errors.push(format!("option '{}': checkbox needs a boolean default", opt.key));
                    }
                }
                Form::Radio | Form::Listbox => {
                    if opt.choices.len() < 2 {
                        errors.push(format!("option '{}': needs at least two choices", opt.key));
                    }
                    match &opt.default {
                        Some(DefaultValue::Text(v))
                            if opt.choices.iter().any(|c| &c.value == v) => {}
                        _ => errors.push(format!(
                            "option '{}': default must be one of its choices",
                            opt.key
                        )),
                    }
                }
                Form::Text | Form::Button => {
                    if opt.default.is_some() {
                        errors.push(format!("option '{}': default must be null", opt.key));
                    }
                }
            }
            if let Some(b) = &opt.maps_to {
                if !BEHAVIOURS.contains(&b.as_str()) {
                    errors.push(format!("option '{}': unknown behaviour '{b}'", opt.key));
                }
            }
        }
        for opt in &self.options {
            if let Some(req) = &opt.requires {
                if !option_keys.contains(&req.as_str()) {
                    errors.push(format!("option '{}': requires unknown key '{req}'", opt.key));
                }
            }
        }

        // Controls.
        let mut control_ids: Vec<&str> = Vec::new();
        for (ci, c) in self.controls.iter().enumerate() {
            if control_ids.contains(&c.id.as_str()) {
                errors.push(format!("duplicate control id '{}'", c.id));
            }
            control_ids.push(&c.id);
            if !ACTIONS.contains(&c.action.as_str()) {
                errors.push(format!("control '{}': unknown action '{}'", c.id, c.action));
            }
            if c.action == "navigate" {
                let ok = match &c.argument {
                    Some(Argument::Text(t)) => NAVIGATE_ARGS.contains(&t.as_str()),
                    _ => false,
                };
                if !ok {
                    errors.push(format!("control '{}': navigate needs a direction argument", c.id));
                }
            }
            if let Some(v) = &c.value {
                match self.numeric.get(v) {
                    None => errors.push(format!("control '{}': unknown encoding '{v}'", c.id)),
                    Some(enc) => {
                        if !matches!(enc.kind.as_str(), "absolute" | "relative" | "text" | "constant") {
                            errors.push(format!("control '{}': unknown numeric kind '{}'", c.id, enc.kind));
                        }
                        if let Some(name) = &enc.encoding {
                            if !NUMERIC_ENCODINGS.contains(&name.as_str()) {
                                errors.push(format!("control '{}': unknown numeric encoding '{name}'", c.id));
                            }
                        }
                    }
                }
            }
            if let Some(ch) = c.channel {
                if !(1..=16).contains(&ch) {
                    errors.push(format!("control '{}': channel {ch} outside 1-16", c.id));
                }
            }
            if let Some((a, b)) = c.channel_range {
                if !(1..=16).contains(&a) || !(1..=16).contains(&b) || a > b {
                    errors.push(format!("control '{}': bad channel_range", c.id));
                }
            }
            match c.kind {
                Kind::Note if c.note.is_none() => {
                    errors.push(format!("control '{}': note kind needs a note", c.id));
                }
                Kind::Cc | Kind::CcPair if c.message.is_none() => {
                    errors.push(format!("control '{}': needs a message number", c.id));
                }
                Kind::Sysex => {
                    match parse_pattern(c.pattern.as_deref().unwrap_or("")) {
                        Ok(p) => self.sysex_patterns.push((ci, p)),
                        Err(e) => errors.push(format!("control '{}': pattern: {e}", c.id)),
                    }
                }
                _ => {}
            }
            if let Some(e) = &c.expand {
                if e.axis == Axis::Index {
                    errors.push(format!(
                        "control '{}': the index axis is for feedback entries",
                        c.id
                    ));
                }
                if e.count == 0 || e.count > 128 {
                    errors.push(format!("control '{}': expand count out of range", c.id));
                }
                let base_ok = match e.axis {
                    Axis::Note => c.note.is_some(),
                    Axis::Cc => c.message.is_some(),
                    Axis::Channel => c.channel.is_some(),
                    Axis::Index => true,
                };
                if !base_ok {
                    errors.push(format!(
                        "control '{}': expand axis {:?} needs its base address",
                        c.id, e.axis
                    ));
                }
            }
            if let Some(Argument::Expr { expr }) = &c.argument {
                match Expr::parse(expr) {
                    Ok(parsed) => {
                        self.exprs.insert(ci, parsed);
                    }
                    Err(e) => errors.push(format!("control '{}': argument expr: {e}", c.id)),
                }
            }
            if let Some(Argument::OptionRef { option }) = &c.argument {
                if !option_keys.contains(&option.as_str()) {
                    errors.push(format!("control '{}': unknown option '{option}'", c.id));
                }
            }
            if let Some(mo) = &c.mode_option {
                if !option_keys.contains(&mo.key.as_str()) {
                    errors.push(format!("control '{}': mode_option references unknown key", c.id));
                }
            }
        }

        // Expansion and the decode table.
        for (ci, c) in self.controls.iter().enumerate() {
            if c.kind == Kind::Sysex {
                continue;
            }
            let channels: Vec<Option<u8>> = match (c.channel, c.channel_range) {
                (Some(ch), _) => vec![Some(ch)],
                (None, Some((a, b))) => (a..=b).map(Some).collect(),
                (None, None) => vec![None],
            };
            let count = c.expand.as_ref().map(|e| e.count).unwrap_or(1);
            let axis = c.expand.as_ref().map(|e| e.axis);
            for index in 0..count {
                for ch in &channels {
                    let (note, cc, channel) = match c.kind {
                        Kind::Note => {
                            let n = c.note.unwrap_or(0);
                            let n = if axis == Some(Axis::Note) { n + index as u8 } else { n };
                            let ch = if axis == Some(Axis::Channel) {
                                ch.map(|v| v + index as u8)
                            } else {
                                *ch
                            };
                            (Some(n), None, ch)
                        }
                        Kind::Cc | Kind::CcPair => {
                            let m = c.message.unwrap_or(0);
                            let m = if axis == Some(Axis::Cc) { m + index as u8 } else { m };
                            let ch = if axis == Some(Axis::Channel) {
                                ch.map(|v| v + index as u8)
                            } else {
                                *ch
                            };
                            (None, Some(m), ch)
                        }
                        Kind::PitchBend | Kind::ChannelPressure | Kind::ProgramChange => {
                            let ch = if axis == Some(Axis::Channel) {
                                ch.map(|v| v + index as u8)
                            } else {
                                *ch
                            };
                            (None, None, ch)
                        }
                        Kind::Sysex => unreachable!(),
                    };
                    if let Some(n) = note {
                        if n > 127 {
                            errors.push(format!("control '{}': expanded note above 127", c.id));
                            continue;
                        }
                    }
                    if let Some(m) = cc {
                        if m > 127 || (c.kind == Kind::CcPair && m + 32 > 127) {
                            errors.push(format!("control '{}': expanded CC above 127", c.id));
                            continue;
                        }
                    }
                    if let Some(ch) = channel {
                        if ch > 16 {
                            errors.push(format!("control '{}': expanded channel above 16", c.id));
                            continue;
                        }
                    }
                    let ei = self.expanded.len();
                    self.expanded.push(Expanded { control: ci, index, channel, note, cc });
                    let class = match c.kind {
                        Kind::Note => Class::Note,
                        Kind::Cc | Kind::CcPair => Class::Cc,
                        Kind::PitchBend => Class::PitchBend,
                        Kind::ChannelPressure => Class::ChannelPressure,
                        Kind::ProgramChange => Class::ProgramChange,
                        Kind::Sysex => unreachable!(),
                    };
                    let addr = note.or(cc).unwrap_or(0);
                    let keys: Vec<DecodeKey> = if c.kind == Kind::CcPair {
                        vec![(class, channel, addr), (class, channel, addr + 32)]
                    } else {
                        vec![(class, channel, addr)]
                    };
                    for key in keys {
                        // An any-channel control overlaps every channel.
                        let conflict = self.decode.contains_key(&key)
                            || (key.1.is_some()
                                && self.decode.contains_key(&(key.0, None, key.2)))
                            || (key.1.is_none()
                                && self
                                    .decode
                                    .keys()
                                    .any(|k| k.0 == key.0 && k.2 == key.2 && k.1.is_some()));
                        if conflict {
                            errors.push(format!(
                                "control '{}': address overlap at {:?} channel {:?} address {}",
                                c.id, class, key.1, key.2
                            ));
                        } else {
                            self.decode.insert(key, ei);
                        }
                    }
                }
            }
        }

        // Feedback.
        let mut fb_ids: Vec<&str> = Vec::new();
        for fb in &self.feedback {
            if fb_ids.contains(&fb.id.as_str()) {
                errors.push(format!("duplicate feedback id '{}'", fb.id));
            }
            fb_ids.push(&fb.id);
            if !STATES.contains(&fb.state.as_str()) {
                errors.push(format!("feedback '{}': unknown state '{}'", fb.id, fb.state));
            }
            if !self.numeric.contains_key(&fb.value)
                && !self.boolean_encodings.contains_key(&fb.value)
            {
                errors.push(format!("feedback '{}': unknown encoding '{}'", fb.id, fb.value));
            }
            if let Some(enc) = self.boolean_encodings.get(&fb.value) {
                for v in [&enc.on, &enc.off] {
                    if !matches!(v.kind.as_str(), "note" | "cc") {
                        errors.push(format!("feedback '{}': boolean value kind must be note or cc", fb.id));
                    }
                }
            }
            if let Some(opt) = &fb.enabled_by {
                if !option_keys.contains(&opt.as_str()) {
                    errors.push(format!("feedback '{}': enabled_by unknown key '{opt}'", fb.id));
                }
            }
            if let Some(n) = &fb.address.index_nibble {
                if n != "high" {
                    errors.push(format!("feedback '{}': index_nibble must be 'high'", fb.id));
                }
            }
            if let Some(NoteAddress::Text(t)) = &fb.address.note {
                if t != "control" {
                    errors.push(format!("feedback '{}': note address must be a number or 'control'", fb.id));
                }
            }
        }

        // Assignment.
        if let Some(a) = &self.assignment {
            for id in &a.controls {
                if !control_ids.contains(&id.as_str()) {
                    errors.push(format!("assignment: unknown control '{id}'"));
                }
            }
            for choice in &a.choices {
                if !ACTIONS.contains(&choice.as_str()) {
                    errors.push(format!("assignment: unknown action '{choice}'"));
                }
            }
            if let Some(opt) = &a.enabled_by {
                if !option_keys.contains(&opt.as_str()) {
                    errors.push(format!("assignment: enabled_by unknown key '{opt}'"));
                }
            }
        }

        if errors.is_empty() {
            Ok(self)
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn option(&self, key: &str) -> Option<&OptionEntry> {
        self.options.iter().find(|o| o.key == key)
    }

    pub fn control_by_id(&self, id: &str) -> Option<&Control> {
        self.controls.iter().find(|c| c.id == id)
    }

    /// The boolean value of the option mapped to a behaviour, with the
    /// file default when no stored value exists.
    pub fn behaviour_on(
        &self,
        values: &BTreeMap<String, crate::host_settings::ControllerValue>,
        behaviour: &str,
    ) -> bool {
        let Some(opt) = self.options.iter().find(|o| o.maps_to.as_deref() == Some(behaviour))
        else {
            return false;
        };
        match values.get(&opt.key) {
            Some(crate::host_settings::ControllerValue::Bool(b)) => *b,
            _ => matches!(opt.default, Some(DefaultValue::Bool(true))),
        }
    }

    /// Whether the full protocol is accepted. A definition without a
    /// `protocol_support` option runs its full protocol.
    pub fn protocol_on(
        &self,
        values: &BTreeMap<String, crate::host_settings::ControllerValue>,
    ) -> bool {
        if !self
            .options
            .iter()
            .any(|o| o.maps_to.as_deref() == Some("protocol_support"))
        {
            return true;
        }
        self.behaviour_on(values, "protocol_support")
    }

    /// The choice value of an option, with the file default.
    pub fn option_choice(
        &self,
        values: &BTreeMap<String, crate::host_settings::ControllerValue>,
        key: &str,
    ) -> String {
        if let Some(crate::host_settings::ControllerValue::Choice(v)) = values.get(key) {
            return v.clone();
        }
        match self.option(key).and_then(|o| o.default.as_ref()) {
            Some(DefaultValue::Text(v)) => v.clone(),
            _ => String::new(),
        }
    }

    /// The boolean value of an option, with the file default.
    pub fn option_bool(
        &self,
        values: &BTreeMap<String, crate::host_settings::ControllerValue>,
        key: &str,
    ) -> bool {
        match values.get(key) {
            Some(crate::host_settings::ControllerValue::Bool(b)) => *b,
            _ => self
                .option(key)
                .map(|o| matches!(o.default, Some(DefaultValue::Bool(true))))
                .unwrap_or(false),
        }
    }
}

/// Parse a SysEx pattern: hex bytes with `??` wildcards, `f0` … `f7`.
pub fn parse_pattern(text: &str) -> Result<Vec<Option<u8>>, String> {
    let mut out = Vec::new();
    for token in text.split_whitespace() {
        if token == "??" {
            out.push(None);
        } else {
            let b = u8::from_str_radix(token, 16)
                .map_err(|_| format!("bad hex byte '{token}'"))?;
            out.push(Some(b));
        }
    }
    if out.len() < 2 || out.first() != Some(&Some(0xf0)) || out.last() != Some(&Some(0xf7)) {
        return Err("pattern must start f0 and end f7".into());
    }
    Ok(out)
}

/// Log unknown keys at the levels the format defines; ignore them.
fn warn_unknown_keys(value: &serde_json::Value) {
    const TOP: &[&str] = &[
        "schema", "id", "name", "version", "shape", "match", "channels", "port",
        "options", "assignment", "controls", "feedback", "numeric",
        "boolean_encodings", "extra",
    ];
    const CONTROL: &[&str] = &[
        "id", "label", "kind", "note", "message", "channel", "channel_range",
        "pattern", "value_byte", "expand", "value", "press", "mode",
        "mode_option", "simple", "action", "argument",
    ];
    const OPTION: &[&str] = &[
        "key", "label", "help", "form", "group", "break", "choices", "default",
        "requires", "maps_to",
    ];
    const FEEDBACK: &[&str] = &[
        "id", "enabled_by", "kind", "address", "state", "value", "sync",
        "rate_hz", "period_ms",
    ];
    fn check(v: &serde_json::Value, allowed: &[&str], path: &str) {
        if let Some(obj) = v.as_object() {
            for key in obj.keys() {
                if !allowed.contains(&key.as_str()) {
                    log::warn!("controller map: unknown key '{key}' in {path}, ignored");
                }
            }
        }
    }
    check(value, TOP, "the top level");
    if let Some(controls) = value.get("controls").and_then(|v| v.as_array()) {
        for (i, c) in controls.iter().enumerate() {
            check(c, CONTROL, &format!("control {i}"));
        }
    }
    if let Some(options) = value.get("options").and_then(|v| v.as_array()) {
        for (i, o) in options.iter().enumerate() {
            check(o, OPTION, &format!("option {i}"));
        }
    }
    if let Some(feedback) = value.get("feedback").and_then(|v| v.as_array()) {
        for (i, f) in feedback.iter().enumerate() {
            check(f, FEEDBACK, &format!("feedback {i}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mackie() -> ControllerMap {
        ControllerMap::parse(crate::controllers::load::EMBEDDED_MACKIE)
            .unwrap()
            .validate()
            .unwrap()
    }

    fn arc() -> ControllerMap {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/work/ARC-USB/MAPS/RME_ARC_USB_MIDI.json"
        ))
        .unwrap();
        ControllerMap::parse(&text).unwrap().validate().unwrap()
    }

    #[test]
    fn the_mackie_map_parses_and_validates() {
        let map = mackie();
        assert_eq!(map.id, "mackie_control");
        assert_eq!(map.shape, Shape::Surface);
        assert_eq!(map.channels, 8);
        // 8 strip faders + master, 8 VPOTs, the strip buttons, the simple
        // CC block: every expanded address is unique.
        assert!(map.expanded.len() > 200);
    }

    #[test]
    fn the_mackie_defaults_reproduce_the_current_settings() {
        let map = mackie();
        let values = BTreeMap::new();
        assert!(map.behaviour_on(&values, "protocol_support"));
        assert!(map.behaviour_on(&values, "full_lcd"));
        assert!(map.behaviour_on(&values, "send_user_names"));
        assert!(map.behaviour_on(&values, "submix_linked"));
        assert!(!map.behaviour_on(&values, "send_level_messages"));
        assert!(!map.behaviour_on(&values, "peak_hold"));
        assert!(!map.behaviour_on(&values, "stereo_to_mono"));
        assert!(!map.behaviour_on(&values, "background_disable"));
    }

    #[test]
    fn the_arc_map_parses_and_validates() {
        let map = arc();
        assert_eq!(map.id, "rme_arc_usb");
        assert_eq!(map.shape, Shape::Remote);
        assert_eq!(map.controls.len(), 17);
        assert!(map.assignment.is_some());
        // The keys match any channel: the unit's parser rule.
        assert!(map.controls.iter().all(|c| c.channel.is_none()));
    }

    #[test]
    fn duplicate_addresses_are_rejected() {
        let text = r#"{
            "schema": 1, "id": "t", "name": "T", "version": "1",
            "shape": "keys", "options": [],
            "controls": [
                { "id": "a", "kind": "note", "note": 5, "action": "dim" },
                { "id": "b", "kind": "note", "note": 5, "action": "mono" }
            ]
        }"#;
        let err = ControllerMap::parse(text).unwrap().validate().unwrap_err();
        assert!(err.contains("overlap"), "{err}");
    }

    #[test]
    fn an_any_channel_control_overlaps_a_channel_one_control() {
        let text = r#"{
            "schema": 1, "id": "t", "name": "T", "version": "1",
            "shape": "keys", "options": [],
            "controls": [
                { "id": "a", "kind": "note", "note": 5, "action": "dim" },
                { "id": "b", "kind": "note", "note": 5, "channel": 1, "action": "mono" }
            ]
        }"#;
        let err = ControllerMap::parse(text).unwrap().validate().unwrap_err();
        assert!(err.contains("overlap"), "{err}");
    }

    #[test]
    fn unknown_actions_behaviours_states_and_encodings_are_rejected() {
        let text = r#"{
            "schema": 1, "id": "t", "name": "T", "version": "1",
            "shape": "keys",
            "options": [
                { "key": "x", "label": "X", "form": "checkbox", "default": true, "maps_to": "nope" }
            ],
            "controls": [
                { "id": "a", "kind": "note", "note": 5, "action": "nope", "value": "nope" }
            ],
            "feedback": [
                { "id": "f", "kind": "note", "address": { "note": 5 }, "state": "nope", "value": "nope" }
            ]
        }"#;
        let err = ControllerMap::parse(text).unwrap().validate().unwrap_err();
        assert!(err.contains("unknown action"), "{err}");
        assert!(err.contains("unknown behaviour"), "{err}");
        assert!(err.contains("unknown state"), "{err}");
        assert!(err.contains("unknown encoding"), "{err}");
    }

    #[test]
    fn a_newer_schema_is_rejected() {
        let text = r#"{
            "schema": 2, "id": "t", "name": "T", "version": "1",
            "shape": "keys", "options": [], "controls": []
        }"#;
        let err = ControllerMap::parse(text).unwrap().validate().unwrap_err();
        assert!(err.contains("schema"), "{err}");
    }

    #[test]
    fn a_bad_listbox_default_is_rejected() {
        let text = r#"{
            "schema": 1, "id": "t", "name": "T", "version": "1",
            "shape": "keys",
            "options": [
                { "key": "x", "label": "X", "form": "listbox", "default": "c",
                  "choices": [ { "value": "a", "label": "A" }, { "value": "b", "label": "B" } ] }
            ],
            "controls": []
        }"#;
        let err = ControllerMap::parse(text).unwrap().validate().unwrap_err();
        assert!(err.contains("default"), "{err}");
    }

    #[test]
    fn sysex_patterns_parse_with_wildcards() {
        let p = parse_pattern("f0 00 00 66 14 ?? ?? f7").unwrap();
        assert_eq!(p.len(), 8);
        assert_eq!(p[5], None);
        assert!(parse_pattern("00 01 f7").is_err());
        assert!(parse_pattern("f0 00").is_err());
    }

    #[test]
    fn the_row_fader_expansion_covers_192_addresses() {
        let map = mackie();
        let count = map
            .expanded
            .iter()
            .filter(|e| map.controls[e.control].id == "row_faders")
            .count();
        assert_eq!(count, 192);
    }
}