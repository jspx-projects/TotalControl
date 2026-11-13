//! Babyface-classic device profile.
//!
//! Ten hardware inputs (two analog, eight optical), six output pairs and
//! twelve software playback channels.  The optical port carries ADAT or
//! S/PDIF; in S/PDIF mode the first optical pair is S/PDIF L/R.  The
//! output blocks are in device order, so the display order is the
//! identity.

use crate::channel::ChannelType;
use crate::profile::{
    ChannelTable, DeviceProfile, GlobalControl, GlobalSet, InputSpec, LayoutSpec, MeterLayout,
    MeterSpec, Operation, OperationSet, OutputPairSpec, PreampSpec, ProfileId, ReadPolicy,
    Requirements, RoutingMode,
};

const fn input(name: &'static str, channel_type: ChannelType) -> InputSpec {
    InputSpec { name, channel_type }
}

const fn pair(left: &'static str, right: &'static str) -> OutputPairSpec {
    OutputPairSpec { left, right }
}

const PAIRS: &[OutputPairSpec] = &[
    pair("AN1", "AN2"),
    pair("PH3", "PH4"),
    pair("AS1", "AS2"),
    pair("ADAT3", "ADAT4"),
    pair("ADAT5", "ADAT6"),
    pair("ADAT7", "ADAT8"),
];

pub const CHANNELS: ChannelTable = ChannelTable {
    inputs: &[
        input("AN1", ChannelType::Mic),
        input("AN2", ChannelType::Mic),
        input("AS1", ChannelType::ADAT),
        input("AS2", ChannelType::ADAT),
        input("ADAT3", ChannelType::ADAT),
        input("ADAT4", ChannelType::ADAT),
        input("ADAT5", ChannelType::ADAT),
        input("ADAT6", ChannelType::ADAT),
        input("ADAT7", ChannelType::ADAT),
        input("ADAT8", ChannelType::ADAT),
    ],
    outputs: PAIRS,
    playback_pairs: PAIRS,
    output_order: &[0, 1, 2, 3, 4, 5],
};

const INPUT_OPS: OperationSet = OperationSet::new(
    &[
        Operation::Send,
        Operation::Pan,
        Operation::Mute,
        Operation::Solo,
        Operation::Trim,
        Operation::Name,
        Operation::Gain,
        Operation::Phantom,
    ],
    &[Operation::Gain],
);

const PLAYBACK_OPS: OperationSet = OperationSet::new(
    &[
        Operation::Send,
        Operation::Pan,
        Operation::Mute,
        Operation::Solo,
        Operation::Trim,
        Operation::Name,
    ],
    &[],
);

const OUTPUT_OPS: OperationSet = OperationSet::new(
    &[Operation::Volume, Operation::Mute, Operation::Name],
    &[Operation::Volume],
);

const GLOBALS: GlobalSet = GlobalSet::new(
    &[GlobalControl::ClockSource, GlobalControl::SampleRate],
    &[],
);

const PREAMPS: &[PreampSpec] = &[
    PreampSpec {
        input: 0,
        gain_min: 6.0,
        gain_max: 60.0,
        gain_step: 3.0,
        phantom: true,
        pad: false,
        instrument: false,
        level: false,
        autoset: false,
        read: ReadPolicy::Host,
    },
    PreampSpec {
        input: 1,
        gain_min: 6.0,
        gain_max: 60.0,
        gain_step: 3.0,
        phantom: true,
        pad: false,
        instrument: false,
        level: false,
        autoset: false,
        read: ReadPolicy::Host,
    },
];

const METERS: MeterSpec = MeterSpec {
    layout: MeterLayout::BabyfaceClassic,
    hardware: true,
    playback: false,
};

const LAYOUT: LayoutSpec = LayoutSpec { template: "no-fx" };

const REQUIRES: Requirements = Requirements {
    min_inputs: 10,
    min_outputs: 12,
    min_playback: 12,
    meter_len: Some(512),
};

pub const PROFILE: DeviceProfile = DeviceProfile {
    id: ProfileId::BabyfaceClassic,
    model_name: "Babyface-classic",
    channels: &CHANNELS,
    routing: RoutingMode::Pair,
    input_ops: INPUT_OPS,
    playback_ops: PLAYBACK_OPS,
    output_ops: OUTPUT_OPS,
    globals: GLOBALS,
    clock_sources: &[],
    preamps: PREAMPS,
    meters: METERS,
    layout: LAYOUT,
    requires: REQUIRES,
    phase_without_eq: false,
    channel_width: false,
    dsp: false,
};

#[cfg(test)]
mod tests {
    use super::CHANNELS;

    #[test]
    fn topology_is_ten_in_twelve_out() {
        assert_eq!(CHANNELS.input_count(), 10);
        assert_eq!(CHANNELS.output_pair_count(), 6);
        assert_eq!(CHANNELS.playback_pairs.len(), 6);
    }
}
