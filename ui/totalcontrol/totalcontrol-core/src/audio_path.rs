//! The host audio path: who owns the device clock, and TotalControl's
//! authority on it.
//!
//! Three paths share one device-centric model. PipeWire owns the PCM and
//! TotalControl either forces the rate (the Device pro path) or lets the graph
//! conduct (the convenience path). JACK (`jackd`) or a DAW opening `hw:`
//! directly owns the PCM instead; TotalControl reads the rate and never writes
//! it. With nobody holding the PCM the path is idle. The path is *detected*
//! from kernel facts (the PCM owner, `hw_params`, the stream epoch), never
//! configured: a setting expresses intent, reality is read back.
//!
//! This module is the pure heart of that model. It holds the fact snapshot,
//! the owner→path classification, the clock-authority decisions and the
//! scheduling policy as functions over an immutable snapshot, with no I/O, so
//! every decision is unit-tested without PipeWire. The effectful shell (the
//! kernel-fact collector, the bounded PipeWire backend and the latest-value
//! snapshot the GUI reads) lives in the GUI's audio-path service; the PipeWire
//! CLI transport stays in [`crate::pw_clock`].
//!
//! Device-centric invariant: TotalControl is never a PCM owner on a pro path.
//! It controls the mixer over hwdep and observes the path over procfs + hwdep.
//! Ownership (server/DAW), control (TotalControl hwdep) and observation
//! (TotalControl procfs) stay separate.

use crate::pw_clock::{CardStream, PwClock, SchedPolicy, DEVICE_SAMPLE_RATES};

/// How the graph rate is chosen when PipeWire owns the path. `Auto` lets the
/// graph conduct and the device follows it; `Fixed` forces the device-set rate.
/// This is TotalControl's own authority setting, independent of the detected
/// path: under an external owner it is a read-only report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum EngineRateMode {
    /// PipeWire decides (`clock.force-rate = 0`).
    #[default]
    #[serde(alias = "Follow")]
    Auto,
    /// Force the rate stored in the host's fixed-rate preference.
    Fixed,
}

/// Who holds the card's playback PCM, resolved from the kernel's `owner_pid`
/// through `/proc/<pid>/comm`. The classification follows the PCM owner, not a
/// server's presence: a DAW running on PipeWire's JACK emulation is a PipeWire
/// client and owns nothing, so the path stays PipeWire (the pipewire-jack trap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmOwner {
    /// Nobody holds the PCM (`hw_params` reads `closed`).
    Idle,
    /// `pipewire` / `pipewire-pulse`.
    PipeWire,
    /// `jackd` / `jackdbus`.
    Jack,
    /// Any other process: the DAW holds `hw:` directly.
    DirectAlsa,
    /// The PCM is open but `owner_pid` could not be resolved. Defensive only:
    /// every kernel that can run PipeWire prints `owner_pid`. The graph's Pro
    /// Audio nodes disambiguate this case in [`classify`].
    Unknown,
}

impl PcmOwner {
    /// Resolve an owner from the owning process's `comm`. `None` means the PCM
    /// is closed. Kernel `comm` is bounded to 15 characters; the server names
    /// fit.
    pub fn from_comm(comm: Option<&str>) -> Self {
        match comm {
            None => PcmOwner::Idle,
            Some(c) if c == "pipewire" || c == "pipewire-pulse" => PcmOwner::PipeWire,
            Some(c) if c == "jackd" || c == "jackdbus" => PcmOwner::Jack,
            Some(_) => PcmOwner::DirectAlsa,
        }
    }
}

/// The external PCM owner: JACK or direct ALSA. One behaviour (read the rate,
/// never write it, reapply on stream start / rate change); the labels differ
/// only in display and manual guidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalOwner {
    Jack,
    Alsa,
}

/// The detected audio path with TotalControl's authority on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPath {
    /// PipeWire owns the PCM; TotalControl forces the device-set rate (the
    /// Device pro path: IRQ scheduling + the period-aligned cushion).
    PwDevice,
    /// PipeWire owns the PCM; the graph conducts the rate (the convenience
    /// path: timer scheduling + a fixed generous margin).
    PwFollow,
    /// JACK or a direct-ALSA DAW owns the PCM; TotalControl reads the rate and
    /// writes nothing to the clock.
    External(ExternalOwner),
    /// Nobody holds the PCM. TotalControl applies nothing and waits; it
    /// classifies when a stream opens.
    Idle,
}

impl AudioPath {
    /// Whether PipeWire owns the PCM (either authority sub-mode).
    pub fn is_pipewire(self) -> bool {
        matches!(self, AudioPath::PwDevice | AudioPath::PwFollow)
    }

    /// Whether an external process (JACK or a DAW) owns the PCM.
    pub fn is_external(self) -> bool {
        matches!(self, AudioPath::External(_))
    }

    /// Whether TotalControl forces the graph rate on this path. Only the
    /// PipeWire Device path does; under an external owner the rate is read-only.
    pub fn tc_forces_rate(self) -> bool {
        matches!(self, AudioPath::PwDevice)
    }
}

/// Classify the path from the PCM owner, TotalControl's rate mode and (for the
/// defensive unknown-owner case) whether the graph has Pro Audio nodes. Pure:
/// the pipewire-jack trap classifies as PipeWire because the owner is PipeWire,
/// regardless of the DAW's own "JACK" label.
pub fn classify(
    owner: PcmOwner,
    mode: EngineRateMode,
    pro_nodes_present: Option<bool>,
) -> AudioPath {
    let pw = || match mode {
        EngineRateMode::Fixed => AudioPath::PwDevice,
        EngineRateMode::Auto => AudioPath::PwFollow,
    };
    match owner {
        PcmOwner::Idle => AudioPath::Idle,
        PcmOwner::PipeWire => pw(),
        PcmOwner::Jack => AudioPath::External(ExternalOwner::Jack),
        PcmOwner::DirectAlsa => AudioPath::External(ExternalOwner::Alsa),
        // Defensive fallback: Pro Audio nodes present → PipeWire; PCM open but
        // no nodes → an external owner (JACK-or-ALSA, indistinguishable, one
        // behaviour). The degradation is cosmetic, not behavioural.
        PcmOwner::Unknown => match pro_nodes_present {
            Some(true) => pw(),
            _ => AudioPath::External(ExternalOwner::Alsa),
        },
    }
}

/// The device's achievable rate/quantum envelope, device-agnostic. The driver
/// is the single owner of the supported rates and the per-rate period floor;
/// the GUI fills this from the driver's reported envelope (or the mock's
/// defaults) and the pure decisions read it.
#[derive(Debug, Clone, Default)]
pub struct RateCaps {
    rates: Vec<u32>,
    /// `(rate, period_floor)` per supported rate.
    floors: Vec<(u32, u32)>,
    /// The floor for a rate the driver did not report (the mock, or an absent
    /// envelope): the driver's global `urb_frames`, else a low default.
    fallback_floor: u32,
    /// ISO transport: the pitch envelope derives from the per-band packet
    /// capacity. Interrupt transports carry no per-microframe frame ceiling.
    iso: bool,
}

impl RateCaps {
    pub fn new(rates: Vec<u32>, floors: Vec<(u32, u32)>, fallback_floor: u32) -> Self {
        Self { rates, floors, fallback_floor: fallback_floor.max(1), iso: false }
    }

    /// Mark the transport ISO (the driver-reported envelope carries the flag).
    pub fn with_iso(mut self, iso: bool) -> Self {
        self.iso = iso;
        self
    }

    /// The supported rates, in the driver's order. Empty means "use the
    /// canonical device rate list" (the mock).
    pub fn rates(&self) -> &[u32] {
        &self.rates
    }

    /// The smallest period (one URB) the driver accepts at `rate`.
    pub fn period_floor(&self, rate: u32) -> u32 {
        self.floors
            .iter()
            .find(|(r, _)| *r == rate)
            .map(|(_, f)| *f)
            .unwrap_or(self.fallback_floor)
            .max(1)
    }
}

/// The immutable snapshot the pure clock decisions read: the kernel and graph
/// observations plus TotalControl's stored intent. One source of truth per
/// fact; every decision derives from this, never re-reads the world.
#[derive(Debug, Clone)]
pub struct ClockFacts {
    /// TotalControl's rate-authority setting.
    pub mode: EngineRateMode,
    /// The rate / quantum TotalControl forces (`0` = no override), kept in the
    /// host preferences and re-applied at launch.
    pub force_rate: u32,
    pub force_quantum: u32,
    /// The live PipeWire graph clock, when readable.
    pub clock: Option<PwClock>,
    /// The card's open playback stream (kernel `hw_params`), `None` when idle.
    pub stream: Option<CardStream>,
    /// The graph quantum the server actually runs (`pw-top`), when probed.
    pub graph_quantum: Option<u32>,
    /// The rate the unit reports it is set to (device state word `0x11`).
    pub device_rate: Option<u32>,
    /// Whether the device clock source is external (not Internal).
    pub clock_source_external: bool,
    /// The rate stored in the device settings (the model's sample rate).
    pub device_set_rate: u32,
    /// The device's achievable rate/quantum envelope.
    pub caps: RateCaps,
}

impl ClockFacts {
    /// The supported rates the GUI offers: the driver-reported envelope when
    /// present, otherwise the canonical device list (the mock).
    pub fn device_rates(&self) -> Vec<u32> {
        if self.caps.rates().is_empty() {
            DEVICE_SAMPLE_RATES.to_vec()
        } else {
            self.caps.rates().to_vec()
        }
    }

    /// The smallest graph quantum the driver can run at `rate` (one URB). The
    /// graph quantum is the ALSA period under IRQ scheduling and two periods
    /// under timer scheduling; either way the period floor is the quantum floor.
    pub fn quantum_floor(&self, rate: u32) -> u32 {
        self.caps.period_floor(rate)
    }

    /// The convenience-path quantum minimum: the highest period floor across
    /// the device's supported rates. A quantum at or above it is achievable at
    /// every rate, so a graph-conducted rate switch never leaves an impossible
    /// quantum.
    pub fn convenience_quantum_min(&self) -> u32 {
        self.device_rates()
            .iter()
            .map(|r| self.quantum_floor(*r))
            .max()
            .unwrap_or(1)
            .max(1)
    }

    /// The pitch envelope: the highest varispeed percent the transport carries
    /// at `rate`. The ISO endpoint delivers at most `max_frames` frames per
    /// microframe packet — the band's highest period floor / 8 + 1 — so the
    /// programmed clock `rate * (1 + pitch/100)` must stay at or below
    /// `8000 * max_frames`. The driver clamps every DDS write to the same
    /// ceiling (`ffuc_dds_clamp`); the slider offers the envelope so the
    /// displayed value is what the device runs. Non-ISO transports carry no
    /// per-microframe frame ceiling and keep the full ±5 % range.
    pub fn pitch_max_percent(&self, rate: u32) -> f32 {
        if !self.caps.iso || rate == 0 {
            return 5.0;
        }
        let band = |r: u32| if r <= 48_000 { 1 } else if r <= 96_000 { 2 } else { 4 };
        let band_floor = self
            .device_rates()
            .iter()
            .filter(|r| band(**r) == band(rate))
            .map(|r| self.quantum_floor(*r))
            .max()
            .unwrap_or_else(|| self.quantum_floor(rate));
        // 8000 * (band_floor / 8 + 1), the band's packet capacity in frames/s.
        let capacity = 1000 * band_floor + 8000;
        let max = (capacity as f64 / rate as f64 - 1.0) * 100.0;
        (max.min(5.0).max(0.0)) as f32
    }

    /// The zero-slack zone: the effective quantum below twice the rate floor.
    /// Nominal playback is clean there; varispeed can miss every follower
    /// cycle and silence playback. One period of slack (twice the floor)
    /// removes the penalty.
    pub fn zero_slack(&self, rate: u32) -> bool {
        let q = if self.force_quantum > 0 {
            self.force_quantum
        } else {
            self.graph_quantum.unwrap_or(0)
        };
        q > 0 && q < 2 * self.quantum_floor(rate)
    }

    /// The lowest quantum the current mode allows at `rate`. The Device path
    /// uses the rate's period floor; the convenience path also holds the
    /// convenience minimum so the quantum is safe at every graph rate.
    pub fn minimum_quantum(&self, rate: u32) -> u32 {
        let floor = self.quantum_floor(rate);
        if self.mode == EngineRateMode::Auto {
            floor.max(self.convenience_quantum_min())
        } else {
            floor
        }
    }

    /// The graph rate as it stands: a forced rate is what the graph is being
    /// told to run at even before the stream reopens; without one, the open
    /// card stream is the actual rate, then the default `clock.rate`, then the
    /// device's set rate.
    pub fn current_graph_rate(&self) -> u32 {
        self.clock
            .map(|c| c.force_rate)
            .filter(|r| *r > 0)
            .or_else(|| self.stream.map(|s| s.rate))
            .or_else(|| self.clock.map(|c| c.rate))
            .filter(|r| *r > 0)
            .unwrap_or(self.device_set_rate)
    }

    /// The rate Device mode forces: the device's set rate is the boss. Falls
    /// back to the stored host force, then the running graph rate.
    pub fn device_mode_rate(&self) -> u32 {
        if self.device_set_rate > 0 {
            self.device_set_rate
        } else if self.force_rate > 0 {
            self.force_rate
        } else {
            self.current_graph_rate()
        }
    }

    /// The rate Fixed mode forces and holds. On internal clock it is the user's
    /// stored fixed choice; slaved to an external clock the device's own
    /// reported rate is the authority and the readback wins.
    pub fn device_authority_rate(&self) -> u32 {
        if self.clock_source_external {
            if let Some(rate) = self.device_rate.filter(|r| *r > 0) {
                return rate;
            }
        }
        if self.force_rate > 0 {
            self.force_rate
        } else {
            self.device_mode_rate()
        }
    }

    /// The device rate hub: the rate the device is set to run at, from which
    /// the graph rate and the available quantum derive. A forced graph rate is
    /// the target device rate; without a force the actual device report / card
    /// stream is the hub. The idle status report alone is not used when a force
    /// is set: it lags at the last-running rate.
    pub fn running_rate(&self) -> u32 {
        self.clock
            .map(|c| c.force_rate)
            .filter(|r| *r > 0)
            .or_else(|| self.device_rate.filter(|r| *r > 0))
            .or_else(|| self.stream.map(|s| s.rate).filter(|r| *r > 0))
            .unwrap_or_else(|| self.current_graph_rate())
    }

    /// The period the scheduling cushion aligns to: the forced (or graph)
    /// quantum raised to the rate's floor. The cushion is placed *before* a
    /// deliberate rate/quantum change, so it aligns to the target quantum the
    /// PCM will re-prepare at, not the period the running stream still holds.
    pub fn cushion_period(&self, rate: u32) -> u32 {
        let quantum = if self.force_quantum > 0 {
            self.force_quantum
        } else {
            self.clock.map(|c| c.effective_quantum()).unwrap_or(0)
        };
        quantum.max(self.quantum_floor(rate)).max(1)
    }

    /// The scheduling policy for the current rate authority (the three-tier
    /// model). Device (Fixed) is the pro path: IRQ scheduling + a period-aligned
    /// cushion `H = 2·P`, placed before each deliberate rate/quantum change.
    /// PipeWire (Auto) is the convenience path: timer scheduling + a fixed
    /// generous margin the DLL servos — safe at every rate without tracking the
    /// graph-conducted rate. An external owner opens the PCM directly and uses
    /// neither (the caller applies no policy there).
    pub fn sched_policy(&self, rate: u32) -> SchedPolicy {
        if self.mode == EngineRateMode::Fixed {
            SchedPolicy {
                disable_tsched: true,
                headroom: cushion_headroom(self.cushion_period(rate)),
                start_delay: 0,
                period_num: 32,
            }
        } else {
            SchedPolicy {
                disable_tsched: false,
                headroom: 336,
                start_delay: 3 * self.quantum_floor(rate),
                period_num: 16,
            }
        }
    }

    /// The rate Fixed mode must re-assert, or `None` when the live force already
    /// holds the authority. A diverged force (a PipeWire restart cleared it, or
    /// another client wrote it) is re-asserted unless a user selection owns its
    /// confirmation window (`pending`) or a re-assert is still cooling down
    /// (`reforce_cooling`). Pure decision, testable without PipeWire.
    pub fn rate_authority_target(&self, pending: bool, reforce_cooling: bool) -> Option<u32> {
        if self.mode != EngineRateMode::Fixed {
            return None;
        }
        let desired = self.device_authority_rate();
        if desired == 0 || self.clock.map(|c| c.force_rate).unwrap_or(0) == desired {
            return None;
        }
        if pending || reforce_cooling {
            return None;
        }
        Some(desired)
    }
}

/// The scheduling cushion: `H = 2·P`, a whole multiple of the ALSA period.
/// PipeWire's IRQ-mode wake threshold must stay period-aligned: at fractional
/// rates a non-aligned target doubles the write-skip rate and collapses the
/// ring fill. Two periods give a comfortable startup margin. This is the
/// PipeWire equivalent of the reference host's safety offset; JACK never sees
/// it (it opens the PCM directly).
pub fn cushion_headroom(period: u32) -> u32 {
    period.saturating_mul(2)
}

/// A change between two observed card rates. `None` means "no open stream";
/// the last known rate is kept across that gap so a rate switch made while the
/// stream was briefly closed is still detected when it reopens.
pub fn card_rate_changed(last: Option<u32>, current: Option<u32>) -> bool {
    matches!((last, current), (Some(old), Some(new)) if old != new)
}

/// Whether the first observed card rate differs from the rate the device
/// reported when TotalControl opened it. The common startup case: the scene is
/// applied while the card is at its old rate, then the owner opens it at a
/// different rate and the unit reloads its stored mixer.
pub fn first_rate_needs_reapply(last: Option<u32>, current: Option<u32>, model_rate: u32) -> bool {
    last.is_none() && current.is_some_and(|rate| rate != model_rate)
}

/// Whether an observed card-rate transition needs a mixer scene reapply.
pub fn rate_needs_mixer_reapply(last: Option<u32>, current: Option<u32>, model_rate: u32) -> bool {
    card_rate_changed(last, current) || first_rate_needs_reapply(last, current, model_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_ucx() -> RateCaps {
        // The UCX envelope: per-rate period floors (one URB), 192 the highest.
        RateCaps::new(
            DEVICE_SAMPLE_RATES.to_vec(),
            vec![
                (32_000, 32), (44_100, 48), (48_000, 48), (64_000, 64),
                (88_200, 96), (96_000, 96), (128_000, 128),
                (176_400, 184), (192_000, 192),
            ],
            48,
        )
        .with_iso(true)
    }

    fn facts(mode: EngineRateMode) -> ClockFacts {
        ClockFacts {
            mode,
            force_rate: 0,
            force_quantum: 0,
            clock: None,
            stream: None,
            graph_quantum: None,
            device_rate: None,
            clock_source_external: false,
            device_set_rate: 48_000,
            caps: caps_ucx(),
        }
    }

    // ── Classification ──────────────────────────────────────────────

    #[test]
    fn owner_from_comm_resolves_the_servers() {
        assert_eq!(PcmOwner::from_comm(None), PcmOwner::Idle);
        assert_eq!(PcmOwner::from_comm(Some("pipewire")), PcmOwner::PipeWire);
        assert_eq!(PcmOwner::from_comm(Some("pipewire-pulse")), PcmOwner::PipeWire);
        assert_eq!(PcmOwner::from_comm(Some("jackd")), PcmOwner::Jack);
        assert_eq!(PcmOwner::from_comm(Some("jackdbus")), PcmOwner::Jack);
        assert_eq!(PcmOwner::from_comm(Some("reaper")), PcmOwner::DirectAlsa);
    }

    #[test]
    fn pipewire_owner_splits_by_authority_mode() {
        assert_eq!(
            classify(PcmOwner::PipeWire, EngineRateMode::Fixed, None),
            AudioPath::PwDevice
        );
        assert_eq!(
            classify(PcmOwner::PipeWire, EngineRateMode::Auto, None),
            AudioPath::PwFollow
        );
    }

    #[test]
    fn external_owners_are_read_only_paths() {
        assert_eq!(
            classify(PcmOwner::Jack, EngineRateMode::Fixed, None),
            AudioPath::External(ExternalOwner::Jack)
        );
        assert_eq!(
            classify(PcmOwner::DirectAlsa, EngineRateMode::Auto, None),
            AudioPath::External(ExternalOwner::Alsa)
        );
        assert_eq!(classify(PcmOwner::Idle, EngineRateMode::Fixed, None), AudioPath::Idle);
    }

    #[test]
    fn the_pipewire_jack_trap_classifies_as_pipewire() {
        // The DAW calls itself "JACK" but runs on PipeWire's emulation, so the
        // PCM owner is pipewire. Ownership, not the server's presence, decides.
        let path = classify(PcmOwner::PipeWire, EngineRateMode::Auto, Some(true));
        assert_eq!(path, AudioPath::PwFollow);
        assert!(path.is_pipewire());
        assert!(!path.is_external());
    }

    #[test]
    fn unknown_owner_falls_back_to_the_pro_nodes() {
        assert_eq!(
            classify(PcmOwner::Unknown, EngineRateMode::Fixed, Some(true)),
            AudioPath::PwDevice
        );
        assert!(classify(PcmOwner::Unknown, EngineRateMode::Auto, Some(false)).is_external());
        assert!(classify(PcmOwner::Unknown, EngineRateMode::Auto, None).is_external());
    }

    #[test]
    fn only_the_device_path_forces_the_rate() {
        assert!(AudioPath::PwDevice.tc_forces_rate());
        assert!(!AudioPath::PwFollow.tc_forces_rate());
        assert!(!AudioPath::External(ExternalOwner::Jack).tc_forces_rate());
        assert!(!AudioPath::External(ExternalOwner::Alsa).tc_forces_rate());
        assert!(!AudioPath::Idle.tc_forces_rate());
    }

    // ── Quantum floor / convenience minimum ─────────────────────────

    #[test]
    fn quantum_floor_comes_from_the_envelope() {
        let f = facts(EngineRateMode::Fixed);
        assert_eq!(f.quantum_floor(48_000), 48);
        assert_eq!(f.quantum_floor(176_400), 184);
        assert_eq!(f.quantum_floor(192_000), 192);
        // An unreported rate uses the fallback floor.
        assert_eq!(f.quantum_floor(22_050), 48);
    }

    #[test]
    fn convenience_minimum_is_the_highest_floor() {
        let f = facts(EngineRateMode::Auto);
        assert_eq!(f.convenience_quantum_min(), 192);
        // The convenience path holds the minimum at every rate.
        assert_eq!(f.minimum_quantum(48_000), 192);
        assert_eq!(f.minimum_quantum(192_000), 192);
        // The Device path uses only the rate's own floor.
        let d = facts(EngineRateMode::Fixed);
        assert_eq!(d.minimum_quantum(48_000), 48);
    }

    #[test]
    fn pitch_envelope_is_the_transport_capacity() {
        let f = facts(EngineRateMode::Fixed);
        // Every rate whose +5% clock fits the band capacity keeps the full
        // range: 1x 56000, 2x 104000, 4x 200000 frames/s.
        for rate in [32_000, 44_100, 48_000, 64_000, 88_200, 96_000, 128_000, 176_400] {
            assert_eq!(f.pitch_max_percent(rate), 5.0, "rate {rate}");
        }
        // 192k +5% programs 201600 > 200000: the ceiling is +4.1667%.
        let max = f.pitch_max_percent(192_000);
        assert!((max - 4.166667).abs() < 0.001, "192k max {max}");
        // The ceiling clock stays at or below the capacity.
        let clock = 192_000.0 * (1.0 + max as f64 / 100.0);
        assert!(clock <= 200_000.0, "ceiling clock {clock}");
        // A non-ISO transport keeps the full range at every rate.
        let mut interrupt = f.clone();
        interrupt.caps = RateCaps::new(
            interrupt.device_rates(),
            vec![(192_000, 1024)],
            48,
        );
        assert_eq!(interrupt.pitch_max_percent(192_000), 5.0);
        assert_eq!(f.pitch_max_percent(0), 5.0);
    }

    #[test]
    fn zero_slack_is_below_twice_the_floor() {
        let mut f = facts(EngineRateMode::Fixed);
        // 48k floor 48: the floor itself is zero-slack, 96 is safe.
        f.force_quantum = 48;
        assert!(f.zero_slack(48_000));
        f.force_quantum = 96;
        assert!(!f.zero_slack(48_000));
        // 192k floor 192: 256 is still zero-slack, 384 is safe.
        f.force_quantum = 256;
        assert!(f.zero_slack(192_000));
        f.force_quantum = 384;
        assert!(!f.zero_slack(192_000));
        // No force: the graph quantum decides; unknown is not warned.
        f.force_quantum = 0;
        f.graph_quantum = Some(192);
        assert!(f.zero_slack(192_000));
        f.graph_quantum = None;
        assert!(!f.zero_slack(192_000));
    }

    // ── Scheduling policy (three-tier) ──────────────────────────────

    #[test]
    fn device_policy_is_irq_plus_aligned_cushion() {
        let mut f = facts(EngineRateMode::Fixed);
        f.force_quantum = 96;
        let p = f.sched_policy(48_000);
        assert!(p.disable_tsched);
        assert_eq!(p.headroom, 192); // 2·P, P = max(96, floor 48)
        assert_eq!(p.start_delay, 0);
        assert_eq!(p.period_num, 32);
        // At 176.4k the cushion tracks the higher floor.
        assert_eq!(f.sched_policy(176_400).headroom, 368); // 2·184
    }

    #[test]
    fn convenience_policy_is_timer_plus_fixed_margin() {
        let f = facts(EngineRateMode::Auto);
        let p = f.sched_policy(48_000);
        assert!(!p.disable_tsched);
        assert_eq!(p.headroom, 336);
        assert_eq!(p.start_delay, 144); // 3·floor(48k) = 3·48
        assert_eq!(p.period_num, 16);
        assert_eq!(f.sched_policy(176_400).start_delay, 552); // 3·184
    }

    #[test]
    fn cushion_targets_the_forced_quantum_before_the_transition() {
        let mut f = facts(EngineRateMode::Fixed);
        f.force_quantum = 96;
        // The running stream still holds the old period; the cushion aligns to
        // the target quantum the PCM will re-prepare at, not the live period.
        f.stream = Some(CardStream { rate: 48_000, period: 48, buffer: 2048 });
        assert_eq!(f.cushion_period(48_000), 96);
        assert_eq!(f.sched_policy(48_000).headroom, 192);
        // Without a force, the graph quantum raised to the floor is the target.
        f.force_quantum = 0;
        assert_eq!(f.cushion_period(48_000), 48);
    }

    // ── Rate authority ──────────────────────────────────────────────

    #[test]
    fn device_authority_prefers_the_external_readback() {
        let mut f = facts(EngineRateMode::Fixed);
        f.force_rate = 48_000;
        f.clock_source_external = true;
        f.device_rate = Some(96_000);
        assert_eq!(f.device_authority_rate(), 96_000);
        // Internal clock: the stored fixed choice wins.
        f.clock_source_external = false;
        assert_eq!(f.device_authority_rate(), 48_000);
    }

    #[test]
    fn rate_authority_reasserts_only_a_diverged_force() {
        let mut f = facts(EngineRateMode::Fixed);
        f.force_rate = 48_000;
        f.clock = Some(PwClock {
            rate: 48_000, quantum: 256, force_rate: 0, force_quantum: 0,
            min_quantum: 0, max_quantum: 0,
        });
        // The live force was cleared (a PipeWire restart): re-assert.
        assert_eq!(f.rate_authority_target(false, false), Some(48_000));
        // The live force already holds: nothing to do.
        f.clock.as_mut().unwrap().force_rate = 48_000;
        assert_eq!(f.rate_authority_target(false, false), None);
        // A pending user selection owns its window.
        f.clock.as_mut().unwrap().force_rate = 0;
        assert_eq!(f.rate_authority_target(true, false), None);
        // A re-assert is cooling down.
        assert_eq!(f.rate_authority_target(false, true), None);
        // Auto mode never re-asserts.
        f.mode = EngineRateMode::Auto;
        assert_eq!(f.rate_authority_target(false, false), None);
    }

    #[test]
    fn running_rate_prefers_the_force_then_device_then_stream() {
        let mut f = facts(EngineRateMode::Auto);
        f.device_set_rate = 44_100;
        assert_eq!(f.running_rate(), 44_100); // falls to the graph/set rate
        f.stream = Some(CardStream { rate: 48_000, period: 48, buffer: 1024 });
        assert_eq!(f.running_rate(), 48_000);
        f.device_rate = Some(96_000);
        assert_eq!(f.running_rate(), 96_000);
        f.clock = Some(PwClock {
            rate: 48_000, quantum: 256, force_rate: 192_000, force_quantum: 0,
            min_quantum: 0, max_quantum: 0,
        });
        assert_eq!(f.running_rate(), 192_000);
    }

    // ── Reapply predicates ──────────────────────────────────────────

    #[test]
    fn rate_change_detection_keeps_the_last_rate_across_a_gap() {
        assert!(rate_needs_mixer_reapply(Some(48_000), Some(44_100), 48_000));
        assert!(!rate_needs_mixer_reapply(Some(48_000), Some(48_000), 48_000));
        // A brief close does not lose the switch.
        assert!(!rate_needs_mixer_reapply(Some(48_000), None, 48_000));
        assert!(rate_needs_mixer_reapply(Some(48_000), Some(96_000), 48_000));
        // First sight at a different rate than the model: the startup case.
        assert!(rate_needs_mixer_reapply(None, Some(44_100), 48_000));
        assert!(!rate_needs_mixer_reapply(None, Some(48_000), 48_000));
        assert!(!rate_needs_mixer_reapply(None, None, 48_000));
    }
}
