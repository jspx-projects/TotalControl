//! The control-only (two-driver) device model.
//!
//! Some devices have working audio in the mainline kernel driver while a
//! small second driver of ours carries only control and meters. Such a
//! device is a **control card** (our hwdep node, no PCM) paired with a
//! separate **audio card** (the mainline PCM card). TotalControl shows and
//! handles the pair as one device.
//!
//! This module is the shared framework: a descriptor per device, a pairing
//! rule, and the [`ControlOnly`] wrapper that owns the audio/control identity
//! separation. A device backend implements the ordinary [`RmeDevice`]
//! contract over its hwdep ABI and registers one descriptor; the framework
//! does the pairing and reports the audio card id, so a backend cannot point
//! the clock or the meters at the control card by mistake.
//!
//! The plan and the rationale are in `docs/work/TWO-DRIVER-DEVICE-MODEL.md`.
//!
//! The framework is complete, but its first device backend (the Digiface
//! USB) is a separate contribution. Until a descriptor is registered, the
//! production paths run only against an empty registry and the contract is
//! exercised by the mock tests, so some items are unused outside tests.

#![allow(dead_code)]

use std::path::Path;

use totalcontrol_core::{Error, RmeDevice};

use crate::app::{DeviceKind, DeviceTarget};

/// How to find the mainline audio card. Match by id or by a description
/// fragment, never by card number: numbers change across reconnects.
#[derive(Clone, Debug)]
pub enum CardMatch {
    /// The card's ALSA id (`/proc/asound/cardN/id`).
    Id(&'static str),
    /// A fragment of the card's description in `/proc/asound/cards`.
    DescriptionContains(&'static str),
}

impl CardMatch {
    fn matches(&self, card: &SoundCard) -> bool {
        match self {
            CardMatch::Id(id) => card.id == *id,
            CardMatch::DescriptionContains(frag) => card.description.contains(frag),
        }
    }
}

/// One control-only device registration. A new device adds one entry plus
/// its backend and profile; the framework handles the rest.
pub struct ControlOnlyDescriptor {
    /// The selectable device kind.
    pub kind: DeviceKind,
    /// The model name shown in the device menu.
    pub model_name: &'static str,
    /// The ALSA id of our hwdep control card.
    pub control_card_id: &'static str,
    /// How to find the paired mainline audio card.
    pub audio_card_match: CardMatch,
    /// Open the backend on the paired control card.
    pub open: fn(&ControlOnlyBinding) -> Result<Box<dyn RmeDevice + Send>, Error>,
}

/// A snapshot of one ALSA card, for pairing.
#[derive(Clone, Debug)]
pub struct SoundCard {
    pub number: i32,
    pub id: String,
    pub description: String,
    /// Whether `/dev/snd/hwC{number}D0` exists.
    pub has_hwdep: bool,
}

/// The paired cards of a control-only device.
#[derive(Clone, Debug)]
pub struct ControlOnlyBinding {
    /// Our hwdep control card number (no PCM).
    pub control_card: i32,
    /// The mainline audio card number.
    pub audio_card: i32,
    /// The mainline audio card id, used for the clock, controllers and meters.
    pub audio_card_id: String,
}

/// Pair a control card with its audio card. Pure: the caller supplies the
/// card list, so the rule is testable without hardware.
///
/// The control card matches by id and must expose hwdep. The audio card
/// matches the descriptor's rule and must be a different card. Either card
/// missing yields `None`.
pub fn pair(desc: &ControlOnlyDescriptor, cards: &[SoundCard]) -> Option<ControlOnlyBinding> {
    let control = cards
        .iter()
        .find(|c| c.id == desc.control_card_id && c.has_hwdep)?;
    let audio = cards
        .iter()
        .find(|c| c.number != control.number && desc.audio_card_match.matches(c))?;
    Some(ControlOnlyBinding {
        control_card: control.number,
        audio_card: audio.number,
        audio_card_id: audio.id.clone(),
    })
}

/// Read the ALSA card list from procfs and sysfs. Cards without a readable
/// id are skipped; a missing procfs yields an empty list.
pub fn read_sound_cards() -> Vec<SoundCard> {
    let Ok(text) = std::fs::read_to_string("/proc/asound/cards") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        // " 2 [FirefaceUCX    ]: FirefaceUSB - Fireface UCX"
        let Some(open) = line.find('[') else { continue };
        let Some(close) = line.find(']') else { continue };
        let Ok(number) = line[..open].trim().parse::<i32>() else {
            continue;
        };
        let id = line[open + 1..close].trim().to_string();
        let description = line[close + 1..]
            .trim_start_matches(':')
            .trim()
            .to_string();
        let has_hwdep = Path::new(&format!("/dev/snd/hwC{number}D0")).exists();
        out.push(SoundCard { number, id, description, has_hwdep });
    }
    out
}

/// A control-only device: a backend plus the paired-card identity. The
/// backend implements [`RmeDevice`] over the control card's hwdep ABI; this
/// wrapper owns the audio/control separation. `DeviceHandle` forwards the
/// `RmeDevice` methods to `backend` and reads the identity from here.
pub struct ControlOnly {
    /// The device backend, on the control card.
    pub backend: Box<dyn RmeDevice + Send>,
    /// The paired cards.
    pub binding: ControlOnlyBinding,
    /// The selectable kind.
    pub kind: DeviceKind,
}

impl ControlOnly {
    /// The audio card id — the mainline card, not the control card. This is
    /// what the clock, controller discovery and meter paths must use.
    pub fn audio_card_id(&self) -> String {
        self.binding.audio_card_id.clone()
    }

    /// The stable target. Control-only devices are selected by kind until a
    /// serial-bearing identity is defined for them.
    pub fn target(&self) -> DeviceTarget {
        DeviceTarget::kind_only(self.kind)
    }
}

/// The registered control-only devices. Empty until a device backend lands;
/// the Digiface USB registers here once its kernel module and spec exist.
pub fn descriptors() -> Vec<&'static ControlOnlyDescriptor> {
    #[allow(unused_mut)]
    let mut out: Vec<&'static ControlOnlyDescriptor> = Vec::new();
    #[cfg(test)]
    out.push(&test_support::MOCK_DESCRIPTOR);
    out
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use totalcontrol_core::MockDevice;

    /// A mock control card id and audio card id for the framework tests.
    pub const MOCK_CONTROL_ID: &str = "MockControl";
    pub const MOCK_AUDIO_ID: &str = "FirefaceUCX";

    fn open_mock(_binding: &ControlOnlyBinding) -> Result<Box<dyn RmeDevice + Send>, Error> {
        Ok(Box::new(MockDevice::open_ucx()?))
    }

    /// A static descriptor backed by the simulation, registered under
    /// `cfg(test)` so the framework path can be exercised without hardware.
    pub static MOCK_DESCRIPTOR: ControlOnlyDescriptor = ControlOnlyDescriptor {
        kind: DeviceKind::Digiface,
        model_name: "Digiface USB (mock control-only)",
        control_card_id: MOCK_CONTROL_ID,
        audio_card_match: CardMatch::Id(MOCK_AUDIO_ID),
        open: open_mock,
    };

    /// A descriptor backed by the simulation, so the framework path can be
    /// exercised without hardware.
    pub fn mock_descriptor() -> ControlOnlyDescriptor {
        ControlOnlyDescriptor {
            kind: DeviceKind::Digiface,
            model_name: "Digiface USB (mock control-only)",
            control_card_id: MOCK_CONTROL_ID,
            audio_card_match: CardMatch::Id(MOCK_AUDIO_ID),
            open: open_mock,
        }
    }

    /// A synthetic card list with the control card and the audio card.
    pub fn mock_cards() -> Vec<SoundCard> {
        vec![
            SoundCard {
                number: 0,
                id: MOCK_AUDIO_ID.into(),
                description: "FirefaceUSB - Fireface UCX".into(),
                has_hwdep: false,
            },
            SoundCard {
                number: 1,
                id: MOCK_CONTROL_ID.into(),
                description: "MockControl - control only".into(),
                has_hwdep: true,
            },
        ]
    }

    /// A wrapped mock control-only device, for the wrapper tests.
    pub fn mock_device() -> ControlOnly {
        let desc = mock_descriptor();
        let binding = pair(&desc, &mock_cards()).expect("mock cards pair");
        ControlOnly {
            backend: (desc.open)(&binding).expect("mock backend opens"),
            binding,
            kind: desc.kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::*;

    fn cards() -> Vec<SoundCard> {
        mock_cards()
    }

    #[test]
    fn pairing_finds_the_control_and_audio_cards() {
        let desc = mock_descriptor();
        let binding = pair(&desc, &cards()).expect("pairs");
        assert_eq!(binding.control_card, 1);
        assert_eq!(binding.audio_card, 0);
        assert_eq!(binding.audio_card_id, MOCK_AUDIO_ID);
    }

    #[test]
    fn pairing_needs_the_control_card_with_hwdep() {
        let desc = mock_descriptor();
        let mut cards = cards();
        // The control card without hwdep is not a control card.
        cards[1].has_hwdep = false;
        assert!(pair(&desc, &cards).is_none());
    }

    #[test]
    fn pairing_needs_the_audio_card() {
        let desc = mock_descriptor();
        let mut cards = cards();
        cards.retain(|c| c.id != MOCK_AUDIO_ID);
        assert!(pair(&desc, &cards).is_none());
    }

    #[test]
    fn the_audio_card_is_not_the_control_card() {
        let desc = mock_descriptor();
        // Only the control card present: no distinct audio card.
        let cards = vec![SoundCard {
            number: 1,
            id: MOCK_CONTROL_ID.into(),
            description: "control".into(),
            has_hwdep: true,
        }];
        assert!(pair(&desc, &cards).is_none());
    }

    #[test]
    fn description_matching_works() {
        let mut desc = mock_descriptor();
        desc.audio_card_match = CardMatch::DescriptionContains("Fireface UCX");
        let binding = pair(&desc, &cards()).expect("pairs by description");
        assert_eq!(binding.audio_card_id, MOCK_AUDIO_ID);
    }

    #[test]
    fn the_wrapper_reports_the_audio_card_not_the_control_card() {
        let dev = mock_device();
        assert_eq!(dev.audio_card_id(), MOCK_AUDIO_ID);
        assert_ne!(dev.audio_card_id(), MOCK_CONTROL_ID);
        assert_eq!(dev.binding.control_card, 1);
    }

    #[test]
    fn the_wrapper_forwards_the_backend() {
        let mut dev = mock_device();
        // The mock backend is a UCX: it has inputs and reports its model.
        assert!(!dev.backend.inputs().is_empty());
        assert_eq!(dev.backend.model_name(), "Fireface UCX (mock)");
        let before = dev.backend.volume(totalcontrol_core::ChannelId::Input(0), 0).unwrap();
        dev.backend
            .set_volume(totalcontrol_core::ChannelId::Input(0), 0, 0.5)
            .unwrap();
        let after = dev.backend.volume(totalcontrol_core::ChannelId::Input(0), 0).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn reading_cards_does_not_panic() {
        let _ = read_sound_cards();
    }
}