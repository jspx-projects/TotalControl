//! The audio-path service: the effectful shell around the pure audio-path
//! model. It owns every sound-server call so the UI thread never blocks on one
//! (R4 / C11), and it keeps kernel facts flowing even when PipeWire stalls.
//!
//! Two independent loops publish into one latest-value [`EngineSnapshot`]:
//!
//! - a **kernel-fact collector** reads the PCM owner, `hw_params` and the ALSA
//!   delay from procfs on a fast cadence. These are short kernel reads, never a
//!   server call, so they continue while PipeWire is stuck;
//! - a **PipeWire worker** executes engine intents (force rate/quantum, set the
//!   scheduling policy, re-prepare, allowed-rates, sink volume) and the graph
//!   reads. Every call is bounded by the `pw_clock` timeout; a stall delays only
//!   the graph facts, never the kernel facts or the mixer (the device service
//!   is independent).
//!
//! The GUI reads the snapshot on its own cadence and issues intents only; it
//! never waits on a server or hardware operation. This mirrors the device
//! service's `MeterSnapshot` pattern.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use totalcontrol_core::audio_path::PcmOwner;
use totalcontrol_core::pw_clock::{self, CardStream, PwClock, SchedPolicy};

/// The latest-value engine snapshot. The kernel fields are refreshed by the
/// collector independently of the graph fields, so a PipeWire stall leaves the
/// kernel facts live. `None` / stale graph fields are honest unknowns, never a
/// fabricated value.
#[derive(Debug, Clone)]
pub struct EngineSnapshot {
    /// Who holds the playback PCM (kernel `owner_pid` → `comm`).
    pub owner: PcmOwner,
    /// The owning process's `comm`, for the "Controlled by <owner>" tooltip
    /// on a grayed control. `None` when idle or unresolved.
    pub owner_comm: Option<String>,
    /// The open playback stream (`hw_params`), `None` when idle.
    pub stream: Option<CardStream>,
    /// `snd_pcm_delay` (ring fill + in-flight URBs), frames.
    pub delay: Option<u32>,
    /// The ring fill (`appl_ptr − hw_ptr`), frames.
    pub fill: Option<u32>,
    /// The PCM state (`RUNNING`, `XRUN`, …), empty when closed.
    pub pcm_state: String,
    /// When the kernel facts were last read.
    pub kernel_at: Option<Instant>,
    /// The live PipeWire graph clock, when readable.
    pub clock: Option<PwClock>,
    /// The graph quantum the server actually runs (`pw-top`), when probed.
    pub graph_quantum: Option<u32>,
    /// Whether the RME Pro Audio sink is running a playback stream.
    pub sink_running: bool,
    /// The Fireface sink's host volume (percent, dB).
    pub host_volume: Option<(u32, f32)>,
    /// When the graph facts were last read.
    pub pw_at: Option<Instant>,
    /// Re-prepare completion fact: a generation counter bumped after every
    /// `Reprepare` intent runs, and whether that re-prepare confirmed the forced
    /// engine state. The GUI gates a pitched-clock write on a generation newer
    /// than the one it recorded when it requested the re-prepare, so the pitch
    /// only lands after IRQ scheduling is established (the fresh-start
    /// precondition).
    pub reprepare_gen: u64,
    pub reprepare_ok: bool,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self {
            owner: PcmOwner::Idle,
            owner_comm: None,
            stream: None,
            delay: None,
            fill: None,
            pcm_state: String::new(),
            kernel_at: None,
            clock: None,
            graph_quantum: None,
            sink_running: false,
            host_volume: None,
            pw_at: None,
            reprepare_gen: 0,
            reprepare_ok: false,
        }
    }
}

/// An engine operation for the PipeWire worker. The GUI computes the decision
/// (the pure audio-path model) and sends the concrete intent; the worker only
/// executes it off the UI thread.
#[derive(Debug, Clone)]
pub enum Intent {
    /// Read the graph clock and sink probes. `probe_quantum` requests the
    /// `pw-top` graph quantum (the truthful quantum confirmation).
    Poll { probe_quantum: bool },
    ForceRate(u32),
    ForceQuantum(u32),
    /// Apply a scheduling policy to both Pro Audio nodes (the worker reads the
    /// node ids; no nodes is a no-op).
    SetSchedPolicy(SchedPolicy),
    /// Apply the pending rate/quantum forces even when the graph is idle: a
    /// brief capture activation held open until the device confirms the forced
    /// state (a suspended node never applies a force on its own).
    Reprepare,
    /// Replace `clock.allowed-rates`, saving the previous value.
    ApplyAllowedRates(Vec<u32>),
    /// Restore the saved allowed-rates if the list is still the one we wrote.
    RestoreAllowedRates,
    /// Release the rate/quantum overrides, give the packaged convenience
    /// default back and restore the allowed-rates (a clean exit).
    Release,
    /// Set the Fireface sink to unity (0 dB, 100 %).
    SetSinkUnity,
}

/// The bounded intent inbox. Writes queue in order (a policy must land before
/// the force that follows it); a pending `Poll` is coalesced to the latest, so
/// a stalled worker does not accumulate a backlog of reads.
struct IntentInbox {
    queue: Mutex<VecDeque<Intent>>,
    cv: Condvar,
    stop: AtomicBool,
}

const INTENT_CAP: usize = 64;

impl IntentInbox {
    fn new() -> Self {
        Self { queue: Mutex::new(VecDeque::new()), cv: Condvar::new(), stop: AtomicBool::new(false) }
    }

    fn send(&self, intent: Intent) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        let mut q = self.queue.lock().unwrap();
        if matches!(intent, Intent::Poll { .. }) {
            if let Some(pos) = q.iter().position(|i| matches!(i, Intent::Poll { .. })) {
                q[pos] = intent;
                self.cv.notify_one();
                return true;
            }
        }
        while q.len() >= INTENT_CAP {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            q = self.cv.wait(q).unwrap();
        }
        q.push_back(intent);
        self.cv.notify_one();
        true
    }

    fn recv_timeout(&self, timeout: Duration) -> Option<Intent> {
        let mut q = self.queue.lock().unwrap();
        if q.is_empty() && !self.stop.load(Ordering::Relaxed) {
            q = self.cv.wait_timeout(q, timeout).unwrap().0;
        }
        let intent = q.pop_front();
        if intent.is_some() {
            self.cv.notify_all();
        }
        intent
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cv.notify_all();
    }
}

/// A running audio-path service.
pub struct AudioPathService {
    inbox: Arc<IntentInbox>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
    stop: Arc<AtomicBool>,
    joins: Vec<JoinHandle<()>>,
}

impl AudioPathService {
    /// Start the kernel-fact collector and the PipeWire worker for `card_id`.
    pub fn spawn(card_id: String) -> Self {
        let inbox = Arc::new(IntentInbox::new());
        let snapshot = Arc::new(Mutex::new(EngineSnapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let kernel_snapshot = Arc::clone(&snapshot);
        let kernel_stop = Arc::clone(&stop);
        let kernel_card = card_id.clone();
        let kernel_join = std::thread::Builder::new()
            .name("totalcontrol-engine-facts".into())
            .spawn(move || collect_kernel_facts(&kernel_card, &kernel_snapshot, &kernel_stop))
            .expect("starting the engine-fact collector");

        let worker_inbox = Arc::clone(&inbox);
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_stop = Arc::clone(&stop);
        let worker_card = card_id.clone();
        let worker_join = std::thread::Builder::new()
            .name("totalcontrol-engine-pw".into())
            .spawn(move || run_pw_worker(&worker_inbox, &worker_snapshot, &worker_stop, &worker_card))
            .expect("starting the engine PipeWire worker");

        Self { inbox, snapshot, stop, joins: vec![kernel_join, worker_join] }
    }

    /// The current engine snapshot (a cheap, non-blocking copy).
    pub fn snapshot(&self) -> EngineSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Queue an engine intent. Returns false once the service has stopped.
    pub fn send(&self, intent: Intent) -> bool {
        self.inbox.send(intent)
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.inbox.request_stop();
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }
}

impl Drop for AudioPathService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The kernel-fact collector: procfs only, never a server call. It keeps the
/// PCM owner, stream and delay live even when PipeWire is stuck.
fn collect_kernel_facts(card_id: &str, snapshot: &Arc<Mutex<EngineSnapshot>>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let status = pw_clock::card_pcm_status(card_id);
        let stream = pw_clock::card_stream(card_id);
        let comm = status
            .as_ref()
            .and_then(|s| s.owner_pid)
            .and_then(pw_clock::process_comm);
        let owner = match &status {
            None => PcmOwner::Idle,
            Some(s) => match s.owner_pid {
                Some(_) => PcmOwner::from_comm(comm.as_deref()),
                None => PcmOwner::Unknown,
            },
        };
        {
            let mut snap = snapshot.lock().unwrap();
            snap.owner = owner;
            snap.owner_comm = comm;
            snap.stream = stream;
            snap.delay = status.as_ref().and_then(|s| s.delay);
            snap.fill = status.as_ref().and_then(|s| s.fill);
            snap.pcm_state = status.map(|s| s.state).unwrap_or_default();
            snap.kernel_at = Some(Instant::now());
        }
        // A fast cadence: the kernel facts are the live path under a stall.
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// The PipeWire worker: executes intents and the graph reads, every call
/// bounded by the `pw_clock` timeout. A stall here delays only the graph facts.
fn run_pw_worker(inbox: &Arc<IntentInbox>, snapshot: &Arc<Mutex<EngineSnapshot>>, stop: &AtomicBool, card_id: &str) {
    // The allowed-rates ownership state lives here, off the UI thread.
    let mut allowed_saved: Option<Vec<u32>> = None;
    let mut allowed_written: Option<Vec<u32>> = None;

    while !stop.load(Ordering::Relaxed) {
        let Some(intent) = inbox.recv_timeout(Duration::from_millis(250)) else {
            continue;
        };
        match intent {
            Intent::Poll { probe_quantum } => {
                let clock = pw_clock::read();
                let sink_running = pw_clock::fireface_sink_running();
                let host_volume = pw_clock::fireface_sink_volume();
                let graph_quantum = if probe_quantum { pw_clock::graph_quantum() } else { None };
                let mut snap = snapshot.lock().unwrap();
                snap.clock = clock;
                snap.sink_running = sink_running;
                if host_volume.is_some() {
                    snap.host_volume = host_volume;
                }
                if let Some(q) = graph_quantum {
                    snap.graph_quantum = Some(q);
                }
                snap.pw_at = Some(Instant::now());
            }
            Intent::ForceRate(rate) => {
                if let Err(e) = pw_clock::force_rate(rate) {
                    log::warn!("Setting the PipeWire rate failed: {e}");
                }
            }
            Intent::ForceQuantum(q) => {
                if let Err(e) = pw_clock::force_quantum(q) {
                    log::warn!("Setting the PipeWire quantum failed: {e}");
                }
            }
            Intent::SetSchedPolicy(policy) => {
                if let Some((out, inp)) = pw_clock::fireface_pro_node_ids() {
                    if pw_clock::set_sched_policy(out, &policy).is_ok() {
                        let _ = pw_clock::set_sched_policy(inp, &policy);
                    }
                }
            }
            Intent::Reprepare => {
                // Apply the forces even when the graph is idle: a brief capture
                // activation, held until the device confirms the forced state.
                let ok = pw_clock::capture_reprepare(card_id);
                if !ok {
                    log::warn!("capture re-prepare did not confirm the forced engine state");
                }
                // Publish the completion so the GUI can sequence a pitched-clock
                // write after the re-prepare that establishes IRQ scheduling.
                let mut snap = snapshot.lock().unwrap();
                snap.reprepare_gen += 1;
                snap.reprepare_ok = ok;
            }
            Intent::ApplyAllowedRates(wanted) => {
                if wanted.is_empty() {
                    continue;
                }
                if let Some(current) = pw_clock::read_allowed_rates() {
                    let we_own_current = allowed_written.as_ref() == Some(&current);
                    if !we_own_current {
                        allowed_saved = Some(current.clone());
                    }
                    if current != wanted {
                        if let Err(e) = pw_clock::set_allowed_rates(&wanted) {
                            log::warn!("Setting the PipeWire allowed rates failed: {e}");
                            continue;
                        }
                    }
                    allowed_written = Some(wanted);
                }
            }
            Intent::RestoreAllowedRates => {
                if let (Some(written), Some(saved)) = (allowed_written.take(), allowed_saved.take()) {
                    if pw_clock::read_allowed_rates().as_ref() == Some(&written) {
                        if let Err(e) = pw_clock::set_allowed_rates(&saved) {
                            log::warn!("Restoring the PipeWire allowed rates failed: {e}");
                        }
                    }
                }
            }
            Intent::Release => {
                if let Err(e) = pw_clock::force_rate(0) {
                    log::warn!("Releasing the PipeWire rate override failed: {e}");
                }
                if let Err(e) = pw_clock::force_quantum(0) {
                    log::warn!("Releasing the PipeWire buffer override failed: {e}");
                }
                if let Some((out, inp)) = pw_clock::fireface_pro_node_ids() {
                    let convenience = SchedPolicy {
                        disable_tsched: false,
                        headroom: 336,
                        start_delay: 0,
                        period_num: 16,
                    };
                    let _ = pw_clock::set_sched_policy(out, &convenience);
                    let _ = pw_clock::set_sched_policy(inp, &convenience);
                }
                if let (Some(written), Some(saved)) = (allowed_written.take(), allowed_saved.take()) {
                    if pw_clock::read_allowed_rates().as_ref() == Some(&written) {
                        let _ = pw_clock::set_allowed_rates(&saved);
                    }
                }
            }
            Intent::SetSinkUnity => {
                if pw_clock::set_fireface_sink_unity() {
                    let volume = pw_clock::fireface_sink_volume();
                    let mut snap = snapshot.lock().unwrap();
                    if volume.is_some() {
                        snap.host_volume = volume;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inbox_coalesces_polls_but_keeps_writes_in_order() {
        let inbox = IntentInbox::new();
        assert!(inbox.send(Intent::ForceRate(48_000)));
        assert!(inbox.send(Intent::Poll { probe_quantum: false }));
        assert!(inbox.send(Intent::Poll { probe_quantum: true }));
        assert!(inbox.send(Intent::ForceQuantum(128)));
        // The two polls coalesce to the latest; the writes keep their order.
        let a = inbox.recv_timeout(Duration::from_millis(10)).unwrap();
        let b = inbox.recv_timeout(Duration::from_millis(10)).unwrap();
        let c = inbox.recv_timeout(Duration::from_millis(10)).unwrap();
        assert!(matches!(a, Intent::ForceRate(48_000)));
        assert!(matches!(b, Intent::Poll { probe_quantum: true }));
        assert!(matches!(c, Intent::ForceQuantum(128)));
        assert!(inbox.recv_timeout(Duration::from_millis(10)).is_none());
    }

    #[test]
    fn stop_rejects_new_intents() {
        let inbox = IntentInbox::new();
        inbox.request_stop();
        assert!(!inbox.send(Intent::ForceRate(48_000)));
    }

    #[test]
    fn the_snapshot_defaults_to_an_idle_unknown() {
        let snap = EngineSnapshot::default();
        assert_eq!(snap.owner, PcmOwner::Idle);
        assert!(snap.stream.is_none());
        assert!(snap.clock.is_none());
        assert!(snap.kernel_at.is_none());
    }

    /// Live desk check: the service's kernel-fact collector and PipeWire worker
    /// read the real card. Run with `--ignored` on a machine with a Fireface.
    #[test]
    #[ignore]
    fn live_service_collects_kernel_facts_and_graph() {
        let card_id = std::fs::read_dir("/proc/asound")
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if !p.file_name()?.to_str()?.starts_with("card") {
                    return None;
                }
                let id = std::fs::read_to_string(p.join("id")).ok()?;
                id.trim().contains("Fireface").then(|| id.trim().to_string())
            })
            .next()
            .expect("a Fireface card present");
        let svc = AudioPathService::spawn(card_id.clone());

        let mut snap = svc.snapshot();
        for _ in 0..100 {
            if snap.kernel_at.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            snap = svc.snapshot();
        }
        assert!(snap.kernel_at.is_some(), "kernel facts collected");
        assert_eq!(snap.stream, pw_clock::card_stream(&card_id), "stream matches procfs");
        let expect_owner = match pw_clock::card_pcm_status(&card_id) {
            None => PcmOwner::Idle,
            Some(s) => match s.owner_pid {
                Some(pid) => PcmOwner::from_comm(pw_clock::process_comm(pid).as_deref()),
                None => PcmOwner::Unknown,
            },
        };
        assert_eq!(snap.owner, expect_owner, "owner classification matches procfs");

        assert!(svc.send(Intent::Poll { probe_quantum: false }));
        for _ in 0..80 {
            if snap.pw_at.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            snap = svc.snapshot();
        }
        assert!(snap.pw_at.is_some(), "graph read completed on the worker");
    }

    /// Live desk check: a rate force applies while the graph is idle/suspended,
    /// because the force is followed by a re-prepare (the fix for the idle
    /// rate/quantum bug). Drives the service's exact `ForceRate → Reprepare`
    /// sequence and reads the card rate back from procfs. Restores the original
    /// rate afterwards. Run with `--ignored`, PipeWire owning the Fireface, and
    /// no other TotalControl instance running.
    #[test]
    #[ignore]
    fn live_rate_force_applies_while_idle() {
        let card_id = std::fs::read_dir("/proc/asound")
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if !p.file_name()?.to_str()?.starts_with("card") {
                    return None;
                }
                let id = std::fs::read_to_string(p.join("id")).ok()?;
                id.trim().contains("Fireface").then(|| id.trim().to_string())
            })
            .next()
            .expect("a Fireface card present");
        let svc = AudioPathService::spawn(card_id.clone());

        let mut snap = svc.snapshot();
        for _ in 0..100 {
            if snap.kernel_at.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            snap = svc.snapshot();
        }
        let baseline = snap.stream.map(|s| s.rate).unwrap_or(0);
        assert!(baseline > 0, "an open stream to read a rate from");
        let target = if baseline == 48_000 { 44_100 } else { 48_000 };

        // The handler's sequence: force the rate, then re-prepare so it lands
        // even though the node is suspended.
        assert!(svc.send(Intent::ForceRate(target)));
        assert!(svc.send(Intent::Reprepare));
        let mut applied = false;
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(100));
            if pw_clock::card_stream(&card_id).map(|s| s.rate) == Some(target) {
                applied = true;
                break;
            }
        }

        // Restore the baseline before asserting, so a failure still leaves the
        // card at the rate it had.
        let _ = svc.send(Intent::ForceRate(baseline));
        let _ = svc.send(Intent::Reprepare);
        for _ in 0..60 {
            if pw_clock::card_stream(&card_id).map(|s| s.rate) == Some(baseline) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(applied, "rate {target} applied while idle (baseline {baseline})");
    }
}
