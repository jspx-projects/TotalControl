//! The PipeWire graph clock: sample rate and buffer size (the "quantum").
//!
//! With the kernel driver, the Babyface's rate and buffer are not the
//! mixer's to set: the sound server opens the card, so whatever PipeWire's
//! graph runs at is what the hardware runs at. These are the settings a
//! Windows user finds in RME's driver dialog (sample rate, ASIO buffer
//! size); on Linux they live in PipeWire's `settings` metadata, which
//! `pw-metadata` reads and writes at runtime:
//!
//! * `clock.rate` / `clock.quantum`: the defaults the graph uses;
//! * `clock.force-rate` / `clock.force-quantum`: overrides, `0` = none.
//!
//! The overrides are system-wide (every PipeWire client follows them) and
//! last until PipeWire restarts. `pw-metadata` ships with PipeWire, so this
//! needs no build dependency, like `pw_meter`'s use of `pw-record`.

use std::process::{Command, Stdio};

/// The sample-rate classes the supported Fireface and Babyface-classic
/// models use:
/// 1x, 2x and 4x for the 32, 44.1 and 48 kHz families.
pub const DEVICE_SAMPLE_RATES: [u32; 9] = [
    32_000, 44_100, 48_000, 64_000, 88_200, 96_000, 128_000, 176_400, 192_000,
];

/// The graph clock as PipeWire reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PwClock {
    pub rate: u32,
    pub quantum: u32,
    /// `0` = not forced.
    pub force_rate: u32,
    /// `0` = not forced.
    pub force_quantum: u32,
}

impl PwClock {
    /// What the graph runs at: the forced value if any, else the default.
    pub fn effective_rate(&self) -> u32 {
        if self.force_rate > 0 {
            self.force_rate
        } else {
            self.rate
        }
    }

    pub fn effective_quantum(&self) -> u32 {
        if self.force_quantum > 0 {
            self.force_quantum
        } else {
            self.quantum
        }
    }

    /// One buffer's worth of time, in milliseconds.
    pub fn latency_ms(&self) -> f32 {
        let rate = self.effective_rate();
        if rate == 0 {
            return 0.0;
        }
        self.effective_quantum() as f32 * 1000.0 / rate as f32
    }
}

/// Parses `pw-metadata -n settings 0` output, lines like
/// `update: id:0 key:'clock.rate' value:'48000' type:''`. `None` when the
/// two defaults are missing (no PipeWire, or not its settings object).
pub fn parse(output: &str) -> Option<PwClock> {
    let value_of = |key: &str| -> Option<u32> {
        let needle = format!("key:'{key}' value:'");
        let line = output.lines().find(|l| l.contains(&needle))?;
        let rest = &line[line.find(&needle)? + needle.len()..];
        rest[..rest.find('\'')?].trim().parse().ok()
    };
    Some(PwClock {
        rate: value_of("clock.rate")?,
        quantum: value_of("clock.quantum")?,
        force_rate: value_of("clock.force-rate").unwrap_or(0),
        force_quantum: value_of("clock.force-quantum").unwrap_or(0),
    })
}

/// Reads the live clock. `None` if PipeWire (or `pw-metadata`) is absent.
pub fn read() -> Option<PwClock> {
    let out = Command::new("pw-metadata")
        .args(["-n", "settings", "0"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// The graph's allowed sample-rate list, from `pw-metadata` output.
pub fn parse_allowed_rates(output: &str) -> Option<Vec<u32>> {
    let needle = "key:'clock.allowed-rates' value:'";
    let start = output.find(needle)? + needle.len();
    let rest = &output[start..];
    let end = rest.find('\'')?;
    let value = rest[..end].trim().trim_start_matches('[').trim_end_matches(']');
    let rates: Vec<u32> = value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse().ok())
        .collect();
    Some(rates)
}

/// Reads `clock.allowed-rates`.  `None` when PipeWire is absent or the key
/// is not present.
pub fn read_allowed_rates() -> Option<Vec<u32>> {
    let out = Command::new("pw-metadata")
        .args(["-n", "settings", "0"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_allowed_rates(&String::from_utf8_lossy(&out.stdout))
}

/// Replaces the graph's allowed-rate list at runtime.
pub fn set_allowed_rates(rates: &[u32]) -> Result<(), String> {
    let list = rates
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    set_value("clock.allowed-rates", &format!("[ {list} ]"))
}

/// Forces the graph rate; `0` lifts the override.
pub fn force_rate(rate: u32) -> Result<(), String> {
    set("clock.force-rate", rate)
}

/// Forces the graph quantum (buffer size in samples); `0` lifts it.
pub fn force_quantum(quantum: u32) -> Result<(), String> {
    set("clock.force-quantum", quantum)
}

fn set(key: &str, value: u32) -> Result<(), String> {
    set_value(key, &value.to_string())
}

fn set_value(key: &str, value: &str) -> Result<(), String> {
    let status = Command::new("pw-metadata")
        .args(["-n", "settings", "0", key, value])
        .output()
        .map_err(|e| format!("pw-metadata: {e}"))?
        .status;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pw-metadata {key} {value} failed ({status})"))
    }
}

/// What the card itself is running at, from the kernel's view of its open
/// stream (`/proc/asound/<card>/pcm0p/sub0/hw_params`, else the capture
/// side). This is the truth the graph clock cannot give: the sound server
/// may open the card with its own fixed period (WirePlumber's
/// `api.alsa.period-size`, IRQ mode), and then the card's latency does not
/// follow the quantum at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardStream {
    pub rate: u32,
    /// Frames per period: one hardware interrupt's worth.
    pub period: u32,
    /// Frames in the whole ring buffer.
    pub buffer: u32,
}

impl CardStream {
    /// One period's worth of time, in milliseconds.
    pub fn period_ms(&self) -> f32 {
        if self.rate == 0 {
            return 0.0;
        }
        self.period as f32 * 1000.0 / self.rate as f32
    }

    /// The whole ALSA ring buffer's worth of time, in milliseconds.
    pub fn buffer_ms(&self) -> f32 {
        if self.rate == 0 {
            return 0.0;
        }
        self.buffer as f32 * 1000.0 / self.rate as f32
    }
}

/// The smallest period the kernel driver accepts, read from the loaded
/// transport module.  `None` when no RME transport module is loaded.
pub fn driver_period_floor() -> Option<u32> {
    for path in ["/sys/module/snd_fireface_uc/parameters/urb_frames"] {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(value) = text.trim().parse() {
                return Some(value);
            }
        }
    }
    None
}

/// The UC's USB queue: `(frames per URB, URBs in flight)`.  `None` when the
/// Fireface UC transport module is not loaded.
pub fn driver_urb_queue() -> Option<(u32, u32)> {
    let frames = std::fs::read_to_string("/sys/module/snd_fireface_uc/parameters/urb_frames")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let urbs = std::fs::read_to_string("/sys/module/snd_fireface_uc/parameters/nurbs")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some((frames, urbs))
}

/// The UCX interrupt-transport URB size (`ucx_if5_frames`), when the module
/// is loaded.
pub fn driver_if5_frames() -> Option<u32> {
    std::fs::read_to_string("/sys/module/snd_fireface_uc/parameters/ucx_if5_frames")
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The driver's per-rate period floor: one URB (`ffuc_rule_period`).
///
/// ISO packs `iso_packets` microframes per URB. The interrupt transport scales
/// `urb_frames` by the rate band (UC) or uses the fixed `if5_frames` (UCX).
/// Mirrors `ffuc_urb_frames` in `kernel/ffuc.c`; the graph quantum follows this
/// floor when `api.alsa.disable-tsched` makes the quantum one ALSA period.
pub fn period_floor(rate: u32, urb_frames: u32, if5_frames: u32, iso: bool, ucx: bool) -> u32 {
    let rate = rate.max(1);
    if iso {
        let packets = urb_frames.next_multiple_of(8).clamp(8, 256).div_ceil(6).clamp(1, 8);
        packets * rate.div_ceil(8000)
    } else if ucx {
        if5_frames.next_multiple_of(8).clamp(8, 1024)
    } else {
        let base = urb_frames.next_multiple_of(8).clamp(8, 256);
        let band = if rate <= 48_000 { 0 } else if rate <= 96_000 { 1 } else { 2 };
        base << band
    }
}

/// The driver's per-rate ALSA ring minimum (`ffuc_rule_buffer`), URB-aligned.
/// It covers the in-flight URBs plus margin; the UCX also covers the output
/// lead plus two URBs. Mirrors `ffuc_rule_buffer` in `kernel/ffuc.c`.
pub fn buffer_min(rate: u32, urb_frames: u32, nurbs: u32, if5_frames: u32, iso: bool, ucx: bool) -> u32 {
    let rate = rate.max(1);
    let urb = period_floor(rate, urb_frames, if5_frames, iso, ucx);
    let base = if ucx && !iso {
        if5_frames.next_multiple_of(8).clamp(8, 1024)
    } else {
        urb_frames.next_multiple_of(8).clamp(8, 256)
    };
    let packets = base.div_ceil(6).clamp(1, 8);
    let nurbs = nurbs.max(2);
    let mut min = (nurbs + 2) * urb;
    if ucx {
        let fb_delay = (nurbs + 1) * packets;
        let lead = (fb_delay * rate + 4000) / 8000 + 60;
        min = min.max(lead + 2 * urb);
    }
    min.next_multiple_of(urb)
}

/// Whether the RME Pro Audio sink is actually running a playback stream.
/// `pactl list short sinks` reports the node state in its last field.
pub fn fireface_sink_running() -> bool {
    let Ok(out) = Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        line.contains("RME")
            && line.contains("pro-output-0")
            && line.split_whitespace().last() == Some("RUNNING")
    })
}

/// Name of the RME Pro Audio sink/source node.
fn fireface_node(kind: &str, suffix: &str) -> Option<String> {
    let out = Command::new("pactl")
        .args(["list", "short", kind])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
        if !line.contains("RME") || !line.contains(suffix) || line.contains(".monitor") {
            return None;
        }
        line.split_whitespace().nth(1).map(str::to_owned)
    })
}

pub fn fireface_sink_name() -> Option<String> {
    fireface_node("sinks", "pro-output-0")
}

pub fn fireface_source_name() -> Option<String> {
    fireface_node("sources", "pro-input-0")
}

/// The host (PipeWire/Pulse) volume of the RME Pro Audio sink, as percent
/// and dB.  This is a separate attenuation before the device: a sink at
/// 35 % feeds the interface at -27 dB even when the player sends a
/// full-scale CD.  Shown in the device sheet so the meters make sense.
pub fn fireface_sink_volume() -> Option<(u32, f32)> {
    let name = fireface_sink_name()?;
    let out = Command::new("pactl")
        .args(["get-sink-volume", &name])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split('/');
    let _channel = parts.next()?;
    let percent = parts.next()?.trim().trim_end_matches('%').trim().parse().ok()?;
    let db = parts
        .next()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some((percent, db))
}

/// Set the RME Pro Audio sink to unity (0 dB, 100 %).  The interface's own
/// faders remain the monitoring level.
pub fn set_fireface_sink_unity() -> bool {
    let Some(name) = fireface_sink_name() else {
        return false;
    };
    Command::new("pactl")
        .args(["set-sink-volume", &name, "100%"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn set_suspended(command: &str, node: &str, suspended: bool) -> bool {
    Command::new("pactl")
        .args([command, node, if suspended { "1" } else { "0" }])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn set_sink_suspended(node: &str, suspended: bool) -> bool {
    set_suspended("suspend-sink", node, suspended)
}

pub fn set_source_suspended(node: &str, suspended: bool) -> bool {
    set_suspended("suspend-source", node, suspended)
}

/// Parses a `hw_params` file. `None` for "closed" (the card is idle).
pub fn parse_hw_params(text: &str) -> Option<CardStream> {
    let field = |name: &str| -> Option<u32> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        line[name.len()..]
            .trim_start_matches(':')
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    Some(CardStream {
        rate: field("rate")?,
        period: field("period_size")?,
        buffer: field("buffer_size")?,
    })
}

/// The open *playback* stream of the ALSA card whose `id` is `card_id`.
/// `None` when no such card exists or nothing is playing. Playback only: the
/// capture side is held open by TotalControl's own input meter, so
/// reading it would report the app's own stream
/// (it once showed the meter's period as the card's).
pub fn card_stream(card_id: &str) -> Option<CardStream> {
    let cards = std::fs::read_dir("/proc/asound").ok()?;
    for entry in cards.flatten() {
        let dir = entry.path();
        let is_card = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("card"));
        let id = std::fs::read_to_string(dir.join("id")).unwrap_or_default();
        if !is_card || id.trim() != card_id {
            continue;
        }
        let text = std::fs::read_to_string(dir.join("pcm0p/sub0/hw_params")).ok()?;
        return parse_hw_params(&text);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Found \"settings\" metadata 31
update: id:0 key:'log.level' value:'2' type:''
update: id:0 key:'clock.rate' value:'48000' type:''
update: id:0 key:'clock.allowed-rates' value:'[ 48000 ]' type:''
update: id:0 key:'clock.quantum' value:'64' type:''
update: id:0 key:'clock.min-quantum' value:'32' type:''
update: id:0 key:'clock.max-quantum' value:'2048' type:''
update: id:0 key:'clock.force-quantum' value:'0' type:''
update: id:0 key:'clock.force-rate' value:'0' type:''
";

    #[test]
    fn parses_real_pw_metadata_output() {
        let c = parse(SAMPLE).unwrap();
        assert_eq!(
            c,
            PwClock {
                rate: 48_000,
                quantum: 64,
                force_rate: 0,
                force_quantum: 0
            }
        );
        // `clock.quantum` must not be confused with `clock.min-quantum`.
        assert_eq!(c.effective_quantum(), 64);
        assert!((c.latency_ms() - 1.333).abs() < 0.01);
    }

    #[test]
    fn forced_values_win() {
        let c = parse(
            &SAMPLE
                .replace(
                    "clock.force-quantum' value:'0'",
                    "clock.force-quantum' value:'256'",
                )
                .replace(
                    "clock.force-rate' value:'0'",
                    "clock.force-rate' value:'96000'",
                ),
        )
        .unwrap();
        assert_eq!(c.effective_rate(), 96_000);
        assert_eq!(c.effective_quantum(), 256);
    }

    /// Against the live PipeWire: run by hand (`--ignored`). Only ever
    /// writes the values already in place (`0` = no override when none is
    /// set), so it changes nothing on the machine it runs on.
    #[test]
    #[ignore]
    fn live_read_and_neutral_write() {
        let before = read().expect("PipeWire is running");
        assert!(before.effective_rate() > 0 && before.effective_quantum() > 0);
        force_quantum(before.force_quantum).unwrap();
        force_rate(before.force_rate).unwrap();
        assert_eq!(read(), Some(before));
    }

    #[test]
    fn parses_a_running_cards_hw_params() {
        let text = "access: MMAP_INTERLEAVED\nformat: S32_LE\nsubformat: STD\n\
                    channels: 12\nrate: 48000 (48000/1)\nperiod_size: 256\n\
                    buffer_size: 512\n";
        let s = parse_hw_params(text).unwrap();
        assert_eq!(
            s,
            CardStream {
                rate: 48_000,
                period: 256,
                buffer: 512
            }
        );
        assert!((s.period_ms() - 5.333).abs() < 0.01);
        assert!((s.buffer_ms() - 10.667).abs() < 0.01);
        assert_eq!(parse_hw_params("closed\n"), None);
    }

    #[test]
    fn parses_and_writes_the_allowed_rate_list() {
        assert_eq!(
            parse_allowed_rates(SAMPLE),
            Some(vec![48_000])
        );
        assert_eq!(
            parse_allowed_rates(
                "update: id:0 key:'clock.allowed-rates' value:'[ 32000 44100 48000 96000 ]' type:''"
            ),
            Some(vec![32_000, 44_100, 48_000, 96_000])
        );
        assert_eq!(parse_allowed_rates(""), None);
    }

    #[test]
    fn no_pipewire_settings_is_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("Found \"settings\" metadata 31\n"), None);
    }

    // Contract with `ffuc_rule_period` / `ffuc_rule_buffer` (kernel/ffuc.c).
    // The ISO column is the loaded default: urb_frames=48, nurbs=2.
    #[test]
    fn iso_period_floor_is_one_urb_per_rate() {
        let cases = [
            (32_000, 32),
            (44_100, 48),
            (48_000, 48),
            (64_000, 64),
            (88_200, 96),
            (96_000, 96),
            (128_000, 128),
            (176_400, 184),
            (192_000, 192),
        ];
        for (rate, floor) in cases {
            assert_eq!(period_floor(rate, 48, 1024, true, true), floor, "rate {rate}");
        }
        // A requested 48 at 96 kHz is below the floor; the driver clamps to 96.
        assert!(period_floor(96_000, 48, 1024, true, true) > 48);
    }

    #[test]
    fn iso_buffer_min_covers_lead_and_margin() {
        // 336 is the capture test buffer (work note §12); 576 at 96 kHz.
        assert_eq!(buffer_min(44_100, 48, 2, 1024, true, true), 288);
        assert_eq!(buffer_min(48_000, 48, 2, 1024, true, true), 336);
        assert_eq!(buffer_min(96_000, 48, 2, 1024, true, true), 576);
        assert_eq!(buffer_min(88_200, 48, 2, 1024, true, true), 576);
        assert_eq!(buffer_min(176_400, 48, 2, 1024, true, true), 1104);
        assert_eq!(buffer_min(192_000, 48, 2, 1024, true, true), 1152);
    }

    #[test]
    fn interrupt_floor_scales_by_band_or_is_fixed() {
        // UC interrupt: urb_frames shifted by the rate band.
        assert_eq!(period_floor(48_000, 48, 1024, false, false), 48);
        assert_eq!(period_floor(96_000, 48, 1024, false, false), 96);
        assert_eq!(period_floor(192_000, 48, 1024, false, false), 192);
        // UCX interrupt: the fixed IF5 URB at every rate.
        assert_eq!(period_floor(44_100, 48, 1024, false, true), 1024);
        assert_eq!(period_floor(192_000, 48, 1024, false, true), 1024);
    }
}
