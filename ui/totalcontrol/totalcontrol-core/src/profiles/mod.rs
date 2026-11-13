//! Per-model device profiles — see [`crate::profile::DeviceProfile`].
//!
//! Each supported model has one profile const. The profile is the single
//! source for channel names, topology and output display order. Backends and
//! mocks build their channels from it, so they cannot drift apart.

pub mod babyface_classic;
pub mod uc;
pub mod ucx;

use crate::profile::{DeviceProfile, ProfileId};

/// The compiled profile table. One entry per supported `(model, transport)`.
pub const ALL: [&DeviceProfile; 5] = [
    &uc::PC,
    &uc::AP,
    &ucx::ISO,
    &ucx::IF5,
    &babyface_classic::PROFILE,
];

pub fn profile(id: ProfileId) -> &'static DeviceProfile {
    match id {
        ProfileId::UcPc => &uc::PC,
        ProfileId::UcAp => &uc::AP,
        ProfileId::UcxIso => &ucx::ISO,
        ProfileId::UcxIf5 => &ucx::IF5,
        ProfileId::BabyfaceClassic => &babyface_classic::PROFILE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Operation, ReadPolicy};

    #[test]
    fn table_has_one_entry_per_id() {
        let ids: Vec<_> = ALL.iter().map(|p| p.id).collect();
        assert_eq!(ids, ProfileId::ALL.to_vec());
        for id in ProfileId::ALL {
            assert_eq!(profile(id).id, id, "{}", id.label());
        }
    }

    #[test]
    fn every_profile_meets_its_own_requirements() {
        for p in ALL {
            assert!(p.input_count() >= p.requires.min_inputs, "{}", p.id.label());
            assert!(
                p.output_pair_count() * 2 >= p.requires.min_outputs,
                "{}",
                p.id.label()
            );
            assert!(
                p.channels.playback_pairs.len() * 2 >= p.requires.min_playback,
                "{}",
                p.id.label()
            );
        }
    }

    #[test]
    fn uc_and_ucx_share_topology_but_not_dsp() {
        let uc = profile(ProfileId::UcPc);
        let ucx = profile(ProfileId::UcxIso);
        assert_eq!(uc.input_count(), ucx.input_count());
        assert_eq!(uc.output_pair_count(), ucx.output_pair_count());
        assert!(!uc.dsp);
        assert!(ucx.dsp);
        assert_eq!(uc.meters.layout, crate::profile::MeterLayout::UcClass1);
        assert_eq!(ucx.meters.layout, crate::profile::MeterLayout::UcxIf1);
    }

    #[test]
    fn output_volume_is_a_device_readback() {
        for id in [
            ProfileId::UcPc,
            ProfileId::UcxIso,
            ProfileId::BabyfaceClassic,
        ] {
            let p = profile(id);
            assert_eq!(
                p.output_op(Operation::Volume),
                ReadPolicy::Device,
                "{}",
                id.label()
            );
        }
    }

    #[test]
    fn switches_without_readback_are_host_held() {
        let uc = profile(ProfileId::UcPc);
        assert_eq!(uc.input_op(Operation::Phantom), ReadPolicy::Host);
        assert_eq!(uc.input_op(Operation::Gain), ReadPolicy::Device);
    }
}
