//! Device protocol encodings shared by the Fireface backends.
//!
//! The UC and UCX look similar but are **not** interchangeable: the UC uses
//! EP0 vendor requests for the matrix, while the UCX uses IF1 register
//! words.  The two submodules therefore keep separate constants and
//! encodings even where the same dB curve happens to apply.
//!
//! Sources:
//! - `docs/PROTOCOL-COMPLETE.md` (this repository's consolidated map)
//! - agfline/RME-Fireface-UC-Drivers (UC facts)
//! - OroboroModular/rme-fireface-ucx-linux-driver (UCX facts)
//! No code from those projects is copied; their public protocol notes are
//! used as factual references and all uncertainties are listed in the docs.

use std::fmt;

pub const INPUTS: usize = 18;
pub const OUTPUTS: usize = 18;
pub const PLAYBACK: usize = 18;
pub const PAIRS: usize = 9;
pub const MATRIX_SOURCES: usize = INPUTS + PLAYBACK;

/// 0 dB in the UC's 14-bit linear mixer scale.
pub const UC_UNITY_LEVEL14: u32 = 0x8000;

/// UC vendor request numbers (also used by the UCX where they agree).
pub mod req {
    pub const SETTINGS: u8 = 0x10;
    pub const STATUS: u8 = 0x11;
    pub const MIXER: u8 = 0x12;
    pub const MUTE: u8 = 0x13;
    pub const UNMUTE: u8 = 0x14;
    pub const LOOPBACK: u8 = 0x15;
    pub const CHAN_INIT: u8 = 0x16;
    pub const LEVELS: u8 = 0x17;
    /// Input phase: signed coefficient plus a channel code.
    pub const PHASE: u8 = 0x18;
    pub const PGA: u8 = 0x1a;
    pub const DDS: u8 = 0x1b;
    /// IN: 32-bit firmware revision word; bits 31..16 are the revision.
    pub const REVISION: u8 = 0x1c;
    pub const ENABLE: u8 = 0x20;
    /// UCX Windows path: stream-start/stop support.
    pub const STREAM29: u8 = 0x1d;
    pub const SERIAL_LO: u8 = 0x1e;
    pub const SERIAL_HI: u8 = 0x1f;
}

/// Firmware revision carried in the upper 16 bits of the EP0 `0x1c` reply.
pub fn firmware_revision(word: u32) -> u16 {
    (word >> 16) as u16
}

/// A matrix source: a hardware input or a software playback channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Input(usize),
    Playback(usize),
}

impl Source {
    pub fn index(self) -> usize {
        match self {
            Source::Input(i) => i,
            Source::Playback(p) => p,
        }
    }
}

/// A decoded UCX meter packet kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterKind {
    InputsA,
    InputsB,
    Playback,
    OutputsPreFader,
    OutputsPostFader,
}

impl MeterKind {
    pub fn from_tag(tag: u32) -> Option<Self> {
        Some(match tag {
            0x1111_1111 => MeterKind::InputsA,
            0x5555_5555 => MeterKind::InputsB,
            0x2222_2222 => MeterKind::Playback,
            0x3333_3333 => MeterKind::OutputsPreFader,
            0x6666_6666 => MeterKind::OutputsPostFader,
            _ => return None,
        })
    }
}

/// A 244-byte UCX `0x85` meter packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeterPacket {
    pub kind: MeterKind,
    /// Mean-square per channel: 512 × (24-bit sample)²; full scale 2^55.
    pub power: [u64; 20],
    /// Peak per channel: 16 × |24-bit sample|; full scale 2^27.
    pub peak: [u32; 18],
    /// FX bus power: in and out.
    pub fx_power: [u64; 2],
    /// FX bus peak: in and out.
    pub fx_peak: [u32; 2],
}

impl MeterPacket {
    pub const LEN: usize = 244;

    pub fn parse(b: &[u8]) -> Result<Self, ProtocolError> {
        if b.len() != Self::LEN {
            return Err(ProtocolError::MeterLength(b.len()));
        }
        let tag = u32::from_le_bytes(b[240..244].try_into().unwrap());
        let kind = MeterKind::from_tag(tag)
            .ok_or(ProtocolError::MeterTag(tag))?;
        let mut power = [0u64; 20];
        for (i, p) in power.iter_mut().enumerate() {
            *p = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
        }
        let mut peak = [0u32; 18];
        for (i, p) in peak.iter_mut().enumerate() {
            *p = u32::from_le_bytes(b[160 + i * 4..164 + i * 4].try_into().unwrap());
        }
        let fx_power = [power[18], power[19]];
        let fx_peak = [
            u32::from_le_bytes(b[232..236].try_into().unwrap()),
            u32::from_le_bytes(b[236..240].try_into().unwrap()),
        ];
        Ok(Self {
            kind,
            power,
            peak,
            fx_power,
            fx_peak,
        })
    }
}

/// A 324-byte Fireface UC `0x85` meter page.
///
/// Byte 108 begins the 54 32-bit records in display order: 8 analog input,
/// 10 S/PDIF and ADAT input, 18 playback and 18 hardware output records.
/// A record uses the meter ABI of the FireWire generation: bits 8..31 are
/// the level, bits 0..7 the overload field.  A zero record means the
/// channel was below the meter range, and a page whose records read zero
/// everywhere is a filler page, not a meter page.
///
/// The data page header also carries the RMS power words for output
/// channels 4..17 (AN5..ADAT8), two per output pair.  The seven 16-byte
/// groups start at bytes 0, 16, 32, 48, 64, 80 and 96:
///
/// ```text
/// +0   left RMS power (u32)
/// +4   left status
/// +8   right RMS power (u32)
/// +12  right status
/// ```
///
/// The last group is truncated after its right RMS word at byte 104.
/// The flag page still carries output RMS for channels 0..3.
///
/// The stream alternates data pages with flag pages.  Word 1 is `0x1fff` on
/// a data page in the original page format and `0` or `1` on a flag page.
/// The `UC_WIN_RME` personality uses marker `0` on both page kinds, but its
/// data page sets word 6; flag pages leave it zero.
/// A flag page also carries digital input records, so its record region is
/// not entirely zero; its zero playback records must not be treated as
/// silence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UcMeterPacket {
    pub records: [u32; 54],
    /// Output RMS power for channels 4..17, two words per output pair.
    pub output_rms: [u32; UC_HEADER_OUTPUT_RMS_CHANNELS],
}

fn uc_word(b: &[u8], word: usize) -> u16 {
    u16::from_le_bytes(b[word * 2..word * 2 + 2].try_into().unwrap())
}

/// Whether a 324-byte `0x85` page is the Peak/OVR data page.
///
/// The original format marks it with `0x1fff` in word 1.  `UC_WIN_RME`
/// marks neither page there, so word 6 is the discriminator: the data page
/// always carries a non-zero status word there, while the flag page leaves
/// it zero.
fn uc_is_data_page(b: &[u8]) -> bool {
    let marker = u32::from_le_bytes(b[4..8].try_into().unwrap());
    marker == 0x1fff || uc_word(b, 6) != 0
}

impl Default for UcMeterPacket {
    fn default() -> Self {
        Self {
            records: [0; Self::RECORD_COUNT],
            output_rms: [0; UC_HEADER_OUTPUT_RMS_CHANNELS],
        }
    }
}

/// Full scale of the UC 16-bit linear meter words (analog inputs,
/// playback and hardware outputs).  The word is linear RMS, so a
/// -20 dBFS sine reads `0x0ccd` on this scale.
pub const UC_LINEAR_16_FULL_SCALE: f32 = 32768.0;
/// Full scale of the UC 24-bit linear digital-input word in bits 8..31.
pub const UC_LINEAR_24_FULL_SCALE: f32 = 8_388_608.0;
/// Digital-input records below this 24-bit level are page filler, not a
/// level.  The `0x00001fff` filler carries level `0x1f`; a real
/// -90 dBFS signal is about 265.
const UC_DIGITAL_INPUT_MIN_LEVEL: u32 = 0x100;
impl UcMeterPacket {
    pub const LEN: usize = 324;
    pub const ANALOG_INPUT_COUNT: usize = 8;
    pub const DIGITAL_INPUT_COUNT: usize = 10;
    pub const INPUT_COUNT: usize = Self::ANALOG_INPUT_COUNT + Self::DIGITAL_INPUT_COUNT;
    pub const PLAYBACK_COUNT: usize = 18;
    pub const OUTPUT_COUNT: usize = 18;
    pub const RECORD_COUNT: usize = Self::INPUT_COUNT + Self::PLAYBACK_COUNT + Self::OUTPUT_COUNT;

    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() != Self::LEN || b[108..Self::LEN].iter().all(|&byte| byte == 0) {
            return None;
        }
        // Only the data page carries Peak/OVR records.  The alternate flag
        // page carries the RMS words; applying its zero peak records would
        // release every meter once per data page.
        if !uc_is_data_page(b) {
            return None;
        }
        let mut records = [0u32; Self::RECORD_COUNT];
        for (i, r) in records.iter_mut().enumerate() {
            *r = u32::from_le_bytes(b[108 + i * 4..108 + i * 4 + 4].try_into().unwrap());
        }
        let mut output_rms = [0u32; UC_HEADER_OUTPUT_RMS_CHANNELS];
        for group in 0..7 {
            let left = group * 4;
            let right = left + 2;
            output_rms[group * 2] =
                u32::from_le_bytes(b[left * 4..left * 4 + 4].try_into().unwrap());
            output_rms[group * 2 + 1] =
                u32::from_le_bytes(b[right * 4..right * 4 + 4].try_into().unwrap());
        }
        Some(Self { records, output_rms })
    }

    /// Raw analog input record `AN1..AN8`.
    pub fn analog_input(&self, channel: usize) -> u32 {
        self.records.get(channel).copied().unwrap_or(0)
    }

    /// Raw record of a digital input channel (S/PDIF, ADAT).
    pub fn digital_input(&self, channel: usize) -> u32 {
        self.records
            .get(Self::ANALOG_INPUT_COUNT + channel)
            .copied()
            .unwrap_or(0)
    }

    /// Consecutive-over count from an analog input record's low halfword.
    /// The high byte is `0xff` while over; `0xff00` is a no-over filler.
    pub fn analog_input_over_run(&self, channel: usize) -> u8 {
        let ovr = self.analog_input(channel) as u16;
        if (ovr >> 8) as u8 == 0xff {
            (ovr & 0xff) as u8
        } else {
            0
        }
    }

    /// Whether an analog input record carries the over event past the
    /// global OVR sample gate.
    pub fn analog_input_ovr(&self, channel: usize) -> bool {
        self.analog_input_over_run(channel) >= UC_OVR_RUN_GATE
    }

    /// Full-scale-sample count from a digital input record.  The 24-bit
    /// level saturates at `0x7fffff`; the low nibble then carries the
    /// count (0..15), not a half-dB over value or a fixed tag.  The upper
    /// nibble is a separate status/overflow field whose packing is still
    /// being mapped; it is not part of this count.
    pub fn digital_input_over_run(&self, channel: usize) -> u8 {
        let record = self.digital_input(channel);
        if ((record >> 8) & 0xffffff) == 0x7fffff {
            record as u8 & 0x0f
        } else {
            0
        }
    }

    /// Whether a digital input record reaches the global OVR sample gate.
    /// The digital record has no separate `0xff` event byte; saturation of
    /// the 24-bit level is the event, and the low nibble is the count.
    pub fn digital_input_ovr(&self, channel: usize) -> bool {
        self.digital_input_over_run(channel) >= UC_OVR_RUN_GATE
    }

    /// Raw record of a playback channel.
    pub fn playback(&self, channel: usize) -> u32 {
        self.records
            .get(Self::INPUT_COUNT + channel)
            .copied()
            .unwrap_or(0)
    }

    /// Raw record of a hardware output channel.
    pub fn output(&self, channel: usize) -> u32 {
        self.records
            .get(Self::INPUT_COUNT + Self::PLAYBACK_COUNT + channel)
            .copied()
            .unwrap_or(0)
    }

    /// RMS power word for a hardware output channel in the data-page
    /// header.  The header covers output channels 4..17 only; channels
    /// 0..3 use the flag-page bank and return 0 here.
    pub fn output_header_rms(&self, channel: usize) -> u32 {
        if !(UC_OUTPUT_RMS_CHANNELS..Self::OUTPUT_COUNT).contains(&channel) {
            return 0;
        }
        self.output_rms[channel - UC_OUTPUT_RMS_CHANNELS]
    }

    /// Whether a playback channel carries the first word's OVR marker.
    ///
    /// The low half of the playback record is the channel's OVR word; its
    /// high byte is `0xff` when the device reports an over.  The low byte
    /// is status, not the configured sample count.
    pub fn playback_ovr(&self, channel: usize) -> bool {
        let ovr = self.playback(channel) & 0xffff;
        (ovr >> 8) as u8 == 0xff && (ovr & 0xff) != 0
    }

    /// Consecutive full-scale samples at or above 0 dBFS for a software
    /// playback channel, per the record's OVR word.  The device counts the
    /// run since the last sample below full scale; it saturates at 255
    /// (both nibbles) once the run passes the record's window and carries
    /// the count into `0x0f..0xff` for heavier over-drive.  `0` covers the
    /// `0xff00`/`0x0000` no-over fillers.  Verified against a sin/square
    /// staircase: the square at +0.05 dB over lights `0xff` (100%), the
    /// sine's measured runs 4/8/11/13/15 match `run = frac*cycle/2`.
    pub fn playback_over_run(&self, channel: usize) -> u8 {
        if !self.playback_ovr(channel) {
            return 0;
        }
        (self.playback(channel) & 0xff) as u8
    }

    /// Whether a hardware-output channel carries the first word's OVR
    /// marker.  The output bank uses the same layout as playback.
    pub fn output_ovr(&self, channel: usize) -> bool {
        let ovr = self.output(channel) & 0xffff;
        (ovr >> 8) as u8 == 0xff && (ovr & 0xff) != 0
    }

    /// Consecutive full-scale samples at or above 0 dBFS for one hardware
    /// output channel; same OVR-word semantics as `playback_over_run`.
    pub fn output_over_run(&self, channel: usize) -> u8 {
        if !self.output_ovr(channel) {
            return 0;
        }
        (self.output(channel) & 0xff) as u8
    }
}

/// The alternate UC `0x85` flag page, carrying RMS power words.
///
/// The page carries one 4-word slot per channel with the RMS power word at
/// slot offset 3.  The input bank occupies words 0..71, the playback bank
/// words 72..143 (channels 0..17); the output bank follows at words 144..161
/// but is truncated after output channels 0..3 (AN1..AN4), so their RMS
/// words sit at 147, 151, 155 and 159.  The page's word 1 is 0 or 1; word 6
/// stays zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UcRmsPacket {
    /// Input RMS bank: one power word per input at `3 + 4c` for input
    /// channels `0..17` (AN1..8, S/PDIF L/R, ADAT 1..8).
    pub input: [u16; 18],
    pub playback: [u16; 18],
    /// RMS bank for hardware output channels 0..3 (AN1..AN4); the page's
    /// word budget ends before output channel 4's own slot.
    pub output: [u16; 4],
}

/// Output channel count covered by the flag page's output RMS bank.
pub const UC_OUTPUT_RMS_CHANNELS: usize = 4;
/// Output channel count covered by the data-page header RMS bank
/// (AN5..ADAT8; output channels 4..17).
pub const UC_HEADER_OUTPUT_RMS_CHANNELS: usize = 14;

impl UcRmsPacket {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() != UcMeterPacket::LEN {
            return None;
        }
        if uc_is_data_page(b) {
            return None;
        }
        // A quiet flag page can be all zero; there is simply no RMS update
        // to apply.  The input bank occupies words 0..71, the playback bank
        // words 72..143 and the output bank words 144..161.
        let any_input = (0..18).any(|channel| uc_word(b, 3 + channel * 4) != 0);
        if !any_input && (72..=161).all(|word| uc_word(b, word) == 0) {
            return None;
        }
        let mut input = [0u16; 18];
        for channel in 0..18 {
            let word = 3 + channel * 4;
            input[channel] = u16::from_le_bytes(
                b[word * 2..word * 2 + 2].try_into().unwrap(),
            );
        }
        let mut playback = [0u16; 18];
        for pair in 0..9 {
            let left = 72 + pair * 8 + 3;
            let right = 72 + pair * 8 + 7;
            playback[pair * 2] = u16::from_le_bytes(
                b[left * 2..left * 2 + 2].try_into().unwrap(),
            );
            playback[pair * 2 + 1] = u16::from_le_bytes(
                b[right * 2..right * 2 + 2].try_into().unwrap(),
            );
        }
        let mut output = [0u16; UC_OUTPUT_RMS_CHANNELS];
        for channel in 0..UC_OUTPUT_RMS_CHANNELS {
            let word = 147 + channel * 4;
            output[channel] = u16::from_le_bytes(
                b[word * 2..word * 2 + 2].try_into().unwrap(),
            );
        }
        Some(Self {
            input,
            playback,
            output,
        })
    }

    /// RMS power word of an input channel.
    pub fn input(&self, channel: usize) -> u16 {
        self.input.get(channel).copied().unwrap_or(0)
    }

    pub fn playback(&self, channel: usize) -> u16 {
        self.playback.get(channel).copied().unwrap_or(0)
    }

    /// RMS power word of a hardware output channel; channels beyond the
    /// page's truncated bank read 0 (no RMS published).
    pub fn output(&self, channel: usize) -> u16 {
        self.output.get(channel).copied().unwrap_or(0)
    }
}

/// dBFS of a UC digital input meter record (S/PDIF, ADAT).
///
/// The level is a linear 24-bit peak value in bits 8..31; `0x800000` is
/// full scale.  The low byte is a full-scale-sample count and is not part
/// of the level.  A level below the meter's own floor is page filler
/// (`0x1fff`) and returns `None` so the meter bank holds the previous
/// value.
pub fn uc_digital_dbfs(record: u32) -> Option<f32> {
    let level = (record >> 8) & 0xffffff;
    if level == 0 {
        return Some(f32::NEG_INFINITY);
    }
    if level < UC_DIGITAL_INPUT_MIN_LEVEL {
        return None;
    }
    Some(20.0 * (level as f32 / UC_LINEAR_24_FULL_SCALE).log10())
}

/// dBFS of a UC software playback meter record.
///
/// A playback record carries the channel's 16-bit linear peak in the upper
/// halfword; `0x8000` is full scale.  The lower halfword belongs to the
/// separate OVR field and is not part of this peak.
pub fn uc_playback_dbfs(record: u32) -> Option<f32> {
    let level = record >> 16;
    if level == 0 {
        return Some(f32::NEG_INFINITY);
    }
    Some(20.0 * (level as f32 / 32768.0).log10())
}

/// Linear RMS amplitude of a UC flag-page playback RMS word.
///
/// The flag-page word is linear power relative to full-scale peak squared:
/// `rms = sqrt(word / 32768)`.  A full-scale sine has word 16384 and reads
/// `sqrt(0.5)`, i.e. -3 dBFS RMS.
pub fn uc_playback_rms_linear(word: u16) -> f32 {
    (word as f32 / 32768.0).max(0.0).sqrt()
}

/// Linear RMS amplitude of a data-page header output RMS word.
///
/// The header word is 32-bit linear power with `0x80000000` = full-scale
/// peak squared.  A full-scale sine has word `0x40000000` and reads
/// `sqrt(0.5)`, i.e. -3 dBFS RMS.
pub fn uc_output_header_rms_linear(word: u32) -> f32 {
    (word as f64 / 0x8000_0000u64 as f64).max(0.0).sqrt() as f32
}

/// Verified over-band law, OVR counter run -> peak-over-dBFS.
///
/// The record's OVR word counts the samples at or above full scale within
/// the record's own update window.  Measured staircases (sine, 440 Hz, the
/// Phones pair; `docs/work/UC-METER-DECODE.md`) give these run/over pairs,
/// and a record whose count steps between the pairs follows this law by
/// interpolation; a sustained pulse train (every sample over) drives both
/// nibbles to `0xff`.  A run below 4 is inside the over gate and a byte
/// wider than the 4-bit value domain carries the count past 15, where the
/// device no longer resolves the over level.
pub const UC_OVER_RUN_TABLE: [(u8, f32); 5] =
    [(4, 0.05), (8, 0.25), (11, 0.50), (13, 0.75), (15, 1.0)];

/// Peak-over-dBFS of a record's OVR count under the verified law.  `None`
/// where the count does not describe an over value it can resolve: under
/// the lowest table point, or in the counter's carry state.
pub fn uc_counter_over_db(run: u8) -> Option<f32> {
    if run < UC_OVER_RUN_TABLE[0].0 || run > 15 {
        return None;
    }
    let last = (run == 15).then(|| UC_OVER_RUN_TABLE[4].1);
    if let Some(db) = last {
        return Some(db);
    }
    for pair in UC_OVER_RUN_TABLE.windows(2) {
        let (r0, db0) = pair[0];
        let (r1, db1) = pair[1];
        if run < r1 {
            let t = (run - r0) as f32 / (r1 - r0) as f32;
            return Some(db0 + t * (db1 - db0));
        }
    }
    None
}

/// The device's OVR count that lights the OVR LED: TotalMix's global
/// "Full scale samples for OVR" preference default.  The host settings
/// expose the rest of the 1..10 range.
pub const UC_OVR_RUN_GATE: u8 = 3;

/// dBFS of a UC analog input record.
///
/// The level is a linear 16-bit peak word in the upper halfword; `0x8000`
/// is full scale and the lower halfword is packet noise.  A captured AP
/// loopback of a -20 dBFS tone read `0x21d9` on AN5, the same linear scale
/// as playback and output, with the input reference (`-10 dBV`) adding its
/// gain.
pub fn uc_analog_input_dbfs(record: u32) -> Option<f32> {
    uc_playback_dbfs(record)
}

pub fn power_dbfs(power: u64) -> f32 {
    if power <= 31 {
        return f32::NEG_INFINITY;
    }
    10.0 * (power as f64 / (1u64 << 55) as f64).log10() as f32
}

pub fn peak_dbfs(peak: u32) -> f32 {
    if peak == 0 {
        return f32::NEG_INFINITY;
    }
    20.0 * (peak as f64 / (1u64 << 27) as f64).log10() as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    MeterLength(usize),
    MeterTag(u32),
    BadRange(&'static str),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::MeterLength(n) => write!(f, "meter packet is {n} bytes, expected 244"),
            ProtocolError::MeterTag(t) => write!(f, "unknown meter tag {t:#010x}"),
            ProtocolError::BadRange(s) => write!(f, "value out of range: {s}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub mod uc {
    //! Fireface UC: EP0 vendor requests, 14-bit linear matrix gains.
    use super::{Source, UC_UNITY_LEVEL14};

    pub const MIX_STRIDE: u16 = 36;
    pub const MIX_MASTER: u16 = 992;
    pub const PLAYBACK_BASE: u16 = 18;
    /// The UC gain word is 14-bit: `level14 = wValue << 2 | (wIndex >> 14) & 3`.
    /// The ALSA/GUI value is the 12-bit `wValue` part, so its full scale is
    /// 0 dB (0x2000) and the low two level bits stay zero.
    pub const UNITY_WVALUE: u16 = (UC_UNITY_LEVEL14 >> 2) as u16;

    /// 16 output blocks in the matrix are in TotalMix *display* order
    /// (AN1/2 first, Phones last); the hardware-volume array is Phones-first.
    /// This maps an internal (Phones-first) block index to the matrix column.
    pub fn matrix_block(internal_block: usize, output_order: &[usize]) -> usize {
        let pair = internal_block / 2;
        let side = internal_block & 1;
        output_order
            .iter()
            .position(|&p| p == pair)
            .unwrap_or(pair)
            * 2
            + side
    }

    /// Internal (Phones-first) output channel -> UC encoder index.
    pub fn encoder_channel(internal_idx: usize, output_order: &[usize]) -> Option<u8> {
        let pair = internal_idx / 2;
        let side = internal_idx & 1;
        let slot = output_order.iter().position(|&p| p == pair)?;
        Some((4 + 2 * slot + side) as u8)
    }

    /// UC encoder index -> internal (Phones-first) output channel.
    pub fn output_from_encoder(channel: u8, output_order: &[usize]) -> Option<usize> {
        if !(4..=21).contains(&channel) {
            return None;
        }
        let slot = ((channel - 4) / 2) as usize;
        let side = ((channel - 4) & 1) as usize;
        Some(output_order.get(slot)? * 2 + side)
    }

    /// UC output attenuator code -> linear gain.  Raw 0 = +6 dB,
    /// 6 = 0 dB, 63 = about −57 dB.
    pub fn output_linear(raw: u8) -> f32 {
        let db = 6.0 - raw.min(63) as f32;
        10f32.powf(db / 20.0)
    }

    /// Linear 0..2 (+6 dB) -> (`wValue`, matrix address) for one crosspoint.
    pub fn matrix_word(linear: f32, address: u16) -> (u16, u16) {
        // The two level LSBs that would be encoded in the address's top bits
        // overlap the four submix banks; keep them clear and stay within
        // 0.0002 dB of the requested gain.
        let level14 = (linear.clamp(0.0, 2.0) * UC_UNITY_LEVEL14 as f32)
            .round()
            .clamp(0.0, 65532.0) as u32;
        let level14 = level14 & !3;
        let value = (level14 >> 2) as u16;
        let index = address & 0x3fff;
        (value, index)
    }

    /// Linear gain -> UC output attenuator code (0 = +6 dB, 6 = 0 dB,
    /// 63 = −57 dB).  The UC step is 1 dB.
    pub fn output_attenuation(linear: f32) -> u8 {
        let db = if linear <= 0.0 {
            -65.0
        } else {
            20.0 * linear.clamp(0.0, 2.0).log10()
        };
        (6.0 - db).round().clamp(0.0, 63.0) as u8
    }

    pub fn word_for_source(source: Source) -> u16 {
        let base = match source {
            Source::Input(i) => i as u16,
            Source::Playback(p) => PLAYBACK_BASE + p as u16,
        };
        base
    }

    pub fn matrix_address(out: usize, source: Source) -> u16 {
        out as u16 * MIX_STRIDE + word_for_source(source)
    }

    pub fn master_address(out: usize) -> u16 {
        MIX_MASTER + out as u16
    }

    /// The UC matrix has no readback; the GUI value is a linear model.
    pub fn linear_from_wvalue(wvalue: u16) -> f32 {
        wvalue as f32 / UNITY_WVALUE as f32
    }
}

pub mod ucx {
    //! Fireface UCX: IF1 register words.  See `docs/PROTOCOL-COMPLETE.md`.
    use super::Source;

    pub const CHANNELS: usize = 18;
    /// DSP channel number: inputs 0x00..0x11, outputs 0x12..0x23.
    pub const OUTPUT_DSP_BASE: u8 = 0x12;
    pub const MATRIX_BASE: u16 = 0x4000;
    pub const MATRIX_STRIDE: u16 = 0x40;
    pub const PLAYBACK_SOURCE_BASE: u16 = 0x20;
    pub const FX_BUS: [u8; 2] = [30, 31];

    /// Offsets inside a 256-register channel block.
    pub mod offset {
        pub const MUTE_OR_FADER: u8 = 0x00;
        pub const FX_SEND_OR_BALANCE: u8 = 0x01;
        /// Input block: stereo/mono mode.  Output block: mute.
        pub const INPUT_STEREO: u8 = 0x02;
        pub const OUTPUT_MUTE: u8 = 0x02;
        pub const FX_RETURN: u8 = 0x03;
        /// Output block: stereo/mono mode.
        pub const OUTPUT_STEREO: u8 = 0x04;
        pub const MS_PROC: u8 = 0x05;
        pub const INPUT_PHASE: u8 = 0x06;
        pub const GAIN_REFLEVEL_OR_PHASE: u8 = 0x07;
        /// Input block: 48 V (1/2) or Level (3/4).  Output block: Level.
        pub const INPUT_LEVEL_OR_48V: u8 = 0x08;
        pub const OUTPUT_REF_LEVEL: u8 = 0x08;
        pub const HI_Z: u8 = 0x09;
        pub const AUTOSET: u8 = 0x0a;

        pub const LOW_CUT_ON: u8 = 0x20;
        pub const LOW_CUT_FREQ: u8 = 0x21;
        pub const LOW_CUT_SLOPE: u8 = 0x22;

        pub const EQ_ON: u8 = 0x40;
        pub const EQ1_TYPE: u8 = 0x41;
        pub const EQ3_TYPE: u8 = 0x48;
        pub const DYN_ON: u8 = 0x60;
        pub const DYN_GAIN: u8 = 0x61;
        pub const DYN_ATTACK: u8 = 0x62;
        pub const DYN_RELEASE: u8 = 0x63;
        pub const COMP_THRESHOLD: u8 = 0x64;
        pub const COMP_RATIO: u8 = 0x65;
        pub const EXP_THRESHOLD: u8 = 0x66;
        pub const EXP_RATIO: u8 = 0x67;

        pub const AUTO_LEVEL_ON: u8 = 0x80;
        pub const AUTO_LEVEL_MAX_GAIN: u8 = 0x81;
        pub const AUTO_LEVEL_HEADROOM: u8 = 0x82;
        pub const AUTO_LEVEL_RISE_TIME: u8 = 0x83;

        pub const fn eq_band(band: usize) -> (u8, u8, u8) {
            match band {
                0 => (0x42, 0x43, 0x44),
                1 => (0x45, 0x46, 0x47),
                _ => (0x49, 0x4a, 0x4b),
            }
        }
    }

    pub mod global {
        pub const MUTE_ENABLE: u16 = 0x3d01;
        pub const DIM: u16 = 0x3d03;
        pub const MONITOR_MASK: u16 = 0x3f98;
        pub const MONITOR_FLAGS: u16 = 0x3f9e;
        /// Vendor-driver IF1 heartbeat.  The unit only streams live state
        /// deltas (including the front-panel Phones volume) while this is
        /// being refreshed.
        pub const HEARTBEAT: u16 = 0x3f00;
    }

    pub const MINUS_INF_TENTHS: i16 = -650;

    pub fn channel_reg(dsp_channel: u8, offset: u8) -> u16 {
        ((dsp_channel as u16) << 8) | offset as u16
    }

    pub fn input_channel(input: usize) -> u8 {
        input as u8
    }

    pub fn output_channel(dsp_output: usize) -> u8 {
        OUTPUT_DSP_BASE + dsp_output as u8
    }

    /// TotalControl output pairs are Phones-first; the UCX DSP outputs are
    /// AN1/2, AN3/4, AN5/6, PH7/8, SPDIF, AS1/2, ADAT3/4….
    pub fn output_pair_dsp(pair: usize) -> Option<usize> {
        Some(match pair {
            0 => 6,  // Phones 7/8
            1 => 0,  // AN 1/2
            2 => 2,  // AN 3/4
            3 => 4,  // AN 5/6
            4 => 8,  // SPDIF
            5 => 10, // AS 1/2 (ADAT 1/2)
            6 => 12, // ADAT 3/4
            7 => 14, // ADAT 5/6
            8 => 16, // ADAT 7/8
            _ => return None,
        })
    }

    /// Map an IF1 input-gain state address (`0x0007`, `0x0107`,
    /// `0x0207`, `0x0307`) to the input index.
    pub fn input_gain_index(addr: u16) -> Option<usize> {
        if addr & 0xff != offset::GAIN_REFLEVEL_OR_PHASE as u16 {
            return None;
        }
        let channel = (addr >> 8) as usize;
        (channel < 4).then_some(channel)
    }

    /// Map an IF1 output-volume state address (channel 0x12..0x23, offset
    /// 0) to TotalControl's Phones-first output-channel index.
    pub fn output_index_from_state(addr: u16) -> Option<usize> {
        if addr as u8 != 0 {
            return None;
        }
        let dsp = (addr >> 8).checked_sub(OUTPUT_DSP_BASE as u16)? as usize;
        if dsp >= CHANNELS {
            return None;
        }
        let dsp_pair = dsp / 2;
        let side = dsp & 1;
        let pair = match dsp_pair {
            3 => 0,  // Phones 7/8
            0 => 1,  // AN1/2
            1 => 2,  // AN3/4
            2 => 3,  // AN5/6
            4 => 4,  // SPDIF
            5 => 5,  // AS 1/2
            6 => 6,  // ADAT 3/4
            7 => 7,  // ADAT 5/6
            8 => 8,  // ADAT 7/8
            _ => return None,
        };
        Some(pair * 2 + side)
    }

    pub fn source_index(source: Source) -> u16 {
        match source {
            Source::Input(i) => i as u16,
            Source::Playback(p) => PLAYBACK_SOURCE_BASE + p as u16,
        }
    }

    pub fn matrix_reg(dsp_output: u8, source: Source) -> u16 {
        MATRIX_BASE + dsp_output as u16 * MATRIX_STRIDE + source_index(source)
    }

    /// The unit's own fader/pan record for hardware input `input` in output
    /// `output`'s submix (two fields on the same address).
    pub fn fader_table_reg(dsp_output: u8, input: usize) -> u16 {
        channel_reg(output_channel(dsp_output as usize),
                    0xe0 + input as u8)
    }

    /// A matrix coefficient: bit 15 = coarse scale, bits 14..0 = signed.
    pub fn matrix_gain(linear: f64) -> u16 {
        let fine = (linear * 32768.0).round();
        if (-16384.0..=16383.0).contains(&fine) {
            return (fine as i16 as u16) & 0x7fff;
        }
        let coarse = (linear * 4096.0).round().clamp(-16384.0, 16383.0);
        0x8000 | ((coarse as i16 as u16) & 0x7fff)
    }

    pub fn matrix_gain_db(db: f64) -> u16 {
        if db.is_nan() || db == f64::NEG_INFINITY {
            return 0;
        }
        matrix_gain(10f64.powf(db / 20.0))
    }

    pub fn matrix_linear(raw: u16) -> f64 {
        let n = (raw & 0x7fff) as i32;
        let signed = if n & 0x4000 != 0 { n - 0x8000 } else { n };
        signed as f64 / if raw & 0x8000 != 0 { 4096.0 } else { 32768.0 }
    }

    pub fn matrix_db(raw: u16) -> f64 {
        20.0 * matrix_linear(raw).abs().log10()
    }

    pub fn db_to_tenths(db: f32) -> u16 {
        if db.is_nan() || db <= MINUS_INF_TENTHS as f32 / 10.0 {
            return MINUS_INF_TENTHS as u16;
        }
        ((db * 10.0).round() as i16) as u16
    }

    pub fn tenths_to_db(v: u16) -> f32 {
        let t = v as i16;
        if t <= MINUS_INF_TENTHS {
            f32::NEG_INFINITY
        } else {
            t as f32 / 10.0
        }
    }

    pub fn linear_to_tenths(linear: f32) -> u16 {
        if linear <= 0.0 {
            MINUS_INF_TENTHS as u16
        } else {
            db_to_tenths(20.0 * linear.log10())
        }
    }

    pub fn tenths_to_linear(tenths: u16) -> f32 {
        let db = tenths_to_db(tenths);
        if db == f32::NEG_INFINITY {
            0.0
        } else {
            10f32.powf(db / 20.0)
        }
    }

    /// Fader/pan table field (same address, bit 3 selects pan).
    pub fn fader_level(tenths: i16) -> u16 {
        (tenths.clamp(MINUS_INF_TENTHS, 60) << 4) as u16
    }

    pub fn fader_pan(pan: i16) -> u16 {
        (((pan.clamp(-100, 100) << 4) as u16) | 0x8) as u16
    }

    /// Clock/options control register (request 16, masked write).
    pub fn control_word(clock: u8, word_termination: bool,
                        word_single_speed: bool, spdif_professional: bool,
                        eq_dyn_for_record: bool, optical_out_spdif: bool) -> u16 {
        let mut v = ((clock & 3) as u16) << 1;
        if word_termination {
            v |= 0x0008;
        }
        if word_single_speed {
            v |= 0x0040;
        }
        if spdif_professional {
            v |= 0x0080;
        }
        if !eq_dyn_for_record {
            v |= 0x0100;
        }
        if optical_out_spdif {
            v |= 0x0400;
        }
        v
    }

    pub const CONTROL_MASK: u16 = 0x05cf;
    pub const RATE_FAMILY_MASK: u16 = 0x0030;

    pub fn rate_family(rate: u32) -> u16 {
        match rate {
            32000 | 64000 | 128000 => 0,
            44100 | 88200 | 176400 => 1,
            _ => 2,
        }
    }

    pub fn playback_mask(channels: usize) -> (u16, u16) {
        let mask = ((1u32 << channels.min(30)) - 1) << 2;
        ((mask >> 16) as u16, mask as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ucx_matrix_values_match_captures() {
        assert_eq!(ucx::matrix_gain(1.0), 0x9000);
        assert_eq!(ucx::matrix_gain(0.5), 0x8800);
        assert_eq!(ucx::matrix_gain(-0.5), 0x4000);
        assert_eq!(ucx::matrix_gain(-1.0), 0xf000);
        assert!((ucx::matrix_db(0x258f) + 10.65).abs() < 0.01);
        assert!((ucx::matrix_linear(0x258f) - 0.2934).abs() < 1e-4);
    }

    #[test]
    fn ucx_addresses_match_captures() {
        assert_eq!(ucx::matrix_reg(2, Source::Input(0)), 0x4080);
        assert_eq!(ucx::matrix_reg(2, Source::Playback(0)), 0x40a0);
        assert_eq!(ucx::matrix_reg(3, Source::Playback(1)), 0x40e1);
        assert_eq!(ucx::fader_table_reg(0, 6), 0x12e6);
        assert_eq!(ucx::fader_table_reg(3, 1), 0x15e1);
    }

    #[test]
    fn ucx_fader_fields_match_captures() {
        assert_eq!(ucx::fader_level(-106), 0xf960);
        assert_eq!(ucx::fader_level(-650), 0xd760);
        assert_eq!(ucx::fader_pan(-100), 0xf9c8);
        assert_eq!(ucx::fader_pan(100), 0x0648);
        assert_eq!(ucx::fader_pan(0), 0x0008);
    }

    #[test]
    fn uc_matrix_word_round_trip() {
        let (value, index) = uc::matrix_word(1.0, 36 * 3 + 18);
        assert_eq!(value, 0x2000);
        assert_eq!(index, 126);
        assert!((uc::linear_from_wvalue(value) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn uc_matrix_word_keeps_the_cell_address_and_14_bit_range() {
        let address = 36 * 3 + 18;
        for linear in [0.1, 0.25, 0.5, 0.707, 1.0, 1.5, 2.0] {
            let (value, index) = uc::matrix_word(linear, address);
            assert_eq!(index, address, "gain {linear} moved the cell");
            assert_eq!(index & 0xc000, 0, "gain {linear} set a submix-bank bit");
            assert!(value <= 0x3fff, "gain {linear} exceeded the 14-bit level");
            let measured = uc::linear_from_wvalue(value);
            assert!((measured - linear).abs() < 0.001, "gain {linear} -> {measured}");
        }
        // Fractional gains must not leak their level LSBs into wIndex bits
        // 14:15.  Captured Phones cell 6/18 is a plain index 234.
        let (_, index) = uc::matrix_word(0.80082595, 6 * 36 + 18);
        assert_eq!(index, 234);
    }

    #[test]
    fn uc_encoder_and_internal_output_are_inverse() {
        let order = [1, 2, 3, 0, 4, 5, 6, 7, 8];
        for internal in 0..18 {
            let enc = uc::encoder_channel(internal, &order).unwrap();
            assert_eq!(uc::output_from_encoder(enc, &order), Some(internal));
        }
        assert_eq!(uc::output_from_encoder(3, &order), None);
        assert_eq!(uc::output_from_encoder(22, &order), None);
    }

    #[test]
    fn uc_output_attenuator_code_and_linear_round_trip() {
        for raw in 0..=63u8 {
            let linear = uc::output_linear(raw);
            assert_eq!(uc::output_attenuation(linear), raw);
        }
        assert!((uc::output_linear(0) - 1.9953).abs() < 0.001);
        assert!((uc::output_linear(6) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ucx_output_pair_order() {
        assert_eq!(ucx::output_pair_dsp(0), Some(6));
        assert_eq!(ucx::output_pair_dsp(1), Some(0));
        assert_eq!(ucx::output_pair_dsp(8), Some(16));
    }

    #[test]
    fn ucx_output_state_map_inverts_the_pair_map() {
        for pair in 0..9 {
            let dsp = ucx::output_pair_dsp(pair).unwrap();
            for side in 0..2 {
                let addr = ucx::channel_reg(ucx::output_channel(dsp + side), 0);
                assert_eq!(
                    ucx::output_index_from_state(addr),
                    Some(pair * 2 + side),
                    "pair {pair} side {side}"
                );
            }
        }
    }

    #[test]
    fn meter_packet_parses() {
        let mut b = [0u8; 244];
        b[0..8].copy_from_slice(&(512u64 * 100 * 100).to_le_bytes());
        b[144..152].copy_from_slice(&(512u64 * 40 * 40).to_le_bytes());
        b[152..160].copy_from_slice(&(512u64 * 30 * 30).to_le_bytes());
        b[160..164].copy_from_slice(&(16u32 * 100).to_le_bytes());
        b[232..236].copy_from_slice(&(16u32 * 40).to_le_bytes());
        b[236..240].copy_from_slice(&(16u32 * 30).to_le_bytes());
        b[240..244].copy_from_slice(&0x1111_1111u32.to_le_bytes());
        let p = MeterPacket::parse(&b).unwrap();
        assert_eq!(p.kind, MeterKind::InputsA);
        assert_eq!(p.power[0], 512 * 10_000);
        assert_eq!(p.peak[0], 1600);
        assert_eq!(p.fx_power, [512 * 1_600, 512 * 900]);
        assert_eq!(p.fx_peak, [640, 480]);
    }

    #[test]
    fn ep0_revision_uses_the_upper_word() {
        assert_eq!(firmware_revision(0x002e_0103), 46);
    }

    #[test]
    fn uc_meter_records_map_display_order() {
        let mut b = [0u8; UcMeterPacket::LEN];
        b[4..8].copy_from_slice(&0x1fffu32.to_le_bytes());
        b[108..112].copy_from_slice(&0x01000000u32.to_le_bytes());
        b[140..144].copy_from_slice(&0x02000000u32.to_le_bytes());
        b[180..184].copy_from_slice(&0x03000000u32.to_le_bytes());
        b[252..256].copy_from_slice(&0x04000000u32.to_le_bytes());
        let p = UcMeterPacket::parse(&b).unwrap();
        assert_eq!(p.analog_input(0), 0x01000000, "byte 108 is AN1");
        assert_eq!(
            p.digital_input(0),
            0x02000000,
            "byte 140 is the first digital input"
        );
        assert_eq!(p.playback(0), 0x03000000, "byte 180 is playback 1");
        assert_eq!(p.output(0), 0x04000000, "byte 252 is output AN1");
        assert!(!p.playback_ovr(0));
        assert!(UcMeterPacket::parse(&[0u8; UcMeterPacket::LEN]).is_none());
    }

    #[test]
    fn uc_flag_page_carries_playback_rms_power() {
        let mut b = [0u8; UcMeterPacket::LEN];
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[150..152].copy_from_slice(&16384u16.to_le_bytes());
        b[158..160].copy_from_slice(&1u16.to_le_bytes());
        let p = UcRmsPacket::parse(&b).expect("flag page");
        assert_eq!(p.playback(0), 16384);
        assert_eq!(p.playback(1), 1);
        assert!((uc_playback_rms_linear(p.playback(0)) - 0.7071).abs() < 0.001);
        assert!(UcRmsPacket::parse(&[0u8; UcMeterPacket::LEN]).is_none());

        // Every playback pair occupies the same eight-word stride; marker
        // 0 is a valid flag-page marker.
        let mut all = [0u8; UcMeterPacket::LEN];
        all[4..8].copy_from_slice(&0u32.to_le_bytes());
        for pair in 0..9 {
            let left = 75 + 8 * pair;
            all[left * 2..left * 2 + 2]
                .copy_from_slice(&(pair as u16 + 1).to_le_bytes());
            let right = 79 + 8 * pair;
            all[right * 2..right * 2 + 2]
                .copy_from_slice(&(pair as u16 + 101).to_le_bytes());
        }
        assert!(UcMeterPacket::parse(&all).is_none());
        let all = UcRmsPacket::parse(&all).expect("flag page marker 0");
        for pair in 0..9 {
            assert_eq!(all.playback(pair * 2), pair as u16 + 1);
            assert_eq!(all.playback(pair * 2 + 1), pair as u16 + 101);
        }
    }

    #[test]
    fn uc_flag_page_carries_input_rms_power() {
        let mut b = [0u8; UcMeterPacket::LEN];
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        // AN5 (c=4) and S/PDIF L (c=8) live at 3 + 4c.
        b[(3 + 4 * 4) * 2..(3 + 4 * 4) * 2 + 2]
            .copy_from_slice(&1098u16.to_le_bytes());
        b[(3 + 4 * 8) * 2..(3 + 4 * 8) * 2 + 2]
            .copy_from_slice(&159u16.to_le_bytes());
        let p = UcRmsPacket::parse(&b).expect("flag page");
        assert_eq!(p.input(4), 1098);
        assert_eq!(p.input(8), 159);
        assert_eq!(p.input(5), 0);
        assert!((uc_playback_rms_linear(p.input(4)) - 0.1831).abs() < 0.001);

        // A page with only an input RMS word still parses.
        let mut only = [0u8; UcMeterPacket::LEN];
        only[4..8].copy_from_slice(&1u32.to_le_bytes());
        only[(3 + 4 * 0) * 2..(3 + 4 * 0) * 2 + 2]
            .copy_from_slice(&1u16.to_le_bytes());
        assert!(UcRmsPacket::parse(&only).is_some());
    }

    #[test]
    fn uc_input_ovr_records() {
        let mut p = UcMeterPacket::default();
        // Analog AN1: low halfword is the OVR word, high halfword 0x7fff.
        p.records[0] = 0x7fff_ff04;
        assert_eq!(p.analog_input_over_run(0), 4);
        assert!(p.analog_input_ovr(0));
        p.records[0] = 0x7fff_ff00;
        assert_eq!(p.analog_input_over_run(0), 0);
        assert!(!p.analog_input_ovr(0));
        p.records[0] = 0x7fff_1234;
        assert_eq!(p.analog_input_over_run(0), 0);
        // Digital S/PDIF L: the low nibble is the full-scale-sample count
        // when the 24-bit level saturates.  The upper nibble is not part
        // of the count and must not inflate the gate.
        p.records[8] = 0x7fffffcf;
        assert_eq!(p.digital_input_over_run(0), 0x0f);
        assert!(p.digital_input_ovr(0));
        p.records[8] = 0x7fffff05;
        assert_eq!(p.digital_input_over_run(0), 5);
        assert!(p.digital_input_ovr(0));
        p.records[8] = 0x7fffff67;
        assert_eq!(p.digital_input_over_run(0), 7);
        assert!(p.digital_input_ovr(0));
        p.records[8] = 0x7fffff02;
        assert!(!p.digital_input_ovr(0), "count 2 is under the gate");
        p.records[8] = 0x0ccd0000;
        assert!(!p.digital_input_ovr(0));
        p.records[8] = 0x7fffff00;
        assert!(!p.digital_input_ovr(0));
    }

    #[test]
    fn uc_win_rme_data_page_uses_word_six_and_is_not_rms() {
        // UC_WIN_RME marks neither page with 0x1fff.  Its data page sets
        // word 6 and carries the Peak/OVR records; its flag page leaves
        // word 6 zero.  A data page read as RMS would turn playback 1's
        // peak at word 91 into fake RMS on AN5/6 and Main's peak at word
        // 139 into fake RMS on ADAT7/8.
        let mut b = [0u8; UcMeterPacket::LEN];
        b[4..8].copy_from_slice(&0u32.to_le_bytes());
        b[6 * 2..6 * 2 + 2].copy_from_slice(&8191u16.to_le_bytes());
        b[91 * 2..91 * 2 + 2].copy_from_slice(&12000u16.to_le_bytes());
        b[139 * 2..139 * 2 + 2].copy_from_slice(&80u16.to_le_bytes());
        b[180..184].copy_from_slice(&0x3fff0000u32.to_le_bytes());

        let data = UcMeterPacket::parse(&b).expect("data page marker 0");
        assert_eq!(data.playback(0) >> 16, 0x3fff);
        assert!(UcRmsPacket::parse(&b).is_none(), "data page is not RMS");

        // The same marker and word 6 zero is the flag page.
        b[6 * 2..6 * 2 + 2].fill(0);
        b[75 * 2..75 * 2 + 2].copy_from_slice(&16384u16.to_le_bytes());
        assert!(UcMeterPacket::parse(&b).is_none());
        assert!(UcRmsPacket::parse(&b).is_some(), "flag page carries RMS");
    }

    #[test]
    fn uc_playback_ovr_is_the_first_word_high_byte() {
        let mut b = [0u8; UcMeterPacket::LEN];
        b[4..8].copy_from_slice(&0x1fffu32.to_le_bytes());
        b[180..184].copy_from_slice(&0x7fffff03u32.to_le_bytes());
        let p = UcMeterPacket::parse(&b).unwrap();
        assert!(p.playback_ovr(0));
        assert!(!p.playback_ovr(1));

        // 0xff00 is the no-over filler, not an active OVR.
        b[180..184].copy_from_slice(&0x7fffff00u32.to_le_bytes());
        let p = UcMeterPacket::parse(&b).unwrap();
        assert!(!p.playback_ovr(0));
    }

    #[test]
    fn uc_meter_flag_page_is_not_a_meter_page() {
        let mut b = [0u8; UcMeterPacket::LEN];
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        // The flag page carries live digital input records but no playback
        // or output levels.
        b[140..144].copy_from_slice(&0x03000000u32.to_le_bytes());
        assert!(UcMeterPacket::parse(&b).is_none());

        b[4..8].copy_from_slice(&0x1fffu32.to_le_bytes());
        assert!(UcMeterPacket::parse(&b).is_some());
    }

    #[test]
    fn uc_playback_records_decode_the_peak_word() {
        // Captured with a steady 1 kHz playback stream: the upper halfword
        // is the 16-bit peak; the lower halfword is the separate OVR field.
        assert!((uc_playback_dbfs(0x3fff0000).unwrap() + 6.0206).abs() < 0.01);
        assert!((uc_playback_dbfs(0x1470000).unwrap() + 40.0).abs() < 0.1);
        assert!((uc_playback_dbfs(0x0cce0000).unwrap() + 20.0).abs() < 0.1);
        assert_eq!(uc_playback_dbfs(0), Some(f32::NEG_INFINITY));
    }

    #[test]
    fn uc_analog_input_records_decode_the_linear_scale() {
        // The upper halfword is a linear 16-bit peak word: 0x8000 is full
        // scale.  AP capture: a looped -20 dBFS tone read 0x21d9 on AN5
        // at the -10 dBV input reference.
        assert!((uc_analog_input_dbfs(0x21d90000).unwrap() + 11.56).abs() < 0.05);
        assert!((uc_analog_input_dbfs(1036u32 << 16).unwrap() + 30.0).abs() < 0.05);
        assert!(uc_analog_input_dbfs(1u32 << 16).unwrap() + 90.3 < 0.1);
        assert_eq!(uc_analog_input_dbfs(0), Some(f32::NEG_INFINITY));
    }

    #[test]
    fn uc_digital_input_records_decode_the_linear_scale() {
        // Bits 8..31 are a linear 24-bit peak word: 0x800000 is full scale.
        // AP capture: the physical S/PDIF loop at -20 dBFS read 0x0ccd0000.
        assert!((uc_digital_dbfs(0x0ccd0000).unwrap() + 20.0).abs() < 0.05);
        let level = (UC_LINEAR_24_FULL_SCALE * 10f32.powf(-6.0 / 20.0)) as u32;
        assert!((uc_digital_dbfs(level << 8).unwrap() + 6.0).abs() < 0.05);
        assert_eq!(uc_digital_dbfs(0), Some(f32::NEG_INFINITY));
        // The low byte is an over count, not a half-dB headroom value.  It
        // does not move the level past the saturated 24-bit level.
        assert!(uc_digital_dbfs(0x8000000f).unwrap().abs() < 1e-6);
        assert!(uc_digital_dbfs(0x8000002f).unwrap().abs() < 1e-6);
        assert!(uc_digital_dbfs(0x7fffff6f).unwrap().abs() < 1e-5);
        // No-data fillers are not levels and must not peg the meter.
        assert_eq!(uc_digital_dbfs(0x00001fff), None);
        assert_eq!(uc_digital_dbfs(0x00000fff), None);
    }

    #[test]
    fn uc_data_page_header_carries_output_rms() {
        let mut b = [0u8; UcMeterPacket::LEN];
        b[4..8].copy_from_slice(&0x1fffu32.to_le_bytes());
        // A nonzero record keeps the page a meter page.
        b[252..256].copy_from_slice(&0x0100_0000u32.to_le_bytes());
        // AN5 left/right: full-scale sine power (0.5) -> 0.7071 RMS.
        b[0..4].copy_from_slice(&0x4000_0000u32.to_le_bytes());
        b[8..12].copy_from_slice(&0x4000_0000u32.to_le_bytes());
        // ADAT 7/8 right is the last header word at byte 104.
        b[104..108].copy_from_slice(&0x2000_0000u32.to_le_bytes());
        let p = UcMeterPacket::parse(&b).expect("data page");
        assert_eq!(p.output_header_rms(4), 0x4000_0000);
        assert_eq!(p.output_header_rms(5), 0x4000_0000);
        assert_eq!(p.output_header_rms(17), 0x2000_0000);
        assert_eq!(p.output_header_rms(0), 0, "AN1 is on the flag page");
        assert!((uc_output_header_rms_linear(0x4000_0000) - 0.70710677).abs() < 1e-5);
    }

}
