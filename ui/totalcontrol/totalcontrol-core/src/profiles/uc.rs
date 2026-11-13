//! Fireface UC profile.
//!
//! The UC has the same 18-in / 18-out TotalMix **topology** as the UCX, with
//! no EQ/dynamics/FX section. The topology data is shared below, but protocol,
//! register, meter and feature rules are not: see `docs/DEVICE-PROFILES.md`.
//! The active UC path is interrupt IF5; the ISO IF0 description in
//! `docs/DRIVER-SPEC.md` is the parked UCX path.

use crate::profile::{
    ChannelTable, DeviceProfile, GlobalControl, GlobalSet, LayoutSpec, MeterLayout, MeterSpec,
    Operation, OperationSet, OutputPairSpec, PreampSpec, ProfileId, ReadPolicy, Requirements,
    RoutingMode,
};

use super::ucx;

pub const CHANNELS: ChannelTable = ChannelTable {
    // The channel names and TotalMix row order are shared with the UCX.
    // Only the DSP/FX feature set differs, and that lives in the backend.
    inputs: ucx::CHANNELS.inputs,
    // The UC numbers its hardware outputs ascending, so Phones sits at
    // Out 7/8 (its default TotalMix name is "AN 7/8"), not first as on the
    // UCX.  Same nine pairs, same names, device block order.
    outputs: &[
        OutputPairSpec {
            left: "AN1",
            right: "AN2",
        },
        OutputPairSpec {
            left: "AN3",
            right: "AN4",
        },
        OutputPairSpec {
            left: "AN5",
            right: "AN6",
        },
        OutputPairSpec {
            left: "Phones 7",
            right: "Phones 8",
        },
        OutputPairSpec {
            left: "SPDIF",
            right: "SPDIF",
        },
        OutputPairSpec {
            left: "AS 1",
            right: "AS 2",
        },
        OutputPairSpec {
            left: "ADAT 3",
            right: "ADAT 4",
        },
        OutputPairSpec {
            left: "ADAT 5",
            right: "ADAT 6",
        },
        OutputPairSpec {
            left: "ADAT 7",
            right: "ADAT 8",
        },
    ],
    playback_pairs: ucx::CHANNELS.playback_pairs,
    // TotalMix FX lists the UC's hardware outputs numerically ascending:
    // Out 1/2 = AN1/2, 3/4 = AN3/4, 5/6 = AN5/6, 7/8 = Phones (default name
    // "AN 7/8"), 9/10 = SPDIF, 11/12..17/18 = ADAT.  The block order the
    // device uses for `outputs` is the same, so the display order is the
    // identity and no permutation applies.
    output_order: &[0, 1, 2, 3, 4, 5, 6, 7, 8],
};

const INPUT_OPS: OperationSet = OperationSet::new(
    &[
        Operation::Send,
        Operation::Routing,
        Operation::Pan,
        Operation::Mute,
        Operation::Solo,
        Operation::Phase,
        Operation::Trim,
        Operation::Name,
        Operation::Link,
        Operation::Width,
        Operation::Ms,
        Operation::Gain,
        Operation::Phantom,
        Operation::Pad,
        Operation::Instrument,
    ],
    &[Operation::Gain],
);

const PLAYBACK_OPS: OperationSet = OperationSet::new(
    &[
        Operation::Send,
        Operation::Routing,
        Operation::Pan,
        Operation::Mute,
        Operation::Solo,
        Operation::Trim,
        Operation::Name,
        Operation::Link,
        Operation::Width,
        Operation::Ms,
    ],
    &[],
);

const OUTPUT_OPS: OperationSet = OperationSet::new(
    &[
        Operation::Volume,
        Operation::Mute,
        Operation::Solo,
        Operation::Balance,
        Operation::Phase,
        Operation::Link,
        Operation::NoTrim,
        Operation::Loopback,
    ],
    &[Operation::Volume, Operation::Balance],
);

const GLOBALS: GlobalSet = GlobalSet::new(
    &[
        GlobalControl::ClockSource,
        GlobalControl::SampleRate,
        GlobalControl::OpticalFormat,
        GlobalControl::SpdifProfessional,
        GlobalControl::WordClock,
        GlobalControl::An12Link,
    ],
    &[],
);

const PREAMPS: &[PreampSpec] = &[
    PreampSpec {
        input: 0,
        gain_min: 0.0,
        gain_max: 65.0,
        gain_step: 1.0,
        phantom: true,
        pad: false,
        instrument: false,
        level: false,
        autoset: false,
        read: ReadPolicy::Device,
    },
    PreampSpec {
        input: 1,
        gain_min: 0.0,
        gain_max: 65.0,
        gain_step: 1.0,
        phantom: true,
        pad: false,
        instrument: false,
        level: false,
        autoset: false,
        read: ReadPolicy::Device,
    },
    PreampSpec {
        input: 2,
        gain_min: 0.0,
        gain_max: 18.0,
        gain_step: 0.5,
        phantom: false,
        pad: true,
        instrument: true,
        level: false,
        autoset: false,
        read: ReadPolicy::Device,
    },
    PreampSpec {
        input: 3,
        gain_min: 0.0,
        gain_max: 18.0,
        gain_step: 0.5,
        phantom: false,
        pad: true,
        instrument: true,
        level: false,
        autoset: false,
        read: ReadPolicy::Device,
    },
];

const METERS: MeterSpec = MeterSpec {
    layout: MeterLayout::UcClass1,
    hardware: true,
    playback: false,
};

const LAYOUT: LayoutSpec = LayoutSpec { template: "no-fx" };

const REQUIRES: Requirements = Requirements {
    min_inputs: 18,
    min_outputs: 18,
    min_playback: 18,
    meter_len: Some(324),
};

pub const PC: DeviceProfile = DeviceProfile {
    id: ProfileId::UcPc,
    model_name: "Fireface UC",
    channels: &CHANNELS,
    routing: RoutingMode::Cell,
    input_ops: INPUT_OPS,
    playback_ops: PLAYBACK_OPS,
    output_ops: OUTPUT_OPS,
    globals: GLOBALS,
    preamps: PREAMPS,
    meters: METERS,
    layout: LAYOUT,
    requires: REQUIRES,
    phase_without_eq: true,
    channel_width: false,
    dsp: false,
};

pub const AP: DeviceProfile = DeviceProfile {
    id: ProfileId::UcAp,
    ..PC
};

#[cfg(test)]
mod tests {
    use super::CHANNELS;

    #[test]
    fn hardware_output_row_is_numeric_ascending_with_phones_at_out_7_8() {
        // Out 1/2 AN1/2, 3/4 AN3/4, 5/6 AN5/6, 7/8 Phones, then SPDIF and
        // ADAT.  The display order is the block order, so Phones is third
        // pair in display terms and eighth/ninth channel.
        assert_eq!(CHANNELS.output_order, &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(CHANNELS.outputs[3].left, "Phones 7");
        assert_eq!(CHANNELS.outputs[3].right, "Phones 8");
        assert_eq!(CHANNELS.outputs[4].left, "SPDIF");
    }
}
