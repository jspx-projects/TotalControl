//! Device discovery: the USB MIDI ports a controller can live on.
//! See `TOTALCONTROL-CONTROLLER-SPEC.md` §5.

use std::path::PathBuf;

/// One rawmidi device of a card, identified for persistence by card id and
/// device index. The card number is resolved at open time and never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiDevice {
    pub card_id: String,
    pub card: u32,
    pub midi: u32,
    pub description: String,
    pub is_usb: bool,
}

impl MidiDevice {
    /// The display string of §5.1, `Card Name — MIDI 1`.
    pub fn display(&self) -> String {
        let name = self
            .description
            .rsplit(" - ")
            .next()
            .unwrap_or(&self.description);
        format!("{name} — MIDI {}", self.midi + 1)
    }

    /// The persistence key: card id and rawmidi index.
    pub fn key(&self) -> String {
        format!("{}:{}", self.card_id, self.midi)
    }

    pub fn port(&self) -> crate::mackie::MidiPort {
        crate::mackie::MidiPort { card: self.card, device: self.midi }
    }
}

/// Every rawmidi device of every card, USB cards flagged. Reads
/// `/proc/asound/cards` and the per-card midi nodes; a card is USB when its
/// device path resolves under a USB bus.
pub fn all_devices() -> Vec<MidiDevice> {
    let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in cards.lines() {
        // " 2 [FirefaceUC     ]: FirefaceUSB - Fireface UC"
        let Some(open) = line.find('[') else { continue };
        let Some(close) = line.find(']') else { continue };
        let card_id = line[open + 1..close].trim().to_string();
        let Ok(card) = line[..open].trim().parse::<u32>() else {
            continue;
        };
        let description = line[close + 1..]
            .trim_start_matches(':')
            .trim()
            .to_string();
        let is_usb = std::fs::read_link(format!("/sys/class/sound/card{card}/device"))
            .map(|p| p.to_string_lossy().contains("usb"))
            .unwrap_or(false);
        for midi in 0..8u32 {
            let node = PathBuf::from(format!("/proc/asound/card{card}/midi{midi}"));
            if !node.exists() {
                break;
            }
            out.push(MidiDevice {
                card_id: card_id.clone(),
                card,
                midi,
                description: description.clone(),
                is_usb,
            });
        }
    }
    out
}

/// The devices the controller listbox shows: every USB MIDI port plus the
/// open unit's own ports.
pub fn controller_devices(unit_card_id: &str) -> Vec<MidiDevice> {
    all_devices()
        .into_iter()
        .filter(|d| d.is_usb || d.card_id == unit_card_id)
        .collect()
}

/// Protocol preselection (§5.3), rules 2 and 3: file name equals the card
/// id, then a `match` fragment of the description.
pub fn preselect<'a>(
    device: &MidiDevice,
    protocols: &'a [super::load::LoadedProtocol],
) -> Option<&'a str> {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let card_norm = normalize(&device.card_id);
    for p in protocols {
        let Some(map) = &p.map else { continue };
        let file_norm = normalize(&p.file_name.trim_end_matches(".json"));
        if !card_norm.is_empty() && file_norm == card_norm {
            return Some(&map.id);
        }
    }
    // The definition's own port match.
    for p in protocols {
        let Some(map) = &p.map else { continue };
        let Some(pm) = &map.port.match_ else { continue };
        if pm.card_id.as_deref() == Some(device.card_id.as_str()) {
            return Some(&map.id);
        }
        if pm
            .description
            .as_deref()
            .map(|d| device.description.contains(d))
            .unwrap_or(false)
        {
            return Some(&map.id);
        }
    }
    for p in protocols {
        let Some(map) = &p.map else { continue };
        if map
            .match_fragments
            .iter()
            .any(|frag| device.description.contains(frag.as_str()))
        {
            return Some(&map.id);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_key() {
        let d = MidiDevice {
            card_id: "ARC".into(),
            card: 3,
            midi: 0,
            description: "ARC - ARC USB".into(),
            is_usb: true,
        };
        assert_eq!(d.display(), "ARC USB — MIDI 1");
        assert_eq!(d.key(), "ARC:0");
    }

    #[test]
    fn enumeration_survives_a_missing_proc() {
        // On a host without /proc/asound the list is empty, never a panic.
        let _ = all_devices();
    }
}