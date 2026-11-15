//! Mackie Control Universal host codec.
//!
//! TotalControl is the host: a Mackie Control compatible surface connects
//! to one rawmidi port pair and drives the mixer, the same model TotalMix
//! FX implements.  This module is pure — it parses surface messages into
//! ordinary GUI messages, encodes feedback, and runs the connection
//! handshake.  The rawmidi transport lives in [`crate::midi`]; the mixer
//! mapping and the model-dependent actions live in `app.rs`.
//!
//! The note and CC map follows the Mackie Control Universal protocol
//! (N. Jarnoux's reverse-engineered reference and the TouchMCU
//! documentation).  The TotalMix meaning of each control follows the RME
//! user guides' MIDI Remote Control chapter.  The "simple" note commands
//! of that chapter are the same notes as the surface buttons, so one note
//! table serves both.
//!
//! Navigation counts strips, not channels: a linked stereo pair is one
//! strip unless "Map Stereo to 2 Mono Channels" is on.  The app builds
//! the strip windows from the confirmed model.

use totalcontrol_core::ChannelId;

use crate::app::Message;
use crate::osc_rme::Bus;

/// Independent MIDI remotes in TotalMix FX.  This version exposes one.
pub const REMOTES: usize = 4;
/// Channel strips per surface (the master fader is separate).
pub const STRIPS: usize = 8;
/// Characters per LCD row; the LCD is two rows of 56.
pub const LCD_ROW: usize = 56;
/// Characters per strip in the full LCD layout (8 x 7 = 56).
pub const LCD_STRIP: usize = 7;

/// One ALSA rawmidi port: card and device index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MidiPort {
    pub card: u32,
    pub device: u32,
}

impl MidiPort {
    /// The ALSA hardware name, as accepted by `Rawmidi::new`.
    pub fn hw_name(&self) -> String {
        format!("hw:{},{}", self.card, self.device)
    }
}

/// One remote's settings — the TotalMix FX MIDI page for one index.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MackieConfig {
    /// "In Use": the remote is active.
    pub in_use: bool,
    /// MIDI In port the surface is connected to.
    pub input: Option<MidiPort>,
    /// MIDI Out port for feedback; `None` means control without feedback.
    pub output: Option<MidiPort>,
    /// "Enable Protocol Support".  When off, only the simple CC commands
    /// of the RME MIDI chapter are accepted.
    pub protocol: bool,
    /// "Extender to": index of the main remote this one extends.  The
    /// group shares one navigation state and forms one fader block.
    /// Reserved for the multi-remote increment; not exposed yet.
    pub extender_to: Option<usize>,
    /// "Map Stereo to 2 Mono Channels": one strip per channel instead of
    /// one strip per linked pair.
    pub stereo_to_mono: bool,
    /// "Submix Selection in Input/Playback Row": REC selects the submix
    /// from any row, not only the output row.
    pub submix_any_row: bool,
    /// "Enable full LCD support": eight names and eight values.  When
    /// off, only a brief note about the first fader is sent.
    pub full_lcd: bool,
    /// "Send User defined Names": user labels replace the channel names.
    pub send_names: bool,
    /// "Send Level Messages": meter feedback on the LCD.
    pub send_levels: bool,
    /// "Peak Hold": meters follow the held peak, not the instant level.
    pub peak_hold: bool,
    /// "Disable MIDI in background": suspend while the window has no
    /// focus or is hidden.
    pub background_disable: bool,
    /// "Submix linked to MIDI / OSC control": the remote follows the
    /// submix selected in the GUI, and vice versa.
    pub submix_linked: bool,
}

impl Default for MackieConfig {
    fn default() -> Self {
        Self {
            in_use: false,
            input: None,
            output: None,
            protocol: true,
            extender_to: None,
            stereo_to_mono: false,
            submix_any_row: false,
            full_lcd: true,
            send_names: true,
            send_levels: false,
            peak_hold: false,
            background_disable: false,
            submix_linked: true,
        }
    }
}

/// The Mackie Control Options checkboxes of the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MackieOption {
    Protocol,
    FullLcd,
    SendNames,
    SendLevels,
    PeakHold,
}

impl MackieConfig {
    /// A remote needs an input port to do anything.
    pub fn runnable(&self) -> bool {
        self.in_use && self.input.is_some()
    }

    /// Apply one settings checkbox.
    pub fn set_option(&mut self, option: MackieOption, on: bool) {
        match option {
            MackieOption::Protocol => self.protocol = on,
            MackieOption::FullLcd => self.full_lcd = on,
            MackieOption::SendNames => self.send_names = on,
            MackieOption::SendLevels => self.send_levels = on,
            MackieOption::PeakHold => self.peak_hold = on,
        }
    }

    pub fn option(&self, option: MackieOption) -> bool {
        match option {
            MackieOption::Protocol => self.protocol,
            MackieOption::FullLcd => self.full_lcd,
            MackieOption::SendNames => self.send_names,
            MackieOption::SendLevels => self.send_levels,
            MackieOption::PeakHold => self.peak_hold,
        }
    }
}

// ---- note map ---------------------------------------------------------

pub const NOTE_REC: u8 = 0x00; // ..0x07
pub const NOTE_SOLO: u8 = 0x08; // ..0x0f
pub const NOTE_MUTE: u8 = 0x10; // ..0x17
pub const NOTE_SELECT: u8 = 0x18; // ..0x1f
pub const NOTE_VPOT_PUSH: u8 = 0x20; // ..0x27
pub const NOTE_ASSIGN_TRACK: u8 = 0x28;
pub const NOTE_ASSIGN_SEND: u8 = 0x29;
pub const NOTE_ASSIGN_PAN: u8 = 0x2a;
pub const NOTE_ASSIGN_PLUGIN: u8 = 0x2b;
pub const NOTE_ASSIGN_EQ: u8 = 0x2c;
pub const NOTE_ASSIGN_INSTRUMENT: u8 = 0x2d;
pub const NOTE_BANK_LEFT: u8 = 0x2e;
pub const NOTE_BANK_RIGHT: u8 = 0x2f;
pub const NOTE_CHANNEL_LEFT: u8 = 0x30;
pub const NOTE_CHANNEL_RIGHT: u8 = 0x31;
pub const NOTE_FLIP: u8 = 0x32;
pub const NOTE_GLOBAL_VIEW: u8 = 0x33;
pub const NOTE_F1: u8 = 0x36; // ..0x3d = F8
pub const NOTE_MIDI_TRACKS: u8 = 0x3e; // F9 / Cue Main Out
pub const NOTE_INPUTS: u8 = 0x3f; // F10 / Cue Phones 1
pub const NOTE_AUDIO_TRACKS: u8 = 0x40; // F11 / Cue Phones 2
pub const NOTE_AUDIO_INSTRUMENTS: u8 = 0x41; // F12 / Cue Phones 3
pub const NOTE_AUX: u8 = 0x42; // Cue Phones 4
pub const NOTE_SHIFT: u8 = 0x46;
pub const NOTE_SAVE: u8 = 0x50;
pub const NOTE_UNDO: u8 = 0x51;
pub const NOTE_REWIND: u8 = 0x5b;
pub const NOTE_FORWARD: u8 = 0x5c;
pub const NOTE_STOP: u8 = 0x5d;
pub const NOTE_PLAY: u8 = 0x5e;
pub const NOTE_RECORD: u8 = 0x5f;
pub const NOTE_CURSOR_UP: u8 = 0x60;
pub const NOTE_CURSOR_DOWN: u8 = 0x61;
pub const NOTE_CURSOR_LEFT: u8 = 0x62;
pub const NOTE_CURSOR_RIGHT: u8 = 0x63;
pub const NOTE_ZOOM: u8 = 0x64;
pub const NOTE_SCRUB: u8 = 0x65;
pub const NOTE_USER1: u8 = 0x66; // Assignable 1 / PAGE+
pub const NOTE_USER2: u8 = 0x67; // Assignable 2 / PAGE-
pub const NOTE_FADER_TOUCH: u8 = 0x68; // ..0x6f strips, 0x70 master
pub const NOTE_RUDE_SOLO: u8 = 0x73;

/// The loopback-detection note TotalMix sends: note 127, velocity 90,
/// channel 16.  A surface never sends it; hearing it on the input means
/// the output is wired back into the input.
pub const LOOPBACK_NOTE: u8 = 0x7f;
pub const LOOPBACK_VELOCITY: u8 = 0x5a;
pub const LOOPBACK_CHANNEL: u8 = 15;

pub const CC_VPOT: u8 = 0x10; // ..0x17 turn
pub const CC_VPOT_RING: u8 = 0x30; // ..0x37 LED ring
pub const CC_JOG: u8 = 0x3c;
/// Simple CC faders: 102..117 on channels 1..12 (rows 1..3).
pub const CC_FADER_BASE: u8 = 102;
/// Main Out volume: standard CC Volume on channel 1.
pub const CC_VOLUME: u8 = 7;
/// Input gain: CC 9, the value is the gain itself.
pub const CC_GAIN: u8 = 9;
/// Trim Gains on/off and submix selection live on channel 13.
pub const CC_TRIM_CHANNEL: u8 = 12;
pub const CC_TRIM_ON: u8 = 102;
pub const CC_TRIM_OFF: u8 = 103;
pub const CC_SUBMIX_BASE: u8 = 104;
/// Reverb / Echo user presets: CC 12 on channels 3 / 4.
pub const CC_FX_PRESET: u8 = 12;

// ---- MIDI byte stream parser ------------------------------------------

/// One parsed MIDI event.  The kernel delivers a raw byte stream, so the
/// host expands running status and frames SysEx itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiEvent {
    NoteOn { ch: u8, note: u8, vel: u8 },
    NoteOff { ch: u8, note: u8 },
    Cc { ch: u8, cc: u8, val: u8 },
    PitchBend { ch: u8, value: u16 },
    ChannelPressure { ch: u8, val: u8 },
    SysEx(Vec<u8>),
    Realtime(u8),
}

/// Running-status parser with SysEx framing.  SysEx is capped: the kernel
/// transport carries at most 128 bytes per message anyway.
#[derive(Debug, Default)]
pub struct MidiParser {
    status: u8,
    data: [u8; 2],
    len: usize,
    need: usize,
    sysex: Vec<u8>,
    in_sysex: bool,
}

const SYSEX_CAP: usize = 512;

impl MidiParser {
    /// Feed one byte; returns an event when a message completes.
    pub fn feed(&mut self, b: u8) -> Option<MidiEvent> {
        if b >= 0xf8 {
            return Some(MidiEvent::Realtime(b));
        }
        if b == 0xf0 {
            self.in_sysex = true;
            self.sysex.clear();
            self.sysex.push(b);
            self.status = 0;
            self.len = 0;
            return None;
        }
        if b == 0xf7 {
            if self.in_sysex {
                self.in_sysex = false;
                if self.sysex.len() < SYSEX_CAP {
                    self.sysex.push(b);
                    return Some(MidiEvent::SysEx(std::mem::take(&mut self.sysex)));
                }
                self.sysex.clear();
            }
            return None;
        }
        if self.in_sysex {
            if b & 0x80 != 0 {
                // A status byte aborts an unterminated SysEx.
                self.in_sysex = false;
                self.sysex.clear();
            } else {
                if self.sysex.len() < SYSEX_CAP {
                    self.sysex.push(b);
                }
                return None;
            }
        }
        if b & 0x80 != 0 {
            self.status = b;
            self.len = 0;
            self.need = match b & 0xf0 {
                0xc0 | 0xd0 => 1,
                0xf0 => match b {
                    0xf1 | 0xf3 => 1,
                    0xf2 => 2,
                    _ => 0,
                },
                _ => 2,
            };
            if self.need == 0 {
                let ev = self.complete();
                self.status = 0;
                return ev;
            }
            return None;
        }
        if self.status == 0 {
            return None; // stray data byte
        }
        if self.len < 2 {
            self.data[self.len] = b;
        }
        self.len += 1;
        if self.len >= self.need {
            let ev = self.complete();
            self.len = 0; // running status keeps self.status
            return ev;
        }
        None
    }

    fn complete(&self) -> Option<MidiEvent> {
        let ch = self.status & 0x0f;
        Some(match self.status & 0xf0 {
            0x80 => MidiEvent::NoteOff {
                ch,
                note: self.data[0],
            },
            0x90 => {
                if self.data[1] == 0 {
                    MidiEvent::NoteOff {
                        ch,
                        note: self.data[0],
                    }
                } else {
                    MidiEvent::NoteOn {
                        ch,
                        note: self.data[0],
                        vel: self.data[1],
                    }
                }
            }
            0xb0 => MidiEvent::Cc {
                ch,
                cc: self.data[0],
                val: self.data[1],
            },
            0xc0 => return None, // program change: not used by the surface
            0xd0 => MidiEvent::ChannelPressure {
                ch,
                val: self.data[0],
            },
            0xe0 => MidiEvent::PitchBend {
                ch,
                value: u16::from(self.data[0]) | (u16::from(self.data[1]) << 7),
            },
            _ => return None,
        })
    }
}

// ---- navigation state --------------------------------------------------

/// Per-remote navigation.  Not part of the mixer scene, like `RmeState`.
/// `bank` counts strips, not channels.
#[derive(Debug)]
pub struct MackieNav {
    pub bus: Bus,
    /// First strip of the window, relative to the row.
    pub bank: usize,
    /// Output pair whose submix is being edited.
    pub submix: usize,
    /// Trim Gains mode (DYN / note 0x2d).
    pub trim: bool,
    /// The handshake completed (or a control message arrived without one).
    pub online: bool,
    /// Serial number of the surface, from the connection query.
    pub serial: [u8; 7],
    pub parser: MidiParser,
    /// SEL buttons held, for the SEL+DYN trim combination.
    pub sel_held: [bool; STRIPS],
}

impl Default for MackieNav {
    fn default() -> Self {
        Self {
            bus: Bus::Input,
            bank: 0,
            submix: 0,
            trim: false,
            online: false,
            serial: [0; 7],
            parser: MidiParser::default(),
            sel_held: [false; STRIPS],
        }
    }
}

impl MackieNav {
    /// The part of the state that changes which controls are visible.
    pub fn nav(&self) -> (Bus, usize, usize, bool) {
        (self.bus, self.bank, self.submix, self.trim)
    }
}

/// The strip window: which channel each of the eight strips addresses.
/// Built by the app from the navigation state and the confirmed model.
pub type Window = [Option<ChannelId>; STRIPS];

/// A model-dependent action the codec cannot resolve on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MackieAction {
    /// EQ button: mute the Main Out pair.
    MasterMute,
    /// PLUGINS/INSERT button: solo the Main Out pair.
    MasterSolo,
    /// F9: select the Main Out submix.
    SelectMainOut,
    /// Cue Main Out: listen to the Main Out.
    CueMainOut,
    /// Cue Phones 1..4: listen to the phones mix of that slot.
    CuePhones(usize),
}

fn row_len(counts: [usize; 3], bus: Bus) -> usize {
    match bus {
        Bus::Input => counts[0],
        Bus::Playback => counts[1],
        Bus::Output => counts[2],
    }
}

fn channel(bus: Bus, idx: usize) -> ChannelId {
    match bus {
        Bus::Input => ChannelId::Input(idx),
        Bus::Playback => ChannelId::Playback(idx),
        Bus::Output => ChannelId::Output(idx),
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

/// Fader position (normalized 0..1) to linear gain, through the RME fader
/// curve.  Identical to the OSC scale.
pub fn fader_to_gain(p: f32) -> f32 {
    let db = crate::osc_rme::fader_db(p);
    if db <= -65.0 {
        0.0
    } else {
        10f32.powf(db / 20.0).clamp(0.0, 2.0)
    }
}

/// Parse one surface event.  Returns the messages to dispatch, or `None`
/// when the event is outside the surface.  Navigation changes mutate
/// `nav`; the caller detects them with [`MackieNav::nav`] and refreshes
/// the feedback.
///
/// `window` is the current row's strip window, `out_window` the output
/// row's (REC selects the submix under the strip).  `counts` are channel
/// counts for the simple CC faders, `strip_counts` for navigation.
pub fn parse(
    ev: &MidiEvent,
    nav: &mut MackieNav,
    cfg: &MackieConfig,
    window: &Window,
    out_window: &Window,
    counts: [usize; 3],
    strip_counts: [usize; 3],
    submix_count: usize,
) -> Option<Vec<Message>> {
    match ev {
        MidiEvent::NoteOn { ch, note, vel } => {
            if *ch == LOOPBACK_CHANNEL && *note == LOOPBACK_NOTE && *vel == LOOPBACK_VELOCITY {
                // The caller trips the loopback guard; nothing to dispatch.
                return Some(Vec::new());
            }
            if *ch != 0 {
                return None;
            }
            if !cfg.protocol {
                // The simple note commands are part of the Mackie
                // protocol; with support off only the CC commands work.
                return None;
            }
            let pressed = *vel >= 64;
            parse_note(
                *note, pressed, nav, cfg, window, out_window, strip_counts, submix_count,
            )
        }
        MidiEvent::NoteOff { ch, note } => {
            if *ch != 0 || !cfg.protocol {
                return None;
            }
            // Release: only the SEL hold state matters.
            if (NOTE_SELECT..NOTE_SELECT + STRIPS as u8).contains(note) {
                nav.sel_held[(note - NOTE_SELECT) as usize] = false;
            }
            Some(Vec::new())
        }
        MidiEvent::PitchBend { ch, value } => {
            if !cfg.protocol {
                return None;
            }
            let p = f32::from(*value) / 16383.0;
            if *ch == 8 {
                return Some(vec![Message::RmeMasterVolume(fader_to_gain(p))]);
            }
            if *ch as usize >= STRIPS {
                return None;
            }
            let cid = window[*ch as usize]?;
            Some(vec![fader_message(nav, cid, p)])
        }
        MidiEvent::Cc { ch, cc, val } => {
            parse_cc(*ch, *cc, *val, nav, cfg, window, counts, submix_count)
        }
        MidiEvent::SysEx(data) => parse_sysex(data, nav),
        _ => None,
    }
}

fn fader_message(nav: &MackieNav, cid: ChannelId, p: f32) -> Message {
    if nav.trim && !matches!(cid, ChannelId::Output(_)) {
        // Trim mode: the fader writes the channel trim in dB.
        Message::TrimChanged(cid, crate::osc_rme::fader_db(p))
    } else {
        let out = match cid {
            ChannelId::Output(i) => i,
            _ => nav.submix,
        };
        Message::VolumeChanged(cid, out, fader_to_gain(p))
    }
}

fn parse_note(
    note: u8,
    pressed: bool,
    nav: &mut MackieNav,
    cfg: &MackieConfig,
    window: &Window,
    out_window: &Window,
    strip_counts: [usize; 3],
    submix_count: usize,
) -> Option<Vec<Message>> {
    use crate::osc_rme::RmeToggle as T;
    let strip = |n: u8| -> Option<ChannelId> { window.get(n as usize).copied().flatten() };
    let mut msgs = Vec::new();
    match note {
        NOTE_MUTE..=0x17 if pressed => {
            msgs.push(Message::RmeToggle(T::Mute(strip(note - NOTE_MUTE)?)));
        }
        NOTE_SOLO..=0x0f if pressed => {
            msgs.push(Message::RmeToggle(T::Solo(strip(note - NOTE_SOLO)?)));
        }
        NOTE_SELECT..=0x1f => {
            let idx = (note - NOTE_SELECT) as usize;
            nav.sel_held[idx] = pressed;
            if pressed {
                if let Some(cid) = strip(note - NOTE_SELECT) {
                    msgs.push(Message::RmeSelect(cid, true));
                }
            }
        }
        NOTE_REC..=0x07 if pressed => {
            // REC selects the output bus under the strip.  Without the
            // option it only works while the output row is shown.
            if cfg.submix_any_row || nav.bus == Bus::Output {
                if let Some(ChannelId::Output(i)) = out_window.get((note - NOTE_REC) as usize).copied().flatten() {
                    let pair = i / 2;
                    if pair < submix_count.max(1) {
                        nav.submix = pair;
                        msgs.push(Message::SelectOutput(pair));
                    }
                }
            }
        }
        NOTE_VPOT_PUSH..=0x27 if pressed => {
            let cid = strip(note - NOTE_VPOT_PUSH)?;
            let out = match cid {
                ChannelId::Output(i) => i,
                _ => nav.submix,
            };
            msgs.push(Message::PanChanged(cid, out, 0));
        }
        NOTE_BANK_LEFT | NOTE_CURSOR_LEFT if pressed => {
            shift_bank(nav, strip_counts, -(STRIPS as isize))
        }
        NOTE_BANK_RIGHT | NOTE_CURSOR_RIGHT if pressed => {
            shift_bank(nav, strip_counts, STRIPS as isize)
        }
        NOTE_CHANNEL_LEFT | NOTE_REWIND if pressed => shift_bank(nav, strip_counts, -1),
        NOTE_CHANNEL_RIGHT | NOTE_FORWARD if pressed => shift_bank(nav, strip_counts, 1),
        NOTE_CURSOR_UP | NOTE_USER1 if pressed => {
            set_bus(nav, strip_counts, match nav.bus {
                Bus::Input => Bus::Output,
                Bus::Playback => Bus::Input,
                Bus::Output => Bus::Playback,
            });
        }
        NOTE_CURSOR_DOWN | NOTE_USER2 if pressed => {
            set_bus(nav, strip_counts, match nav.bus {
                Bus::Input => Bus::Playback,
                Bus::Playback => Bus::Output,
                Bus::Output => Bus::Input,
            });
        }
        NOTE_ASSIGN_EQ if pressed => msgs.push(Message::MackieAction(MackieAction::MasterMute)),
        NOTE_ASSIGN_PLUGIN if pressed => msgs.push(Message::MackieAction(MackieAction::MasterSolo)),
        NOTE_STOP if pressed => msgs.push(Message::RmeToggle(T::MainDim)),
        NOTE_PLAY if pressed => msgs.push(Message::RmeToggle(T::MainTalkback)),
        NOTE_ASSIGN_PAN if pressed => msgs.push(Message::RmeToggle(T::MainMono)),
        NOTE_FLIP if pressed => msgs.push(Message::RmeToggle(T::MainSpeakerB)),
        NOTE_ASSIGN_INSTRUMENT if pressed => {
            // DYN toggles Trim Gains.  SEL+DYN activates trim mode for the
            // held strips; the model trim is per channel, so both forms
            // enable the same mode.
            nav.trim = !nav.trim;
        }
        NOTE_RECORD if pressed => msgs.push(Message::ControlRoomRecall),
        NOTE_F1..=0x3d if pressed => {
            msgs.push(Message::SnapshotClicked(note - NOTE_F1 + 1));
        }
        NOTE_MIDI_TRACKS if pressed => {
            msgs.push(Message::MackieAction(MackieAction::SelectMainOut));
            msgs.push(Message::MackieAction(MackieAction::CueMainOut));
        }
        NOTE_INPUTS..=NOTE_AUX if pressed => {
            msgs.push(Message::MackieAction(MackieAction::CuePhones(
                (note - NOTE_INPUTS) as usize,
            )));
        }
        NOTE_FADER_TOUCH..=0x70 => {
            // Fader touch: carried for surface compatibility, no action.
        }
        NOTE_RUDE_SOLO | NOTE_ZOOM | NOTE_SCRUB | NOTE_GLOBAL_VIEW | NOTE_SHIFT
        | NOTE_SAVE | NOTE_UNDO | NOTE_ASSIGN_TRACK | NOTE_ASSIGN_SEND => {
            // No TotalMix meaning; ignored.
        }
        _ => return None,
    }
    Some(msgs)
}

fn parse_cc(
    ch: u8,
    cc: u8,
    val: u8,
    nav: &mut MackieNav,
    cfg: &MackieConfig,
    window: &Window,
    counts: [usize; 3],
    submix_count: usize,
) -> Option<Vec<Message>> {
    // The simple CC commands work even with protocol support off.
    if ch == 0 && cc == CC_VOLUME {
        let p = f32::from(val) / 127.0;
        return Some(vec![Message::RmeMasterVolume(fader_to_gain(p))]);
    }
    if ch == CC_TRIM_CHANNEL {
        match cc {
            CC_TRIM_ON => {
                nav.trim = true;
                return Some(Vec::new());
            }
            CC_TRIM_OFF => {
                nav.trim = false;
                return Some(Vec::new());
            }
            c if c >= CC_SUBMIX_BASE => {
                let idx = ((c - CC_SUBMIX_BASE) / 2) as usize;
                if idx < submix_count.max(1) {
                    nav.submix = idx;
                    return Some(vec![Message::SelectOutput(idx)]);
                }
                return None;
            }
            _ => return None,
        }
    }
    if cc == CC_GAIN && ch < 16 {
        // The value is the gain itself; the app clamps it to the input's
        // range and the device's controllable inputs.
        return Some(vec![Message::Gain(ch as usize, u32::from(val))]);
    }
    if cc == CC_FX_PRESET && (ch == 2 || ch == 3) {
        // Reverb / Echo user presets: the model has no user presets yet.
        log::debug!("Mackie: FX user preset {val} on channel {} is not supported", ch + 1);
        return None;
    }
    if (CC_FADER_BASE..CC_FADER_BASE + 16).contains(&cc) && ch < 12 {
        // Simple CC faders: rows 1..3 on channels 1..4, 5..8, 9..12.
        // These are the chapter-29.5 CC commands, so they work with
        // protocol support off too.
        let row = (ch / 4) as usize;
        let idx = (ch % 4) as usize * 16 + (cc - CC_FADER_BASE) as usize;
        let bus = match row {
            0 => Bus::Input,
            1 => Bus::Playback,
            _ => Bus::Output,
        };
        if idx >= row_len(counts, bus) {
            return None;
        }
        let cid = channel(bus, idx);
        let p = f32::from(val) / 127.0;
        let out = match cid {
            ChannelId::Output(i) => i,
            _ => nav.submix,
        };
        return Some(vec![if nav.trim && row != 2 {
            Message::TrimChanged(cid, crate::osc_rme::fader_db(p))
        } else {
            Message::VolumeChanged(cid, out, fader_to_gain(p))
        }]);
    }
    if !cfg.protocol {
        return None;
    }
    if ch == 0 && (CC_VPOT..CC_VPOT + STRIPS as u8).contains(&cc) {
        // Relative turn: 1..63 clockwise, 65..127 counter-clockwise.
        let steps = if val >= 65 {
            -i16::from(val - 64)
        } else {
            i16::from(val.min(63))
        };
        if steps == 0 {
            return Some(Vec::new());
        }
        let strip = (cc - CC_VPOT) as usize;
        let cid = window[strip]?;
        let out = match cid {
            ChannelId::Output(i) => i,
            _ => nav.submix,
        };
        return Some(vec![Message::MackiePanNudge(cid, out, steps * 2)]);
    }
    None
}

// ---- SysEx: handshake and LCD ------------------------------------------

pub const SYSEX_HEADER: [u8; 5] = [0xf0, 0x00, 0x00, 0x66, 0x14];

pub const SYX_DEVICE_QUERY: u8 = 0x00;
pub const SYX_HOST_CONNECTION_QUERY: u8 = 0x01;
pub const SYX_HOST_CONNECTION_REPLY: u8 = 0x02;
pub const SYX_HOST_CONNECTION_CONFIRM: u8 = 0x03;
pub const SYX_HOST_CONNECTION_ERROR: u8 = 0x04;
pub const SYX_GO_OFFLINE: u8 = 0x0f;
pub const SYX_LCD: u8 = 0x12;
pub const SYX_VERSION_REQUEST: u8 = 0x13;
pub const SYX_VERSION_REPLY: u8 = 0x14;

/// The device query the host sends when a port opens.
pub fn device_query() -> Vec<u8> {
    let mut v = SYSEX_HEADER.to_vec();
    v.push(SYX_DEVICE_QUERY);
    v.push(0xf7);
    v
}

pub fn version_request() -> Vec<u8> {
    let mut v = SYSEX_HEADER.to_vec();
    v.push(SYX_VERSION_REQUEST);
    v.push(0x00);
    v.push(0xf7);
    v
}

pub fn go_offline() -> Vec<u8> {
    let mut v = SYSEX_HEADER.to_vec();
    v.push(SYX_GO_OFFLINE);
    v.push(0x7f);
    v.push(0xf7);
    v
}

/// The challenge/response algorithm of the Mackie connection handshake.
/// The intermediates are wider than a byte, as in the C reference.
pub fn challenge_response(c: &[u8; 4]) -> [u8; 4] {
    let c = [i32::from(c[0]), i32::from(c[1]), i32::from(c[2]), i32::from(c[3])];
    let r = [
        0x7f & (c[0] + (c[1] ^ 0x0a) - c[3]),
        0x7f & ((c[2] >> 4) ^ (c[0] + c[3])),
        0x7f & ((c[3] - (c[2] << 2)) ^ (c[0] | c[1])),
        0x7f & (c[1] - c[2] + (0xf0 ^ (c[3] << 4))),
    ];
    [r[0] as u8, r[1] as u8, r[2] as u8, r[3] as u8]
}

/// The host connection reply: serial echo plus the response code.
pub fn host_connection_reply(serial: &[u8; 7], challenge: &[u8; 4]) -> Vec<u8> {
    let mut v = SYSEX_HEADER.to_vec();
    v.push(SYX_HOST_CONNECTION_REPLY);
    v.extend_from_slice(serial);
    v.extend_from_slice(&challenge_response(challenge));
    v.push(0xf7);
    v
}

/// The version reply to a surface's version request.
pub fn version_reply() -> Vec<u8> {
    let mut v = SYSEX_HEADER.to_vec();
    v.push(SYX_VERSION_REPLY);
    v.extend_from_slice(b"V1.0\x00");
    v.push(0xf7);
    v
}

/// One parsed host-side SysEx command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysexCommand {
    /// The surface asks for a host: serial + challenge.
    ConnectionQuery { serial: [u8; 7], challenge: [u8; 4] },
    /// The surface confirmed the handshake.
    ConnectionConfirm,
    /// The surface reports a handshake error.
    ConnectionError,
    /// The surface answers a version request.
    VersionReply,
    /// A device query (some surfaces send it instead of the query).
    DeviceQuery,
}

pub fn parse_sysex_command(data: &[u8]) -> Option<SysexCommand> {
    if data.len() < 7 || data[0] != 0xf0 || data[data.len() - 1] != 0xf7 {
        return None;
    }
    if &data[1..5] != &SYSEX_HEADER[1..5] {
        return None;
    }
    let body = &data[5..data.len() - 1];
    match *body.first()? {
        SYX_DEVICE_QUERY => Some(SysexCommand::DeviceQuery),
        SYX_HOST_CONNECTION_QUERY if body.len() >= 12 => {
            let mut serial = [0u8; 7];
            let mut challenge = [0u8; 4];
            serial.copy_from_slice(&body[1..8]);
            challenge.copy_from_slice(&body[8..12]);
            Some(SysexCommand::ConnectionQuery { serial, challenge })
        }
        SYX_HOST_CONNECTION_CONFIRM => Some(SysexCommand::ConnectionConfirm),
        SYX_HOST_CONNECTION_ERROR => Some(SysexCommand::ConnectionError),
        SYX_VERSION_REPLY => Some(SysexCommand::VersionReply),
        _ => None,
    }
}

fn parse_sysex(data: &[u8], nav: &mut MackieNav) -> Option<Vec<Message>> {
    match parse_sysex_command(data)? {
        SysexCommand::ConnectionQuery { serial, challenge } => {
            nav.serial = serial;
            // The reply is sent by the app, which owns the output queue.
            nav.online = false;
            Some(vec![Message::MackieHandshake(challenge)])
        }
        SysexCommand::ConnectionConfirm => {
            nav.online = true;
            Some(Vec::new())
        }
        SysexCommand::ConnectionError => {
            log::warn!("Mackie: surface reported a connection error");
            Some(Vec::new())
        }
        SysexCommand::DeviceQuery => Some(vec![Message::MackieHandshake([0; 4])]),
        SysexCommand::VersionReply => Some(Vec::new()),
    }
}

// ---- feedback encoding --------------------------------------------------

pub fn pitch_bend(ch: u8, value: u16) -> [u8; 3] {
    let v = value.min(16383);
    [0xe0 | (ch & 0x0f), (v & 0x7f) as u8, (v >> 7) as u8]
}

pub fn note_on(ch: u8, note: u8, vel: u8) -> [u8; 3] {
    [0x90 | (ch & 0x0f), note & 0x7f, vel & 0x7f]
}

pub fn control_change(ch: u8, cc: u8, val: u8) -> [u8; 3] {
    [0xb0 | (ch & 0x0f), cc & 0x7f, val & 0x7f]
}

pub fn channel_pressure(ch: u8, val: u8) -> [u8; 2] {
    [0xd0 | (ch & 0x0f), val & 0x7f]
}

/// LCD write: offset 0..55 top row, 56..111 bottom row.
pub fn lcd_write(offset: usize, text: &[u8]) -> Vec<u8> {
    let mut v = SYSEX_HEADER.to_vec();
    v.push(SYX_LCD);
    v.push((offset.min(111)) as u8);
    for &b in text {
        if v.len() >= 118 {
            break;
        }
        v.push(b & 0x7f);
    }
    v.push(0xf7);
    v
}

/// The loopback-detection note, sent periodically on the output port.
pub fn loopback_note() -> [u8; 3] {
    note_on(LOOPBACK_CHANNEL, LOOPBACK_NOTE, LOOPBACK_VELOCITY)
}

/// VPOT ring: mode 0 (single dot), position 0..11 from the pan, and the
/// center LED under the knob.
pub fn vpot_ring(strip: usize, pan: i8, center: bool) -> [u8; 3] {
    let pos = (((f32::from(pan) + 100.0) / 200.0) * 11.0).round().clamp(0.0, 11.0) as u8;
    let mut val = pos;
    if center {
        val |= 0x40;
    }
    control_change(0, CC_VPOT_RING + strip as u8, val)
}

/// Meter level in dB to the Mackie channel-pressure nibble.
pub fn meter_nibble(db: f32) -> u8 {
    if db >= 0.0 {
        0x0d
    } else if db >= -2.0 {
        0x0b
    } else if db >= -4.0 {
        0x0a
    } else if db >= -6.0 {
        0x09
    } else if db >= -8.0 {
        0x08
    } else if db >= -10.0 {
        0x07
    } else if db >= -14.0 {
        0x06
    } else if db >= -20.0 {
        0x05
    } else if db >= -30.0 {
        0x04
    } else if db >= -40.0 {
        0x03
    } else if db >= -50.0 {
        0x02
    } else if db >= -60.0 {
        0x01
    } else {
        0x00
    }
}

/// One strip's feedback content, built by the app from the model.
#[derive(Debug, Clone)]
pub struct StripFeedback {
    pub fader: f32,
    pub mute: bool,
    pub solo: bool,
    pub select: bool,
    /// REC LED: this strip's output is the selected submix.
    pub rec: bool,
    pub pan: i8,
    pub name: String,
    pub value: String,
}

impl Default for StripFeedback {
    fn default() -> Self {
        Self {
            fader: 0.0,
            mute: false,
            solo: false,
            select: false,
            rec: false,
            pan: 0,
            name: String::new(),
            value: String::new(),
        }
    }
}

/// Everything the feedback encoder needs for one refresh.
#[derive(Debug, Clone)]
pub struct FeedbackInput {
    pub strips: [StripFeedback; STRIPS],
    pub master_fader: f32,
    pub dim: bool,
    pub talkback: bool,
    pub mono: bool,
    pub speaker_b: bool,
    pub master_mute: bool,
    pub master_solo: bool,
    pub trim: bool,
    pub rude_solo: bool,
    pub full_lcd: bool,
    pub send_names: bool,
    /// Row label for the brief LCD mode ("In", "Play", "Out").
    pub row_name: &'static str,
    /// First channel number of the window, 1-based, for the brief mode.
    pub first_channel: usize,
}

/// Last-sent feedback values, so a refresh only emits the deltas.
#[derive(Debug, Clone)]
pub struct MackieSurface {
    pub faders: [u16; STRIPS + 1],
    pub leds: [u8; 128],
    pub rings: [u8; STRIPS],
    pub lcd: [u8; LCD_ROW * 2],
    pub meters: [u8; STRIPS],
}

impl Default for MackieSurface {
    fn default() -> Self {
        Self {
            faders: [u16::MAX; STRIPS + 1],
            leds: [u8::MAX; 128],
            rings: [u8::MAX; STRIPS],
            lcd: [u8::MAX; LCD_ROW * 2],
            meters: [u8::MAX; STRIPS],
        }
    }
}

fn pad7(s: &str) -> [u8; LCD_STRIP] {
    let mut out = [b' '; LCD_STRIP];
    for (i, b) in s.bytes().take(LCD_STRIP).enumerate() {
        out[i] = if (0x20..0x7f).contains(&b) { b } else { b' ' };
    }
    out
}

/// Encode the changed feedback into `out`.  Emits nothing when the
/// surface already shows everything.
pub fn refresh(input: &FeedbackInput, surface: &mut MackieSurface, out: &mut Vec<u8>) {
    let mut led = |note: u8, on: bool, out: &mut Vec<u8>| {
        let vel = if on { 127 } else { 0 };
        if surface.leds[note as usize] != vel {
            surface.leds[note as usize] = vel;
            out.extend_from_slice(&note_on(0, note, vel));
        }
    };
    for (i, strip) in input.strips.iter().enumerate() {
        let v = (strip.fader.clamp(0.0, 1.0) * 16383.0).round() as u16;
        if surface.faders[i] != v {
            surface.faders[i] = v;
            out.extend_from_slice(&pitch_bend(i as u8, v));
        }
        led(NOTE_MUTE + i as u8, strip.mute, out);
        led(NOTE_SOLO + i as u8, strip.solo, out);
        led(NOTE_SELECT + i as u8, strip.select, out);
        led(NOTE_REC + i as u8, strip.rec, out);
        let ring = vpot_ring(i, strip.pan, strip.pan == 0);
        if surface.rings[i] != ring[2] {
            surface.rings[i] = ring[2];
            out.extend_from_slice(&ring);
        }
    }
    let master = (input.master_fader.clamp(0.0, 1.0) * 16383.0).round() as u16;
    if surface.faders[STRIPS] != master {
        surface.faders[STRIPS] = master;
        out.extend_from_slice(&pitch_bend(8, master));
    }
    led(NOTE_STOP, input.dim, out);
    led(NOTE_PLAY, input.talkback, out);
    led(NOTE_ASSIGN_PAN, input.mono, out);
    led(NOTE_FLIP, input.speaker_b, out);
    led(NOTE_ASSIGN_EQ, input.master_mute, out);
    led(NOTE_ASSIGN_PLUGIN, input.master_solo, out);
    led(NOTE_ASSIGN_INSTRUMENT, input.trim, out);
    led(NOTE_RUDE_SOLO, input.rude_solo, out);

    // LCD: full layout is eight 7-character names over eight values.
    let mut lcd = [b' '; LCD_ROW * 2];
    if input.full_lcd {
        for (i, strip) in input.strips.iter().enumerate() {
            let name = if input.send_names {
                strip.name.as_str()
            } else {
                ""
            };
            lcd[i * LCD_STRIP..(i + 1) * LCD_STRIP].copy_from_slice(&pad7(name));
            lcd[LCD_ROW + i * LCD_STRIP..LCD_ROW + (i + 1) * LCD_STRIP]
                .copy_from_slice(&pad7(&strip.value));
        }
    } else {
        let brief = format!("{} {}", input.row_name, input.first_channel);
        for (i, b) in brief.bytes().take(LCD_ROW).enumerate() {
            lcd[i] = b;
        }
    }
    // Only write the runs that changed, one SysEx per contiguous run.
    let mut i = 0;
    while i < lcd.len() {
        if lcd[i] != surface.lcd[i] {
            let start = i;
            while i < lcd.len() && lcd[i] != surface.lcd[i] && i - start < LCD_ROW {
                i += 1;
            }
            out.extend_from_slice(&lcd_write(start, &lcd[start..i]));
            surface.lcd[start..i].copy_from_slice(&lcd[start..i]);
        } else {
            i += 1;
        }
    }
}

/// Meter feedback for the eight strips: channel pressure, one per strip.
/// `levels` are dB; `over` sets the overload flag.  Only changed nibbles
/// are emitted.
pub fn refresh_meters(
    levels: &[f32; STRIPS],
    over: &[bool; STRIPS],
    surface: &mut MackieSurface,
    out: &mut Vec<u8>,
) {
    for i in 0..STRIPS {
        let mut nib = meter_nibble(levels[i]);
        if over[i] {
            nib = 0x0e;
        }
        if surface.meters[i] != nib {
            surface.meters[i] = nib;
            out.extend_from_slice(&channel_pressure(0, (i as u8) << 4 | nib));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MackieConfig {
        MackieConfig::default()
    }

    fn window() -> Window {
        std::array::from_fn(|i| Some(ChannelId::Input(i)))
    }

    fn out_window() -> Window {
        std::array::from_fn(|i| Some(ChannelId::Output(i * 2)))
    }

    fn counts() -> [usize; 3] {
        [18, 18, 18]
    }

    fn parse_ev(ev: MidiEvent, nav: &mut MackieNav) -> Option<Vec<Message>> {
        parse(&ev, nav, &cfg(), &window(), &out_window(), counts(), counts(), 9)
    }

    #[test]
    fn parser_expands_running_status() {
        let mut p = MidiParser::default();
        assert_eq!(p.feed(0x90), None);
        assert_eq!(p.feed(0x3c), None);
        assert_eq!(
            p.feed(0x7f),
            Some(MidiEvent::NoteOn { ch: 0, note: 0x3c, vel: 0x7f })
        );
        // Running status: data bytes only.
        assert_eq!(p.feed(0x3e), None);
        assert_eq!(
            p.feed(0x01),
            Some(MidiEvent::NoteOn { ch: 0, note: 0x3e, vel: 0x01 })
        );
    }

    #[test]
    fn parser_frames_sysex_and_realtime() {
        let mut p = MidiParser::default();
        for b in [0xf0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00, 0x41] {
            assert_eq!(p.feed(b), None);
        }
        // Realtime interleaves without breaking the SysEx.
        assert_eq!(p.feed(0xf8), Some(MidiEvent::Realtime(0xf8)));
        assert_eq!(p.feed(0xf7), Some(MidiEvent::SysEx(vec![0xf0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00, 0x41, 0xf7])));
    }

    #[test]
    fn note_on_velocity_zero_is_a_release() {
        let mut p = MidiParser::default();
        p.feed(0x90);
        p.feed(0x10);
        assert_eq!(p.feed(0x00), Some(MidiEvent::NoteOff { ch: 0, note: 0x10 }));
    }

    #[test]
    fn mute_note_toggles_the_window_channel() {
        let mut nav = MackieNav::default();
        let msgs = parse_ev(
            MidiEvent::NoteOn { ch: 0, note: NOTE_MUTE + 2, vel: 127 },
            &mut nav,
        )
        .unwrap();
        assert!(matches!(
            msgs[0],
            Message::RmeToggle(crate::osc_rme::RmeToggle::Mute(ChannelId::Input(2)))
        ));
    }

    #[test]
    fn fader_pitch_bend_uses_the_fader_curve() {
        let mut nav = MackieNav::default();
        // 104/127 is the documented 0 dB CC value; the 14-bit equivalent
        // is 104/127 * 16383.
        let v = (104.0f32 / 127.0 * 16383.0).round() as u16;
        let msgs = parse_ev(MidiEvent::PitchBend { ch: 0, value: v }, &mut nav).unwrap();
        match msgs[0] {
            Message::VolumeChanged(ChannelId::Input(0), 0, gain) => {
                let db = 20.0 * gain.log10();
                assert!(db.abs() < 0.2, "expected ~0 dB, got {db}");
            }
            _ => panic!("expected a volume change"),
        }
    }

    #[test]
    fn master_fader_is_channel_nine() {
        let mut nav = MackieNav::default();
        let msgs = parse_ev(MidiEvent::PitchBend { ch: 8, value: 16383 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::RmeMasterVolume(g) if (g - 2.0).abs() < 0.01));
    }

    #[test]
    fn bank_and_channel_navigation_count_strips() {
        let mut nav = MackieNav::default();
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_BANK_RIGHT, vel: 127 }, &mut nav);
        assert_eq!(nav.bank, 8);
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_CHANNEL_RIGHT, vel: 127 }, &mut nav);
        assert_eq!(nav.bank, 9);
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_REWIND, vel: 127 }, &mut nav);
        assert_eq!(nav.bank, 8);
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_CURSOR_LEFT, vel: 127 }, &mut nav);
        assert_eq!(nav.bank, 0);
        // Clamped at zero.
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_BANK_LEFT, vel: 127 }, &mut nav);
        assert_eq!(nav.bank, 0);
    }

    #[test]
    fn row_navigation_cycles_the_three_rows() {
        let mut nav = MackieNav::default();
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_CURSOR_DOWN, vel: 127 }, &mut nav);
        assert_eq!(nav.bus, Bus::Playback);
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_CURSOR_DOWN, vel: 127 }, &mut nav);
        assert_eq!(nav.bus, Bus::Output);
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_CURSOR_UP, vel: 127 }, &mut nav);
        assert_eq!(nav.bus, Bus::Playback);
    }

    #[test]
    fn rec_selects_the_submix_only_in_the_output_row_by_default() {
        let mut nav = MackieNav::default();
        let msgs = parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_REC + 1, vel: 127 }, &mut nav)
            .unwrap();
        assert!(msgs.is_empty());
        nav.bus = Bus::Output;
        let msgs = parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_REC + 1, vel: 127 }, &mut nav)
            .unwrap();
        // out_window strip 1 is Output(2), pair 1.
        assert!(matches!(msgs[0], Message::SelectOutput(1)));
        assert_eq!(nav.submix, 1);
    }

    #[test]
    fn rec_selects_from_any_row_with_the_option() {
        let mut nav = MackieNav::default();
        let mut c = cfg();
        c.submix_any_row = true;
        let msgs = parse(
            &MidiEvent::NoteOn { ch: 0, note: NOTE_REC + 2, vel: 127 },
            &mut nav,
            &c,
            &window(),
            &out_window(),
            counts(),
            counts(),
            9,
        )
        .unwrap();
        assert!(matches!(msgs[0], Message::SelectOutput(2)));
    }

    #[test]
    fn control_room_buttons_map_to_the_rme_toggles() {
        use crate::osc_rme::RmeToggle as T;
        let mut nav = MackieNav::default();
        for (note, want) in [
            (NOTE_STOP, T::MainDim),
            (NOTE_PLAY, T::MainTalkback),
            (NOTE_ASSIGN_PAN, T::MainMono),
            (NOTE_FLIP, T::MainSpeakerB),
        ] {
            let msgs = parse_ev(MidiEvent::NoteOn { ch: 0, note, vel: 127 }, &mut nav).unwrap();
            assert!(matches!(msgs[0], Message::RmeToggle(t) if t == want));
        }
        let msgs = parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_RECORD, vel: 127 }, &mut nav)
            .unwrap();
        assert!(matches!(msgs[0], Message::ControlRoomRecall));
    }

    #[test]
    fn dyn_toggles_trim_and_the_fader_writes_trim() {
        let mut nav = MackieNav::default();
        parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_ASSIGN_INSTRUMENT, vel: 127 }, &mut nav);
        assert!(nav.trim);
        let msgs = parse_ev(MidiEvent::PitchBend { ch: 0, value: 8192 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::TrimChanged(ChannelId::Input(0), _)));
    }

    #[test]
    fn snapshots_are_one_based() {
        let mut nav = MackieNav::default();
        let msgs = parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_F1, vel: 127 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::SnapshotClicked(1)));
        let msgs = parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_F1 + 7, vel: 127 }, &mut nav)
            .unwrap();
        assert!(matches!(msgs[0], Message::SnapshotClicked(8)));
    }

    #[test]
    fn simple_cc_faders_address_all_three_rows() {
        let mut nav = MackieNav::default();
        // B0 66 68: input 1 to 0 dB.
        let msgs = parse_ev(MidiEvent::Cc { ch: 0, cc: 102, val: 104 }, &mut nav).unwrap();
        match msgs[0] {
            Message::VolumeChanged(ChannelId::Input(0), 0, gain) => {
                assert!((20.0 * gain.log10()).abs() < 0.2);
            }
            _ => panic!("expected input 1 volume"),
        }
        // B4 66 7F: playback 1 to maximum.
        let msgs = parse_ev(MidiEvent::Cc { ch: 4, cc: 102, val: 127 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::VolumeChanged(ChannelId::Playback(0), 0, g) if (g - 2.0).abs() < 0.01));
        // B8 68 68: output 3 to 0 dB.
        let msgs = parse_ev(MidiEvent::Cc { ch: 8, cc: 104, val: 104 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::VolumeChanged(ChannelId::Output(2), 2, _)));
    }

    #[test]
    fn cc_volume_on_channel_one_is_the_main_out() {
        let mut nav = MackieNav::default();
        let msgs = parse_ev(MidiEvent::Cc { ch: 0, cc: 7, val: 127 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::RmeMasterVolume(g) if (g - 2.0).abs() < 0.01));
    }

    #[test]
    fn trim_cc_and_submix_cc_on_channel_thirteen() {
        let mut nav = MackieNav::default();
        parse_ev(MidiEvent::Cc { ch: 12, cc: 102, val: 1 }, &mut nav);
        assert!(nav.trim);
        parse_ev(MidiEvent::Cc { ch: 12, cc: 103, val: 1 }, &mut nav);
        assert!(!nav.trim);
        let msgs = parse_ev(MidiEvent::Cc { ch: 12, cc: 106, val: 1 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::SelectOutput(1)));
    }

    #[test]
    fn protocol_off_keeps_the_simple_cc_commands() {
        let mut nav = MackieNav::default();
        let mut c = cfg();
        c.protocol = false;
        let msgs = parse(
            &MidiEvent::Cc { ch: 0, cc: 102, val: 104 },
            &mut nav,
            &c,
            &window(),
            &out_window(),
            counts(),
            counts(),
            9,
        )
        .unwrap();
        assert!(matches!(msgs[0], Message::VolumeChanged(..)));
        // Mackie pitch bend is ignored.
        let msgs = parse(
            &MidiEvent::PitchBend { ch: 0, value: 8192 },
            &mut nav,
            &c,
            &window(),
            &out_window(),
            counts(),
            counts(),
            9,
        );
        assert!(msgs.is_none());
    }

    #[test]
    fn vpot_turn_is_a_relative_pan_nudge() {
        let mut nav = MackieNav::default();
        let msgs = parse_ev(MidiEvent::Cc { ch: 0, cc: CC_VPOT, val: 1 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::MackiePanNudge(ChannelId::Input(0), 0, 2)));
        let msgs = parse_ev(MidiEvent::Cc { ch: 0, cc: CC_VPOT, val: 65 }, &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::MackiePanNudge(ChannelId::Input(0), 0, -2)));
    }

    #[test]
    fn vpot_push_centers_the_pan() {
        let mut nav = MackieNav::default();
        let msgs = parse_ev(MidiEvent::NoteOn { ch: 0, note: NOTE_VPOT_PUSH, vel: 127 }, &mut nav)
            .unwrap();
        assert!(matches!(msgs[0], Message::PanChanged(ChannelId::Input(0), 0, 0)));
    }

    #[test]
    fn challenge_response_matches_the_reference_algorithm() {
        // Reference vector computed from the published algorithm.
        let c = [0x3b, 0x02, 0x00, 0x54];
        let r = challenge_response(&c);
        let expect = [
            0x7f & (0x3b_u8.wrapping_add(0x02 ^ 0x0a).wrapping_sub(0x54)),
            0x7f & ((0x00 >> 4) ^ (0x3b_u8.wrapping_add(0x54))),
            0x7f & (0x54_u8.wrapping_sub(0x00 << 2) ^ (0x3b | 0x02)),
            0x7f & (0x02_u8.wrapping_sub(0x00).wrapping_add(0xf0 ^ (0x54 << 4))),
        ];
        assert_eq!(r, expect);
    }

    #[test]
    fn handshake_round_trip() {
        let mut nav = MackieNav::default();
        let serial = *b"XYZ1234";
        let challenge = [0x11, 0x22, 0x33, 0x44];
        let mut query = SYSEX_HEADER.to_vec();
        query.push(SYX_HOST_CONNECTION_QUERY);
        query.extend_from_slice(&serial);
        query.extend_from_slice(&challenge);
        query.push(0xf7);
        let msgs = parse_ev(MidiEvent::SysEx(query), &mut nav).unwrap();
        assert!(matches!(msgs[0], Message::MackieHandshake(c) if c == challenge));
        assert_eq!(nav.serial, serial);
        let reply = host_connection_reply(&serial, &challenge);
        assert_eq!(reply[5], SYX_HOST_CONNECTION_REPLY);
        assert_eq!(&reply[6..13], &serial);
        assert_eq!(&reply[13..17], &challenge_response(&challenge));
        let mut confirm = SYSEX_HEADER.to_vec();
        confirm.push(SYX_HOST_CONNECTION_CONFIRM);
        confirm.extend_from_slice(&serial);
        confirm.push(0xf7);
        parse_ev(MidiEvent::SysEx(confirm), &mut nav);
        assert!(nav.online);
    }

    #[test]
    fn lcd_full_layout_is_eight_by_seven() {
        let mut input = FeedbackInput {
            strips: std::array::from_fn(|_| StripFeedback::default()),
            master_fader: 0.5,
            dim: false,
            talkback: false,
            mono: false,
            speaker_b: false,
            master_mute: false,
            master_solo: false,
            trim: false,
            rude_solo: false,
            full_lcd: true,
            send_names: true,
            row_name: "In",
            first_channel: 1,
        };
        input.strips[0].name = "Vocals".into();
        input.strips[0].value = "0.0 dB".into();
        let mut surface = MackieSurface::default();
        let mut out = Vec::new();
        refresh(&input, &mut surface, &mut out);
        assert_eq!(&surface.lcd[0..7], b"Vocals ");
        assert_eq!(&surface.lcd[LCD_ROW..LCD_ROW + 7], b"0.0 dB ");
        // A second identical refresh emits no LCD traffic.
        let mut out2 = Vec::new();
        refresh(&input, &mut surface, &mut out2);
        assert!(out2.is_empty());
    }

    #[test]
    fn feedback_only_emits_deltas() {
        let input = FeedbackInput {
            strips: std::array::from_fn(|_| StripFeedback::default()),
            master_fader: 0.0,
            dim: false,
            talkback: false,
            mono: false,
            speaker_b: false,
            master_mute: false,
            master_solo: false,
            trim: false,
            rude_solo: false,
            full_lcd: true,
            send_names: true,
            row_name: "In",
            first_channel: 1,
        };
        let mut surface = MackieSurface::default();
        let mut out = Vec::new();
        refresh(&input, &mut surface, &mut out);
        let first = out.len();
        assert!(first > 0);
        let mut out2 = Vec::new();
        refresh(&input, &mut surface, &mut out2);
        assert!(out2.is_empty());
        // One changed fader emits exactly one pitch bend.
        let mut changed = input.clone();
        changed.strips[3].fader = 0.75;
        let mut out3 = Vec::new();
        refresh(&changed, &mut surface, &mut out3);
        assert_eq!(out3.len(), 3);
        assert_eq!(out3[0], 0xe3);
    }

    #[test]
    fn meter_nibble_follows_the_mackie_scale() {
        assert_eq!(meter_nibble(1.0), 0x0d);
        assert_eq!(meter_nibble(0.0), 0x0d);
        assert_eq!(meter_nibble(-3.0), 0x0a);
        assert_eq!(meter_nibble(-12.0), 0x06);
        assert_eq!(meter_nibble(-70.0), 0x00);
    }

    #[test]
    fn loopback_note_is_channel_sixteen() {
        assert_eq!(loopback_note(), [0x9f, 0x7f, 0x5a]);
    }
}