//! Personality identity and profile selection for the hwdep backend.
//!
//! Wire encoding lives in `protocol.rs`; channel topology and host
//! operations live in the profile table under `crate::profiles`.

use crate::profile::DeviceProfile;

use super::device::Model;

/// Transport/personality identity reported by the kernel for one unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Personality {
    UcPc,
    UcAp,
    UcxIso,
    UcxIf5,
}

impl Personality {
    pub fn from_model(model: Model, iso: bool) -> Option<Self> {
        Some(match (model, iso) {
            (Model::Uc, false) => Personality::UcPc,
            (Model::Uc, true) => Personality::UcAp,
            (Model::Ucx, true) => Personality::UcxIso,
            (Model::Ucx, false) => Personality::UcxIf5,
        })
    }

    pub fn model(self) -> Model {
        match self {
            Personality::UcPc | Personality::UcAp => Model::Uc,
            Personality::UcxIso | Personality::UcxIf5 => Model::Ucx,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Personality::UcPc => "UC PC (interrupt)",
            Personality::UcAp => "UC AP (ISO)",
            Personality::UcxIso => "UCX ISO",
            Personality::UcxIf5 => "UCX IF5 (interrupt)",
        }
    }

    /// Short form shown in the device chip.
    pub fn short_label(self) -> &'static str {
        match self {
            Personality::UcPc => "PC",
            Personality::UcAp => "AP",
            Personality::UcxIso => "ISO",
            Personality::UcxIf5 => "IF5",
        }
    }
}

/// Select the profile for one reported personality.
pub fn profile_for_personality(personality: Personality) -> &'static DeviceProfile {
    match personality {
        Personality::UcPc => &crate::profiles::uc::PC,
        Personality::UcAp => &crate::profiles::uc::AP,
        Personality::UcxIso => &crate::profiles::ucx::ISO,
        Personality::UcxIf5 => &crate::profiles::ucx::IF5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileId;

    #[test]
    fn personality_is_derived_from_model_and_transport() {
        assert_eq!(
            Personality::from_model(Model::Uc, false),
            Some(Personality::UcPc)
        );
        assert_eq!(
            Personality::from_model(Model::Uc, true),
            Some(Personality::UcAp)
        );
        assert_eq!(
            Personality::from_model(Model::Ucx, true),
            Some(Personality::UcxIso)
        );
        assert_eq!(
            Personality::from_model(Model::Ucx, false),
            Some(Personality::UcxIf5)
        );
    }

    #[test]
    fn personality_short_labels_match_the_device_display() {
        assert_eq!(Personality::UcPc.short_label(), "PC");
        assert_eq!(Personality::UcAp.short_label(), "AP");
        assert_eq!(Personality::UcxIso.short_label(), "ISO");
        assert_eq!(Personality::UcxIf5.short_label(), "IF5");
    }

    #[test]
    fn personality_selects_its_profile() {
        assert_eq!(
            profile_for_personality(Personality::UcPc).id,
            ProfileId::UcPc
        );
        assert_eq!(
            profile_for_personality(Personality::UcAp).id,
            ProfileId::UcAp
        );
        assert_eq!(
            profile_for_personality(Personality::UcxIso).id,
            ProfileId::UcxIso
        );
        assert_eq!(
            profile_for_personality(Personality::UcxIf5).id,
            ProfileId::UcxIf5
        );
    }
}
