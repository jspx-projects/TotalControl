//! The controller engine: decode, dispatch and feedback, one path for every
//! definition file. See `TOTALCONTROL-CONTROLLER-SPEC.md` §8.
//!
//! The engine is map-driven. The Mackie codec primitives in
//! [`crate::mackie`] supply the byte parser, the fader curve, the handshake
//! and the feedback encoders; the definition file supplies every address,
//! action and option.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use totalcontrol_core::ChannelId;

use crate::app::Message;
use crate::host_settings::ControllerValue;
use crate::mackie::{
    self, fader_to_gain, MackieAction, MackieNav, MidiEvent, StripFeedback, Window, STRIPS,
};
use crate::osc_rme::{fader_db, Bus, RmeToggle};

use super::expr::Vars;
use super::map::{Argument, Class, ControllerMap, Expanded, Kind, Mode};

/// One decoded control event. `expanded` indexes [`ControllerMap::expanded`].
#[derive(Debug, Clone, PartialEq)]
pub enum ControlEvent {
    Press { expanded: usize },
    Release { expanded: usize },
    Value { expanded: usize, raw: u16, decoded: Decoded },
}

/// A decoded value: a normalized position, relative steps, or the raw byte.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decoded {
    Position(f32),
    Steps(i32),
    Raw(u32),
}

/// Per-instance engine state. The navigation and parser live in `nav` for
/// the surface shape; a remote uses the encoder fields.
#[derive(Debug, Clone)]
pub struct InstanceState {
    pub nav: MackieNav,
    pub encoder_target: String,
    pub encoder_remainder: f32,
    pub cc_pair_msb: HashMap<(u8, u8), u8>,
    pub pressed: HashSet<usize>,
    pub last_recall: Option<Instant>,
    pub feedback: FeedbackCache,
}

impl Default for InstanceState {
    fn default() -> Self {
        Self {
            nav: MackieNav::default(),
            encoder_target: "main".into(),
            encoder_remainder: 0.0,
            cc_pair_msb: HashMap::new(),
            pressed: HashSet::new(),
            last_recall: None,
            feedback: FeedbackCache::default(),
        }
    }
}

impl InstanceState {
    /// Reset the connection state on close or reconnect.
    pub fn reset_connection(&mut self) {
        self.nav.online = false;
        self.nav.parser = Default::default();
        self.pressed.clear();
        self.cc_pair_msb.clear();
        self.feedback = FeedbackCache::default();
    }
}

// ---- decode ---------------------------------------------------------------

fn lookup(map: &ControllerMap, class: Class, ch: u8, addr: u8) -> Option<usize> {
    map.decode
        .get(&(class, Some(ch), addr))
        .or_else(|| map.decode.get(&(class, None, addr)))
        .copied()
}

/// Decode one parsed MIDI event into control events. SysEx and realtime are
/// handled by [`sysex`] and the caller.
pub fn decode(map: &ControllerMap, state: &mut InstanceState, ev: &MidiEvent) -> Vec<ControlEvent> {
    let mut out = Vec::new();
    let value_event = |ei: usize, raw: u16, out: &mut Vec<ControlEvent>| {
        let exp = &map.expanded[ei];
        let control = &map.controls[exp.control];
        let decoded = decode_value(map, control.value.as_deref(), raw);
        out.push(ControlEvent::Value { expanded: ei, raw, decoded });
    };
    match ev {
        MidiEvent::NoteOn { ch, note, vel } => {
            let Some(ei) = lookup(map, Class::Note, *ch + 1, *note) else {
                return out;
            };
            let control = &map.controls[map.expanded[ei].control];
            if control.is_value() {
                value_event(ei, u16::from(*vel), &mut out);
            } else if *vel >= control.press_threshold() {
                out.push(ControlEvent::Press { expanded: ei });
            } else {
                out.push(ControlEvent::Release { expanded: ei });
            }
        }
        MidiEvent::NoteOff { ch, note } => {
            if let Some(ei) = lookup(map, Class::Note, *ch + 1, *note) {
                if !map.controls[map.expanded[ei].control].is_value() {
                    out.push(ControlEvent::Release { expanded: ei });
                }
            }
        }
        MidiEvent::Cc { ch, cc, val } => {
            let Some(ei) = lookup(map, Class::Cc, *ch + 1, *cc) else {
                return out;
            };
            let exp = &map.expanded[ei];
            let control = &map.controls[exp.control];
            if control.kind == Kind::CcPair {
                let base = exp.cc.unwrap_or(0);
                if *cc == base {
                    // MSB: hold it until the LSB arrives.
                    state.cc_pair_msb.insert((*ch, base), *val);
                } else {
                    let msb = state.cc_pair_msb.get(&(*ch, base)).copied().unwrap_or(0);
                    let raw = (u16::from(msb) << 7) | u16::from(*val & 0x7f);
                    value_event(ei, raw, &mut out);
                }
            } else if control.is_value() {
                value_event(ei, u16::from(*val), &mut out);
            } else if *val >= control.press_threshold() {
                out.push(ControlEvent::Press { expanded: ei });
            } else {
                out.push(ControlEvent::Release { expanded: ei });
            }
        }
        MidiEvent::PitchBend { ch, value } => {
            if let Some(ei) = lookup(map, Class::PitchBend, *ch + 1, 0) {
                value_event(ei, *value, &mut out);
            }
        }
        MidiEvent::ChannelPressure { ch, val } => {
            if let Some(ei) = lookup(map, Class::ChannelPressure, *ch + 1, 0) {
                value_event(ei, u16::from(*val), &mut out);
            }
        }
        MidiEvent::SysEx(data) => {
            for (ci, pattern) in &map.sysex_patterns {
                if data.len() != pattern.len() {
                    continue;
                }
                if pattern
                    .iter()
                    .zip(data.iter())
                    .all(|(p, b)| p.map(|v| v == *b).unwrap_or(true))
                {
                    let control = &map.controls[*ci];
                    let raw = control
                        .value_byte
                        .and_then(|i| data.get(i))
                        .copied()
                        .unwrap_or(0);
                    // A pattern control has one expanded instance.
                    if let Some(ei) = map.expanded.iter().position(|e| e.control == *ci) {
                        value_event(ei, u16::from(raw), &mut out);
                    }
                    break;
                }
            }
        }
        MidiEvent::Realtime(_) => {}
    }
    out
}

fn decode_value(map: &ControllerMap, encoding: Option<&str>, raw: u16) -> Decoded {
    let Some(enc) = encoding.and_then(|name| map.numeric.get(name)) else {
        return Decoded::Raw(u32::from(raw));
    };
    match enc.encoding.as_deref() {
        Some("absolute_7bit") => Decoded::Position(f32::from(raw) / 127.0),
        Some("absolute_14bit") => {
            let v = if enc.order.as_deref() == Some("high_first") {
                ((raw & 0x7f) << 7) | (raw >> 7)
            } else {
                raw
            };
            Decoded::Position(f32::from(v) / 16383.0)
        }
        Some("relative_7bit") => {
            let v = raw as u8;
            let steps = if v >= 65 {
                -i32::from(v - 64)
            } else {
                i32::from(v.min(63))
            };
            Decoded::Steps(steps)
        }
        Some("relative_signed_bit") => {
            let v = raw as u8;
            let mut mag = i32::from(v & 0x3f);
            if let Some(max) = enc.max_step {
                mag = mag.min(i32::from(max));
            }
            Decoded::Steps(if v & 0x40 != 0 { -mag } else { mag })
        }
        _ => Decoded::Raw(u32::from(raw)),
    }
}

// ---- dispatch -------------------------------------------------------------

/// Everything dispatch needs from the confirmed model, built by the app.
pub struct DispatchCtx<'a> {
    pub window: Window,
    pub out_window: Window,
    pub counts: [usize; 3],
    pub strip_counts: [usize; 3],
    pub submix_count: usize,
    pub values: &'a BTreeMap<String, ControllerValue>,
    pub assignments: &'a BTreeMap<String, String>,
    pub external_input: bool,
    pub selected_pair: Option<usize>,
}

/// The resolved argument of one event.
#[derive(Debug, Clone)]
enum Arg {
    Int(i64),
    Text(String),
    None,
}

fn resolve_arg(
    map: &ControllerMap,
    control: &super::map::Control,
    exp: &Expanded,
    raw: u16,
    values: &BTreeMap<String, ControllerValue>,
) -> Arg {
    match &control.argument {
        Some(Argument::Int(v)) => Arg::Int(*v),
        Some(Argument::Text(t)) => Arg::Text(t.clone()),
        Some(Argument::OptionRef { option }) => {
            Arg::Text(map.option_choice(values, option))
        }
        Some(Argument::Expr { .. }) => {
            let vars = Vars {
                channel: i64::from(exp.channel.unwrap_or(1)),
                note: i64::from(exp.note.unwrap_or(0)),
                cc: i64::from(exp.cc.unwrap_or(0)),
                value: i64::from(raw),
                strip: exp.index as i64,
            };
            match map.exprs.get(&exp.control) {
                Some(expr) => Arg::Int(expr.eval(&vars)),
                None => Arg::None,
            }
        }
        None => Arg::Int(exp.index as i64),
    }
}

fn arg_int(arg: &Arg) -> i64 {
    match arg {
        Arg::Int(v) => *v,
        _ => 0,
    }
}

fn strip_out(nav: &MackieNav, cid: ChannelId) -> usize {
    match cid {
        ChannelId::Output(i) => i,
        _ => nav.submix,
    }
}

fn fader_message(nav: &MackieNav, cid: ChannelId, p: f32) -> Message {
    if nav.trim && !matches!(cid, ChannelId::Output(_)) {
        Message::TrimChanged(cid, fader_db(p))
    } else {
        Message::VolumeChanged(cid, strip_out(nav, cid), fader_to_gain(p))
    }
}

fn row_len(counts: [usize; 3], bus: Bus) -> usize {
    match bus {
        Bus::Input => counts[0],
        Bus::Playback => counts[1],
        Bus::Output => counts[2],
    }
}

fn shift_bank(nav: &mut MackieNav, strip_counts: [usize; 3], delta: isize) {
    let last = row_len(strip_counts, nav.bus).saturating_sub(1);
    nav.bank = (nav.bank as isize + delta).clamp(0, last as isize) as usize;
}

fn set_bus(nav: &mut MackieNav, strip_counts: [usize; 3], bus: Bus) {
    nav.bus = bus;
    let last = row_len(strip_counts, bus).saturating_sub(1);
    nav.bank = nav.bank.min(last);
}

/// Whether the control is momentary for the current option values.
fn effective_mode(
    map: &ControllerMap,
    control: &super::map::Control,
    values: &BTreeMap<String, ControllerValue>,
) -> Mode {
    if let Some(mo) = &control.mode_option {
        if map.option_choice(values, &mo.key) == mo.value {
            return Mode::Momentary;
        }
    }
    control.mode
}

/// Dispatch one control event into ordinary mixer messages. Navigation
/// changes mutate `state.nav`; the caller detects them and refreshes the
/// feedback.
pub fn dispatch(
    map: &ControllerMap,
    state: &mut InstanceState,
    ctx: &DispatchCtx<'_>,
    ev: &ControlEvent,
) -> Vec<Message> {
    let (ei, raw) = match ev {
        ControlEvent::Press { expanded } | ControlEvent::Release { expanded } => (*expanded, 0),
        ControlEvent::Value { expanded, raw, .. } => (*expanded, *raw),
    };
    let exp = &map.expanded[ei];
    let control = &map.controls[exp.control];

    // With protocol support off only the simple controls work.
    if !map.protocol_on(ctx.values) && !control.simple {
        return Vec::new();
    }

    // The assignment table overrides the shipped action.
    let assigned = map
        .assignment
        .as_ref()
        .filter(|a| a.controls.contains(&control.id))
        .and_then(|_| ctx.assignments.get(&control.id));
    let action = assigned.map(String::as_str).unwrap_or(&control.action);
    let arg = if assigned.is_some() {
        Arg::Int(exp.index as i64)
    } else {
        resolve_arg(map, control, exp, raw, ctx.values)
    };

    let mode = effective_mode(map, control, ctx.values);
    let nav = &mut state.nav;

    match ev {
        ControlEvent::Release { .. } => {
            state.pressed.remove(&ei);
            // SELECT hold state, for the SEL+DYN combination.
            if control.action == "strip_select" && exp.index < STRIPS {
                nav.sel_held[exp.index] = false;
            }
            if mode == Mode::Momentary {
                return momentary_set(action, &arg, false);
            }
            return Vec::new();
        }
        ControlEvent::Press { .. } => {
            state.pressed.insert(ei);
            if control.action == "strip_select" && exp.index < STRIPS {
                nav.sel_held[exp.index] = true;
            }
            if mode == Mode::Momentary {
                return momentary_set(action, &arg, true);
            }
        }
        ControlEvent::Value { .. } => {}
    }

    let decoded = match ev {
        ControlEvent::Value { decoded, .. } => *decoded,
        _ => Decoded::Raw(0),
    };
    let position = match decoded {
        Decoded::Position(p) => p,
        _ => 0.0,
    };
    let index = exp.index;
    let strip = |i: usize| -> Option<ChannelId> { ctx.window.get(i).copied().flatten() };

    match action {
        "none" => Vec::new(),
        "strip_volume" => strip(index)
            .map(|cid| vec![fader_message(nav, cid, position)])
            .unwrap_or_default(),
        "master_volume" => vec![Message::RmeMasterVolume(fader_to_gain(position))],
        "strip_pan" => {
            let Decoded::Steps(steps) = decoded else {
                return Vec::new();
            };
            if steps == 0 {
                return Vec::new();
            }
            let Some(cid) = strip(index) else {
                return Vec::new();
            };
            vec![Message::MackiePanNudge(cid, strip_out(nav, cid), (steps * 2) as i16)]
        }
        "strip_pan_center" => strip(index)
            .map(|cid| vec![Message::PanChanged(cid, strip_out(nav, cid), 0)])
            .unwrap_or_default(),
        "strip_mute" => strip(index)
            .map(|cid| vec![Message::RmeToggle(RmeToggle::Mute(cid))])
            .unwrap_or_default(),
        "strip_solo" => strip(index)
            .map(|cid| vec![Message::RmeToggle(RmeToggle::Solo(cid))])
            .unwrap_or_default(),
        "strip_select" => strip(index)
            .map(|cid| vec![Message::RmeSelect(cid, true)])
            .unwrap_or_default(),
        "submix_select" => {
            let any_row = map.behaviour_on(ctx.values, "submix_any_row");
            if !any_row && nav.bus != Bus::Output {
                return Vec::new();
            }
            let Some(ChannelId::Output(i)) = ctx.out_window.get(index).copied().flatten() else {
                return Vec::new();
            };
            let pair = i / 2;
            if pair < ctx.submix_count.max(1) {
                nav.submix = pair;
                vec![Message::SelectOutput(pair)]
            } else {
                Vec::new()
            }
        }
        "submix_select_index" => {
            let idx = arg_int(&arg) as usize;
            if idx < ctx.submix_count.max(1) {
                nav.submix = idx;
                vec![Message::SelectOutput(idx)]
            } else {
                Vec::new()
            }
        }
        "navigate" => {
            let Arg::Text(dir) = &arg else {
                return Vec::new();
            };
            match dir.as_str() {
                "bank_left" => shift_bank(nav, ctx.strip_counts, -(STRIPS as isize)),
                "bank_right" => shift_bank(nav, ctx.strip_counts, STRIPS as isize),
                "channel_left" => shift_bank(nav, ctx.strip_counts, -1),
                "channel_right" => shift_bank(nav, ctx.strip_counts, 1),
                "row_up" => set_bus(
                    nav,
                    ctx.strip_counts,
                    match nav.bus {
                        Bus::Input => Bus::Output,
                        Bus::Playback => Bus::Input,
                        Bus::Output => Bus::Playback,
                    },
                ),
                "row_down" => set_bus(
                    nav,
                    ctx.strip_counts,
                    match nav.bus {
                        Bus::Input => Bus::Playback,
                        Bus::Playback => Bus::Output,
                        Bus::Output => Bus::Input,
                    },
                ),
                _ => {}
            }
            Vec::new()
        }
        "trim_mode" => {
            nav.trim = !nav.trim;
            Vec::new()
        }
        "trim_on" => {
            nav.trim = true;
            Vec::new()
        }
        "trim_off" => {
            nav.trim = false;
            Vec::new()
        }
        "snapshot" => vec![Message::SnapshotClicked(arg_int(&arg) as u8)],
        "dim" => vec![Message::RmeToggle(RmeToggle::MainDim)],
        "mono" => vec![Message::RmeToggle(RmeToggle::MainMono)],
        "talkback" => vec![Message::RmeToggle(RmeToggle::MainTalkback)],
        "speaker_b" => vec![Message::RmeToggle(RmeToggle::MainSpeakerB)],
        "external_input" => vec![Message::ExternalInput(!ctx.external_input)],
        "recall_volume" => {
            state.last_recall = Some(Instant::now());
            vec![Message::ControlRoomRecall]
        }
        "mute_main" | "master_mute" => vec![Message::MackieAction(MackieAction::MasterMute)],
        "master_solo" => vec![Message::MackieAction(MackieAction::MasterSolo)],
        "global_mute" => vec![Message::RmeToggle(RmeToggle::GlobalMute)],
        "global_solo" => vec![Message::RmeToggle(RmeToggle::GlobalSolo)],
        "cue_main" => vec![Message::MackieAction(MackieAction::CueMainOut)],
        "select_cue_main" => vec![
            Message::MackieAction(MackieAction::SelectMainOut),
            Message::MackieAction(MackieAction::CueMainOut),
        ],
        "cue_phones" => {
            let slot = (arg_int(&arg) - 1).max(0) as usize;
            vec![Message::MackieAction(MackieAction::CuePhones(slot))]
        }
        "input_gain" => vec![Message::Gain(arg_int(&arg) as usize, u32::from(raw))],
        "row_fader" => {
            let idx = arg_int(&arg) as usize;
            let row = idx / 64;
            let ch = idx % 64;
            let bus = match row {
                0 => Bus::Input,
                1 => Bus::Playback,
                _ => Bus::Output,
            };
            if row > 2 || ch >= row_len(ctx.counts, bus) {
                return Vec::new();
            }
            let cid = match bus {
                Bus::Input => ChannelId::Input(ch),
                Bus::Playback => ChannelId::Playback(ch),
                Bus::Output => ChannelId::Output(ch),
            };
            if nav.trim && row != 2 {
                vec![Message::TrimChanged(cid, fader_db(position))]
            } else {
                vec![Message::VolumeChanged(cid, strip_out(nav, cid), fader_to_gain(position))]
            }
        }
        "encoder_target" => {
            if let Arg::Text(t) = &arg {
                if super::map::ENCODER_TARGETS.contains(&t.as_str()) {
                    state.encoder_target = t.clone();
                }
            }
            Vec::new()
        }
        "encoder_target_volume" => {
            let Decoded::Steps(steps) = decoded else {
                return Vec::new();
            };
            if steps == 0 {
                return Vec::new();
            }
            let step_db = match map.option_choice(ctx.values, "encoder_step").as_str() {
                "fine" => 0.1,
                _ => 0.5,
            };
            // Accumulate any sub-step remainder from earlier events; the
            // dB value itself is not quantized.
            let db = steps as f32 * step_db + state.encoder_remainder;
            state.encoder_remainder = 0.0;
            if db == 0.0 {
                return Vec::new();
            }
            let target = if map.behaviour_on(ctx.values, "encoder_follows_selection") {
                match ctx.selected_pair {
                    Some(_) => "selected".to_string(),
                    None => state.encoder_target.clone(),
                }
            } else {
                state.encoder_target.clone()
            };
            if target == "none" {
                return Vec::new();
            }
            vec![Message::EncoderNudge { target, db }]
        }
        "layout_preset" => vec![Message::LayoutClicked(arg_int(&arg) as u8)],
        // Unavailable actions: inert until the model carries them.
        _ => Vec::new(),
    }
}

/// The direct set messages of a momentary control.
fn momentary_set(action: &str, _arg: &Arg, on: bool) -> Vec<Message> {
    match action {
        "dim" => vec![Message::Dim(on)],
        "mono" => vec![Message::ControlRoomMono(on)],
        "talkback" => vec![Message::TalkbackChanged(on)],
        "speaker_b" => vec![Message::SpeakerB(on)],
        "external_input" => vec![Message::ExternalInput(on)],
        _ => Vec::new(),
    }
}

// ---- SysEx handshake --------------------------------------------------------

/// The messages to send when a port opens.
pub fn on_open(map: &ControllerMap) -> Vec<Vec<u8>> {
    if map.extra.handshake.as_deref() == Some("mackie") {
        vec![mackie::device_query(), mackie::version_request()]
    } else {
        Vec::new()
    }
}

/// The message to send when a port closes.
pub fn on_close(map: &ControllerMap) -> Option<Vec<u8>> {
    (map.extra.handshake.as_deref() == Some("mackie")).then(|| mackie::go_offline())
}

/// The outcome of one inbound SysEx message.
pub enum SysexOutcome {
    /// Not a handshake message.
    Ignored,
    /// Send these bytes back.
    Reply(Vec<u8>),
    /// The surface confirmed the connection.
    Online,
}

/// Handle the handshake SysEx of the definition. The challenge reply is
/// built here; the app owns the output queue.
pub fn sysex(map: &ControllerMap, state: &mut InstanceState, data: &[u8]) -> SysexOutcome {
    if map.extra.handshake.as_deref() != Some("mackie") {
        return SysexOutcome::Ignored;
    }
    match mackie::parse_sysex_command(data) {
        Some(mackie::SysexCommand::ConnectionQuery { serial, challenge }) => {
            state.nav.serial = serial;
            state.nav.online = false;
            SysexOutcome::Reply(mackie::host_connection_reply(&serial, &challenge))
        }
        Some(mackie::SysexCommand::DeviceQuery) => {
            SysexOutcome::Reply(mackie::device_query())
        }
        Some(mackie::SysexCommand::ConnectionConfirm) => {
            state.nav.online = true;
            SysexOutcome::Online
        }
        Some(mackie::SysexCommand::ConnectionError) => {
            log::warn!("controller: surface reported a connection error");
            SysexOutcome::Ignored
        }
        Some(mackie::SysexCommand::VersionReply) | None => SysexOutcome::Ignored,
    }
}

/// Whether the event is the loopback-detection note of this definition.
pub fn is_loopback(map: &ControllerMap, ev: &MidiEvent) -> bool {
    let MidiEvent::NoteOn { ch, note, vel } = ev else {
        return false;
    };
    map.feedback.iter().any(|fb| {
        fb.state == "loopback"
            && matches!(&fb.address.note, Some(super::map::NoteAddress::Num(n)) if *n == *note)
            && fb.address.channel.map(|c| c - 1 == *ch).unwrap_or(false)
            && map
                .numeric
                .get(&fb.value)
                .and_then(|n| n.value)
                .map(|v| v == *vel)
                .unwrap_or(false)
    })
}

/// The loopback note bytes and period, when the definition carries one.
pub fn loopback(map: &ControllerMap) -> Option<(Vec<u8>, u64)> {
    let fb = map.feedback.iter().find(|fb| fb.state == "loopback")?;
    let note = match &fb.address.note {
        Some(super::map::NoteAddress::Num(n)) => *n,
        _ => return None,
    };
    let ch = fb.address.channel.unwrap_or(1) - 1;
    let vel = map.numeric.get(&fb.value)?.value?;
    Some((mackie::note_on(ch, note, vel).to_vec(), fb.period_ms.unwrap_or(500)))
}

// ---- feedback ---------------------------------------------------------------

/// The Control Room flags the feedback states read.
#[derive(Debug, Clone, Copy, Default)]
pub struct MainFlags {
    pub dim: bool,
    pub talkback: bool,
    pub mono: bool,
    pub speaker_b: bool,
    pub mute: bool,
    pub solo: bool,
    pub rude_solo: bool,
}

/// Everything the feedback encoder needs for one refresh, built by the app.
pub struct FeedbackCtx<'a> {
    pub strips: [StripFeedback; STRIPS],
    pub master_fader: f32,
    pub main: MainFlags,
    pub trim: bool,
    pub external_input: bool,
    pub levels: [f32; STRIPS],
    pub over: [bool; STRIPS],
    pub row_name: &'static str,
    pub first_channel: usize,
    pub snapshot_current: Option<u8>,
    pub values: &'a BTreeMap<String, ControllerValue>,
}

/// Last-sent feedback values, so a refresh only emits the deltas.
#[derive(Debug, Clone)]
pub struct FeedbackCache {
    values: HashMap<(usize, usize), u16>,
    lcd: [u8; 112],
    lcd_init: bool,
}

impl Default for FeedbackCache {
    fn default() -> Self {
        Self { values: HashMap::new(), lcd: [0; 112], lcd_init: false }
    }
}

const LCD_ROW: usize = 56;
const LCD_STRIP: usize = 7;

fn pad7(s: &str) -> [u8; LCD_STRIP] {
    let mut out = [b' '; LCD_STRIP];
    for (i, b) in s.bytes().take(LCD_STRIP).enumerate() {
        out[i] = if (0x20..0x7f).contains(&b) { b } else { b' ' };
    }
    out
}

/// The state of the action bound to a control, for `action_state` LEDs.
fn action_state(
    map: &ControllerMap,
    state: &InstanceState,
    ctx: &FeedbackCtx<'_>,
    control: &super::map::Control,
    exp: &Expanded,
) -> bool {
    let arg = resolve_arg(map, control, exp, 0, ctx.values);
    let index = exp.index;
    match control.action.as_str() {
        "dim" => ctx.main.dim,
        "mono" => ctx.main.mono,
        "talkback" => ctx.main.talkback,
        "speaker_b" => ctx.main.speaker_b,
        "external_input" => ctx.external_input,
        "mute_main" | "master_mute" => ctx.main.mute,
        "master_solo" => ctx.main.solo,
        "trim_mode" | "trim_on" | "trim_off" => ctx.trim,
        "snapshot" => ctx.snapshot_current == Some(arg_int(&arg) as u8),
        "encoder_target" => match &arg {
            Arg::Text(t) => &state.encoder_target == t,
            _ => false,
        },
        "strip_mute" => ctx.strips.get(index).map(|s| s.mute).unwrap_or(false),
        "strip_solo" => ctx.strips.get(index).map(|s| s.solo).unwrap_or(false),
        "strip_select" => ctx.strips.get(index).map(|s| s.select).unwrap_or(false),
        "submix_select" => ctx.strips.get(index).map(|s| s.rec).unwrap_or(false),
        _ => false,
    }
}

/// One feedback value as the wire byte(s) carry it.
enum FbValue {
    Bool(bool),
    Position(f32),
    Pan(i8),
    Meter(u8),
}

fn state_value(
    map: &ControllerMap,
    state: &InstanceState,
    ctx: &FeedbackCtx<'_>,
    fb: &super::map::Feedback,
    exp: Option<&Expanded>,
    index: usize,
) -> Option<FbValue> {
    match fb.state.as_str() {
        "action_state" => {
            let exp = exp?;
            let control = &map.controls[exp.control];
            Some(FbValue::Bool(action_state(map, state, ctx, control, exp)))
        }
        "strip.mute" => Some(FbValue::Bool(ctx.strips.get(index)?.mute)),
        "strip.solo" => Some(FbValue::Bool(ctx.strips.get(index)?.solo)),
        "strip.select" => Some(FbValue::Bool(ctx.strips.get(index)?.select)),
        "strip.rec" => Some(FbValue::Bool(ctx.strips.get(index)?.rec)),
        "strip.fader" => Some(FbValue::Position(ctx.strips.get(index)?.fader)),
        "strip.pan" => Some(FbValue::Pan(ctx.strips.get(index)?.pan)),
        "master.fader" => Some(FbValue::Position(ctx.master_fader)),
        "main.dim" => Some(FbValue::Bool(ctx.main.dim)),
        "main.talkback" => Some(FbValue::Bool(ctx.main.talkback)),
        "main.mono" => Some(FbValue::Bool(ctx.main.mono)),
        "main.speaker_b" => Some(FbValue::Bool(ctx.main.speaker_b)),
        "main.mute" => Some(FbValue::Bool(ctx.main.mute)),
        "main.solo" => Some(FbValue::Bool(ctx.main.solo)),
        "main.rude_solo" => Some(FbValue::Bool(ctx.main.rude_solo)),
        "nav.trim" => Some(FbValue::Bool(ctx.trim)),
        "meter" => {
            let db = ctx.levels.get(index).copied().unwrap_or(-100.0);
            let mut nib = mackie::meter_nibble(db);
            if ctx.over.get(index).copied().unwrap_or(false) {
                nib = 0x0e;
            }
            Some(FbValue::Meter(nib))
        }
        _ => None,
    }
}

/// Encode the changed feedback into `out`. Emits nothing when the surface
/// already shows everything. The LCD and the periodic loopback note are
/// part of the same loop; meters run at their own rate through
/// [`refresh_meters`].
pub fn refresh(
    map: &ControllerMap,
    state: &mut InstanceState,
    ctx: &FeedbackCtx<'_>,
    out: &mut Vec<u8>,
) {
    for (fi, fb) in map.feedback.iter().enumerate() {
        if fb.state == "loopback" || fb.state == "meter" {
            continue;
        }
        if let Some(opt) = &fb.enabled_by {
            if !map.option_bool(ctx.values, opt) {
                continue;
            }
        }
        if fb.state == "lcd" {
            refresh_lcd(map, fb, state, ctx, out);
            continue;
        }
        let count = fb.address.expand.as_ref().map(|e| e.count).unwrap_or(1);
        // The "control" binding: one entry per expanded note control.
        let bound: Vec<usize> = if matches!(&fb.address.note, Some(super::map::NoteAddress::Text(t)) if t == "control")
        {
            map.expanded
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    let c = &map.controls[e.control];
                    c.kind == Kind::Note && !c.is_value()
                })
                .map(|(ei, _)| ei)
                .collect()
        } else {
            (0..count).collect()
        };
        for (index, ei) in bound.iter().copied().enumerate() {
            let control_bound = matches!(&fb.address.note, Some(super::map::NoteAddress::Text(t)) if t == "control");
            let exp = if control_bound { Some(&map.expanded[ei]) } else { None };
            let Some(value) = state_value(map, state, ctx, fb, exp, index) else {
                continue;
            };
            let ch = fb.address.channel.unwrap_or(1) - 1;
            let bytes: Vec<u8> = match (&value, fb.kind) {
                (FbValue::Bool(on), Kind::Note) => {
                    let Some(enc) = map.boolean_encodings.get(&fb.value) else {
                        continue;
                    };
                    let v = if *on { &enc.on } else { &enc.off };
                    // A control-bound entry takes the control's own note;
                    // an expanded address entry counts from its base.
                    let note = match (&fb.address.note, exp) {
                        (Some(super::map::NoteAddress::Num(n)), _) => *n + index as u8,
                        (_, Some(e)) => e.note.unwrap_or(0),
                        _ => continue,
                    };
                    mackie::note_on(ch, note, v.velocity).to_vec()
                }
                (FbValue::Position(p), Kind::PitchBend) => {
                    let v = (p.clamp(0.0, 1.0) * 16383.0).round() as u16;
                    let key = (fi, index);
                    if state.feedback.values.get(&key) == Some(&v) {
                        continue;
                    }
                    state.feedback.values.insert(key, v);
                    let channel = fb.address.channel.unwrap_or(1) - 1 + index as u8;
                    mackie::pitch_bend(channel, v).to_vec()
                }
                (FbValue::Pan(pan), Kind::Cc) => {
                    // ring_position: mode 0, position 0..11 from the pan,
                    // bit 6 the centre LED.
                    let pos = (((f32::from(*pan) + 100.0) / 200.0) * 11.0)
                        .round()
                        .clamp(0.0, 11.0) as u8;
                    let mut val = pos;
                    if *pan == 0 {
                        val |= 0x40;
                    }
                    let key = (fi, index);
                    if state.feedback.values.get(&key) == Some(&u16::from(val)) {
                        continue;
                    }
                    state.feedback.values.insert(key, u16::from(val));
                    let cc = fb.address.cc.unwrap_or(48) + index as u8;
                    mackie::control_change(ch, cc, val).to_vec()
                }
                _ => continue,
            };
            // Bool LEDs diff on the velocity byte the encoding chose.
            if let FbValue::Bool(_) = &value {
                let vel = u16::from(bytes[2]);
                let key = (fi, index);
                if state.feedback.values.get(&key) == Some(&vel) {
                    continue;
                }
                state.feedback.values.insert(key, vel);
            }
            out.extend_from_slice(&bytes);
        }
    }
}

/// Meter feedback at the entry's rate: channel pressure, strip in the high
/// nibble when `index_nibble` says so.
pub fn refresh_meters(
    map: &ControllerMap,
    state: &mut InstanceState,
    ctx: &FeedbackCtx<'_>,
    out: &mut Vec<u8>,
) {
    for (fi, fb) in map.feedback.iter().enumerate() {
        if fb.state != "meter" {
            continue;
        }
        if let Some(opt) = &fb.enabled_by {
            if !map.option_bool(ctx.values, opt) {
                continue;
            }
        }
        let count = fb.address.expand.as_ref().map(|e| e.count).unwrap_or(1);
        let ch = fb.address.channel.unwrap_or(1) - 1;
        let high_nibble = fb.address.index_nibble.as_deref() == Some("high");
        for index in 0..count {
            let Some(FbValue::Meter(nib)) = state_value(map, state, ctx, fb, None, index) else {
                continue;
            };
            let key = (fi, index);
            if state.feedback.values.get(&key) == Some(&u16::from(nib)) {
                continue;
            }
            state.feedback.values.insert(key, u16::from(nib));
            let val = if high_nibble { (index as u8) << 4 | nib } else { nib };
            out.extend_from_slice(&mackie::channel_pressure(ch, val));
        }
    }
}

fn refresh_lcd(
    map: &ControllerMap,
    fb: &super::map::Feedback,
    state: &mut InstanceState,
    ctx: &FeedbackCtx<'_>,
    out: &mut Vec<u8>,
) {
    let full = map.behaviour_on(ctx.values, "full_lcd");
    let send_names = map.behaviour_on(ctx.values, "send_user_names");
    let mut lcd = [b' '; LCD_ROW * 2];
    if full {
        for (i, strip) in ctx.strips.iter().enumerate() {
            let name = if send_names { strip.name.as_str() } else { "" };
            lcd[i * LCD_STRIP..(i + 1) * LCD_STRIP].copy_from_slice(&pad7(name));
            lcd[LCD_ROW + i * LCD_STRIP..LCD_ROW + (i + 1) * LCD_STRIP]
                .copy_from_slice(&pad7(&strip.value));
        }
    } else {
        let brief = format!("{} {}", ctx.row_name, ctx.first_channel);
        for (i, b) in brief.bytes().take(LCD_ROW).enumerate() {
            lcd[i] = b;
        }
    }
    if !state.feedback.lcd_init {
        state.feedback.lcd = [u8::MAX; 112];
        state.feedback.lcd_init = true;
    }
    // The SysEx header comes from the definition.
    let header: Vec<u8> = fb
        .address
        .header
        .as_deref()
        .unwrap_or("f0 00 00 66 14 12")
        .split_whitespace()
        .filter_map(|t| u8::from_str_radix(t, 16).ok())
        .collect();
    // Only write the runs that changed, one SysEx per contiguous run.
    let mut i = 0;
    while i < lcd.len() {
        if lcd[i] != state.feedback.lcd[i] {
            let start = i;
            while i < lcd.len() && lcd[i] != state.feedback.lcd[i] && i - start < LCD_ROW {
                i += 1;
            }
            let mut msg = header.clone();
            msg.push(start.min(111) as u8);
            for &b in &lcd[start..i] {
                if msg.len() >= 118 {
                    break;
                }
                msg.push(b & 0x7f);
            }
            msg.push(0xf7);
            out.extend_from_slice(&msg);
            state.feedback.lcd[start..i].copy_from_slice(&lcd[start..i]);
        } else {
            i += 1;
        }
    }
}