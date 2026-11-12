//! Fireface UC profile.
//!
//! The UC has the same 18-in / 18-out TotalMix **topology** as the UCX, with
//! no EQ/dynamics/FX section. The topology data is shared below, but protocol,
//! register, meter and feature rules are not: see `docs/DEVICE-PROFILES.md`.
//! The active UC path is interrupt IF5; the ISO IF0 description in
//! `docs/DRIVER-SPEC.md` is the parked UCX path.

use crate::profile::{DeviceProfile, OutputPairSpec};

use super::ucx;

pub const PROFILE: DeviceProfile = DeviceProfile {
    card_substring: "Fireface UC",
    model_name: "Fireface UC",
    // The channel names and TotalMix row order are shared with the UCX.
    // Only the DSP/FX feature set differs, and that lives in the backend.
    inputs: ucx::PROFILE.inputs,
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
    playback_pairs: ucx::PROFILE.playback_pairs,
    // TotalMix FX lists the UC's hardware outputs numerically ascending:
    // Out 1/2 = AN1/2, 3/4 = AN3/4, 5/6 = AN5/6, 7/8 = Phones (default name
    // "AN 7/8"), 9/10 = SPDIF, 11/12..17/18 = ADAT.  The block order the
    // device uses for `outputs` is the same, so the display order is the
    // identity and no permutation applies.
    output_order: &[0, 1, 2, 3, 4, 5, 6, 7, 8],
};


#[cfg(test)]
mod tests {
    use super::PROFILE;

    #[test]
    fn hardware_output_row_is_numeric_ascending_with_phones_at_out_7_8() {
        // Out 1/2 AN1/2, 3/4 AN3/4, 5/6 AN5/6, 7/8 Phones, then SPDIF and
        // ADAT.  The display order is the block order, so Phones is third
        // pair in display terms and eighth/ninth channel.
        assert_eq!(PROFILE.output_order, &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(PROFILE.outputs[3].left, "Phones 7");
        assert_eq!(PROFILE.outputs[3].right, "Phones 8");
        assert_eq!(PROFILE.outputs[4].left, "SPDIF");
    }
}
