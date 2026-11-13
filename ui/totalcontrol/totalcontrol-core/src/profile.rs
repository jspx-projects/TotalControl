//! Declarative device profiles — see
//! `docs/spec/TOTALCONTROL-DEVICE-PROFILE-SPEC.md`.
//!
//! A [`ChannelTable`] describes the channel topology. A [`DeviceProfile`]
//! adds the host operations, globals, preamps, meters, layout selection and
//! connection requirements for one `(model, transport)` pair. The profile
//! table in [`crate::profiles`] is the single source for both.

use crate::capabilities::DeviceCapabilities;
use crate::channel::{ChannelType, InputChannel, OutputChannel, PlaybackChannel, Sensitivity};

/// Read policy for one host operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPolicy {
    /// The host reads the current value from the unit.
    Device,
    /// The host scene is authoritative; the unit has no readback.
    Host,
    /// The operation is not available.
    None,
}

/// Host operation on a channel row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    Send,
    Routing,
    Pan,
    Mute,
    Solo,
    Phase,
    Trim,
    Name,
    Link,
    Eq,
    LowCut,
    Dynamics,
    FxSend,
    FxReturn,
    Loopback,
    Width,
    Ms,
    NoTrim,
    TalkbackReceive,
    Volume,
    Balance,
    Gain,
    Phantom,
    Pad,
    Instrument,
    Level,
    Autoset,
}

impl Operation {
    pub const ALL: [Operation; 27] = [
        Operation::Send,
        Operation::Routing,
        Operation::Pan,
        Operation::Mute,
        Operation::Solo,
        Operation::Phase,
        Operation::Trim,
        Operation::Name,
        Operation::Link,
        Operation::Eq,
        Operation::LowCut,
        Operation::Dynamics,
        Operation::FxSend,
        Operation::FxReturn,
        Operation::Loopback,
        Operation::Width,
        Operation::Ms,
        Operation::NoTrim,
        Operation::TalkbackReceive,
        Operation::Volume,
        Operation::Balance,
        Operation::Gain,
        Operation::Phantom,
        Operation::Pad,
        Operation::Instrument,
        Operation::Level,
        Operation::Autoset,
    ];

    const fn bit(self) -> u64 {
        1u64 << self as u8
    }
}

/// The set operations for one channel class, plus the subset the host can
/// read from the unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperationSet {
    set: u64,
    device_read: u64,
}

impl OperationSet {
    pub const NONE: Self = Self {
        set: 0,
        device_read: 0,
    };

    pub const fn new(set: &[Operation], device_read: &[Operation]) -> Self {
        Self {
            set: bits(set),
            device_read: bits(device_read),
        }
    }

    pub const fn contains(self, operation: Operation) -> bool {
        self.set & operation.bit() != 0
    }

    pub fn read_policy(self, operation: Operation) -> ReadPolicy {
        if self.device_read & operation.bit() != 0 {
            ReadPolicy::Device
        } else if self.set & operation.bit() != 0 {
            ReadPolicy::Host
        } else {
            ReadPolicy::None
        }
    }
}

const fn bits(ops: &[Operation]) -> u64 {
    let mut value = 0;
    let mut i = 0;
    while i < ops.len() {
        value |= ops[i].bit();
        i += 1;
    }
    value
}

/// Host-operable device-wide control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GlobalControl {
    ClockSource,
    SampleRate,
    OpticalFormat,
    SpdifProfessional,
    WordClock,
    EqForRecord,
    Talkback,
    Dim,
    SpeakerB,
    Mono,
    Pitch,
    An12Link,
}

impl GlobalControl {
    const fn bit(self) -> u64 {
        1u64 << self as u8
    }
}

/// The set of host-operable device-wide controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GlobalSet {
    set: u64,
    device_read: u64,
}

impl GlobalSet {
    pub const NONE: Self = Self {
        set: 0,
        device_read: 0,
    };

    pub const fn new(set: &[GlobalControl], device_read: &[GlobalControl]) -> Self {
        Self {
            set: global_bits(set),
            device_read: global_bits(device_read),
        }
    }

    pub const fn contains(self, control: GlobalControl) -> bool {
        self.set & control.bit() != 0
    }

    pub fn read_policy(self, control: GlobalControl) -> ReadPolicy {
        if self.device_read & control.bit() != 0 {
            ReadPolicy::Device
        } else if self.set & control.bit() != 0 {
            ReadPolicy::Host
        } else {
            ReadPolicy::None
        }
    }
}

const fn global_bits(controls: &[GlobalControl]) -> u64 {
    let mut value = 0;
    let mut i = 0;
    while i < controls.len() {
        value |= controls[i].bit();
        i += 1;
    }
    value
}

/// How the profile addresses the mixer matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingMode {
    Pair,
    Cell,
}

/// One input's preamp data.
#[derive(Debug, Clone, Copy)]
pub struct PreampSpec {
    pub input: usize,
    pub gain_min: f32,
    pub gain_max: f32,
    pub gain_step: f32,
    pub phantom: bool,
    pub pad: bool,
    pub instrument: bool,
    pub level: bool,
    pub autoset: bool,
    /// Read policy for gain and the switches.
    pub read: ReadPolicy,
}

/// Hardware meter capability.
#[derive(Debug, Clone, Copy)]
pub struct MeterSpec {
    pub layout: MeterLayout,
    pub hardware: bool,
    pub playback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterLayout {
    None,
    UcClass1,
    UcxIf1,
    BabyfaceClassic,
}

/// Layout selection for a profile.
#[derive(Debug, Clone, Copy)]
pub struct LayoutSpec {
    pub template: &'static str,
}

/// Preconditions checked against `HwdepInfo`.
#[derive(Debug, Clone, Copy)]
pub struct Requirements {
    pub min_inputs: usize,
    pub min_outputs: usize,
    pub min_playback: usize,
    pub meter_len: Option<u32>,
}

/// Profile id, one per supported `(model, transport)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileId {
    UcPc,
    UcAp,
    UcxIso,
    UcxIf5,
    BabyfaceClassic,
}

impl ProfileId {
    pub const ALL: [ProfileId; 5] = [
        ProfileId::UcPc,
        ProfileId::UcAp,
        ProfileId::UcxIso,
        ProfileId::UcxIf5,
        ProfileId::BabyfaceClassic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProfileId::UcPc => "uc_pc",
            ProfileId::UcAp => "uc_ap",
            ProfileId::UcxIso => "ucx_iso",
            ProfileId::UcxIf5 => "ucx_if5",
            ProfileId::BabyfaceClassic => "babyface_classic",
        }
    }
}

/// Channel topology for one model.
#[derive(Debug, Clone, Copy)]
pub struct ChannelTable {
    /// One entry per physical hardware input, in channel-id order.
    pub inputs: &'static [InputSpec],
    /// One entry per physical stereo output pair, in device block order.
    pub outputs: &'static [OutputPairSpec],
    /// One entry per software-playback stereo pair, in device order.
    pub playback_pairs: &'static [OutputPairSpec],
    /// Hardware-output display order.
    pub output_order: &'static [usize],
}

impl ChannelTable {
    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn output_pair_count(&self) -> usize {
        self.outputs.len()
    }

    pub fn build_inputs(&self) -> Vec<InputChannel> {
        self.inputs
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let mut ch =
                    InputChannel::new(i, spec.name, spec.channel_type, self.output_pair_count());
                ch.sensitivity = if spec.channel_type == ChannelType::Instrument {
                    Some(Sensitivity::Plus4dBu)
                } else {
                    None
                };
                ch
            })
            .collect()
    }

    pub fn build_playbacks(&self) -> Vec<PlaybackChannel> {
        let mut pbs = Vec::new();
        let outputs = self.output_pair_count();
        for (i, pair) in self.playback_pairs.iter().enumerate() {
            let mut left = PlaybackChannel::new(i * 2, &format!("PCM {}", pair.left), outputs);
            let mut right =
                PlaybackChannel::new(i * 2 + 1, &format!("PCM {}", pair.right), outputs);
            let target = self.output_order.get(i).copied().unwrap_or(i);
            if let Some(slot) = left.volumes.get_mut(target) {
                *slot = 1.0;
            }
            if let Some(slot) = right.volumes.get_mut(target) {
                *slot = 1.0;
            }
            pbs.push(left);
            pbs.push(right);
        }
        pbs
    }

    pub fn build_outputs(&self) -> Vec<OutputChannel> {
        let mut outs = Vec::new();
        for (i, pair) in self.outputs.iter().enumerate() {
            outs.push(OutputChannel::new(i * 2, &format!("OUT {}", pair.left)));
            outs.push(OutputChannel::new(
                i * 2 + 1,
                &format!("OUT {}", pair.right),
            ));
        }
        outs
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InputSpec {
    pub name: &'static str,
    pub channel_type: ChannelType,
}

#[derive(Debug, Clone, Copy)]
pub struct OutputPairSpec {
    pub left: &'static str,
    pub right: &'static str,
}

/// One `(model, transport)` profile.
#[derive(Debug, Clone, Copy)]
pub struct DeviceProfile {
    pub id: ProfileId,
    pub model_name: &'static str,
    pub channels: &'static ChannelTable,
    pub routing: RoutingMode,
    pub input_ops: OperationSet,
    pub playback_ops: OperationSet,
    pub output_ops: OperationSet,
    pub globals: GlobalSet,
    pub preamps: &'static [PreampSpec],
    pub meters: MeterSpec,
    pub layout: LayoutSpec,
    pub requires: Requirements,
    /// Input phase works without an EQ section.
    pub phase_without_eq: bool,
    /// The device has a per-channel width control rather than one global one.
    pub channel_width: bool,
    /// The vendor DSP surface is available.
    pub dsp: bool,
}

impl DeviceProfile {
    pub fn input_count(&self) -> usize {
        self.channels.input_count()
    }

    pub fn output_pair_count(&self) -> usize {
        self.channels.output_pair_count()
    }

    pub fn build_inputs(&self) -> Vec<InputChannel> {
        self.channels.build_inputs()
    }

    pub fn build_playbacks(&self) -> Vec<PlaybackChannel> {
        self.channels.build_playbacks()
    }

    pub fn build_outputs(&self) -> Vec<OutputChannel> {
        self.channels.build_outputs()
    }

    pub fn output_order(&self) -> &'static [usize] {
        self.channels.output_order
    }

    pub fn input_op(&self, operation: Operation) -> ReadPolicy {
        self.input_ops.read_policy(operation)
    }

    pub fn playback_op(&self, operation: Operation) -> ReadPolicy {
        self.playback_ops.read_policy(operation)
    }

    pub fn output_op(&self, operation: Operation) -> ReadPolicy {
        self.output_ops.read_policy(operation)
    }

    /// Compatibility view for the current UI. The profile is the source;
    /// this is derived and removed when the UI reads operations directly.
    pub fn capabilities(&self) -> DeviceCapabilities {
        let has_input = |op| self.input_ops.contains(op);
        let has_playback = |op| self.playback_ops.contains(op);
        let has_output = |op| self.output_ops.contains(op);
        let has_global = |control| self.globals.contains(control);
        DeviceCapabilities {
            protocol_features: self.dsp,
            eq: has_input(Operation::Eq) || has_output(Operation::Eq),
            low_cut: has_input(Operation::LowCut) || has_output(Operation::LowCut),
            loopback: has_output(Operation::Loopback),
            no_trim: has_output(Operation::NoTrim),
            talkback: has_global(GlobalControl::Talkback),
            phase: has_input(Operation::Phase),
            phase_without_eq: self.phase_without_eq,
            output_phase: has_output(Operation::Phase),
            output_balance: has_output(Operation::Balance),
            stereo_split: has_input(Operation::Link)
                || has_playback(Operation::Link)
                || has_output(Operation::Link),
            matrix_cells: self.routing == RoutingMode::Cell,
            class_compliant: false,
            input_phantom: self.preamps.iter().any(|p| p.phantom),
            input_pad: self.preamps.iter().any(|p| p.pad),
            input_level: self.preamps.iter().any(|p| p.level),
            input_autoset: self.preamps.iter().any(|p| p.autoset),
            dynamics: has_input(Operation::Dynamics) || has_output(Operation::Dynamics),
            fx: has_input(Operation::FxSend)
                && has_playback(Operation::FxSend)
                && has_output(Operation::FxReturn),
            pitch: has_global(GlobalControl::Pitch),
            width: (has_input(Operation::Width) || has_playback(Operation::Width))
                && !self.channel_width,
            channel_width: self.channel_width,
            ms_proc: has_input(Operation::Ms) || has_playback(Operation::Ms),
            an12: has_global(GlobalControl::An12Link),
            input_link: has_input(Operation::Link),
            eq_for_record: has_global(GlobalControl::EqForRecord),
            optical_out: has_global(GlobalControl::OpticalFormat),
        }
    }
}
