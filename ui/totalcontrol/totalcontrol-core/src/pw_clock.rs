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

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Hard bound on every PipeWire tool call. A busy or restarting server can
/// stall `pw-dump`/`pw-cli`/`pw-metadata`; several of these calls run on the
/// UI thread (a mode switch applies the engine policy), so an unbounded wait
/// freezes the whole GUI. On timeout the child is killed and the call is
/// treated as absent.
const PW_TOOL_TIMEOUT: Duration = Duration::from_secs(3);

/// Run a PipeWire tool with a timeout, draining stdout/stderr in threads so a
/// large output (pw-dump) cannot fill the pipe and deadlock the child.
/// `None` on spawn failure or timeout.
fn run_timeout(cmd: &mut Command, timeout: Duration) -> Option<Output> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut so = child.stdout.take()?;
    let mut se = child.stderr.take()?;
    let oh = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let eh = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = oh.join();
                let _ = eh.join();
                return None;
            }
            Err(_) => {
                let _ = oh.join();
                let _ = eh.join();
                return None;
            }
        }
    };
    Some(Output {
        status,
        stdout: oh.join().unwrap_or_default(),
        stderr: eh.join().unwrap_or_default(),
    })
}

/// A PipeWire tool call with the standard timeout.
fn pw_output(cmd: &mut Command) -> Option<Output> {
    run_timeout(cmd, PW_TOOL_TIMEOUT)
}

/// Whether a PipeWire tool call succeeded within the timeout.
fn pw_status_ok(cmd: &mut Command) -> bool {
    pw_output(cmd).map(|o| o.status.success()).unwrap_or(false)
}

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
    /// The host policy's quantum bounds (`clock.min-quantum`/`max-quantum`);
    /// `0` = not reported.
    pub min_quantum: u32,
    pub max_quantum: u32,
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
        min_quantum: value_of("clock.min-quantum").unwrap_or(0),
        max_quantum: value_of("clock.max-quantum").unwrap_or(0),
    })
}

/// Reads the live clock. `None` if PipeWire (or `pw-metadata`) is absent.
pub fn read() -> Option<PwClock> {
    let out = pw_output(Command::new("pw-metadata").args(["-n", "settings", "0"]))?;
    if !out.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// The graph quantum the server is actually running, read from `pw-top`.
///
/// `pw-top` reports the driver node's QUANT; the follower shows 0.  This is
/// the only truthful source of the graph quantum: the ALSA period is a
/// different quantity (the host requests P = Q/2 for a batch device and the
/// driver clamps it to the floor, so several quanta share one period).  Two
/// samples are taken because the first one can be empty.
pub fn graph_quantum() -> Option<u32> {
    // pw-top takes two samples (~2 s); allow a longer bound than the rest.
    let out = run_timeout(
        Command::new("pw-top").args(["-b", "-n", "2"]),
        Duration::from_secs(6),
    )?;
    if !out.status.success() {
        return None;
    }
    parse_graph_quantum(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `pw-top -b` output for the Fireface driver node's quantum.
pub fn parse_graph_quantum(output: &str) -> Option<u32> {
    let mut found = None;
    for line in output.lines() {
        if !line.contains("Fireface") || !line.contains("pro-") {
            continue;
        }
        let mut fields = line.split_whitespace();
        let _state = fields.next()?;
        let _id = fields.next()?;
        let quantum = fields.next().and_then(|q| q.parse::<u32>().ok());
        if let Some(quantum) = quantum.filter(|q| *q > 0) {
            found = Some(quantum);
        }
    }
    found
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
    let out = pw_output(Command::new("pw-metadata").args(["-n", "settings", "0"]))?;
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
    let status = pw_output(Command::new("pw-metadata").args(["-n", "settings", "0", key, value]))
        .ok_or_else(|| format!("pw-metadata {key} {value} timed out"))?
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

/// The ALSA card number for a card id string, read from `/proc/asound`.
/// `None` when the card is not present.
pub fn card_number(card_id: &str) -> Option<i32> {
    let cards = std::fs::read_dir("/proc/asound").ok()?;
    for entry in cards.flatten() {
        let dir = entry.path();
        let name = dir.file_name()?.to_str()?.to_string();
        if !name.starts_with("card") {
            continue;
        }
        let id = std::fs::read_to_string(dir.join("id")).unwrap_or_default();
        if id.trim() == card_id {
            return name.trim_start_matches("card").parse().ok();
        }
    }
    None
}

/// Whether the RME Pro Audio sink is actually running a playback stream.
/// `pactl list short sinks` reports the node state in its last field.
pub fn fireface_sink_running() -> bool {
    let Some(out) = pw_output(Command::new("pactl").args(["list", "short", "sinks"])) else {
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
    let out = pw_output(Command::new("pactl").args(["list", "short", kind]))?;
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
    let out = pw_output(Command::new("pactl").args(["get-sink-volume", &name]))?;
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
    pw_status_ok(Command::new("pactl").args(["set-sink-volume", &name, "100%"]))
}

fn set_suspended(command: &str, node: &str, suspended: bool) -> bool {
    pw_status_ok(
        Command::new("pactl")
            .args([command, node, if suspended { "1" } else { "0" }])
            .stdin(Stdio::null()),
    )
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

/// The procfs directory of the ALSA card whose `id` is `card_id`. `None` when
/// no such card exists.
fn card_proc_dir(card_id: &str) -> Option<std::path::PathBuf> {
    let cards = std::fs::read_dir("/proc/asound").ok()?;
    for entry in cards.flatten() {
        let dir = entry.path();
        let is_card = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("card"));
        let id = std::fs::read_to_string(dir.join("id")).unwrap_or_default();
        if is_card && id.trim() == card_id {
            return Some(dir);
        }
    }
    None
}

/// The open *playback* stream of the ALSA card whose `id` is `card_id`.
/// `None` when no such card exists or nothing is playing. Playback only: the
/// capture side is held open by TotalControl's own input meter, so
/// reading it would report the app's own stream
/// (it once showed the meter's period as the card's).
pub fn card_stream(card_id: &str) -> Option<CardStream> {
    let dir = card_proc_dir(card_id)?;
    let text = std::fs::read_to_string(dir.join("pcm0p/sub0/hw_params")).ok()?;
    parse_hw_params(&text)
}

/// The kernel's view of the open playback PCM, from
/// `/proc/asound/<card>/pcm0p/sub0/status`. Server-free: these are short kernel
/// reads, never a sound-server call, so they keep flowing when PipeWire stalls.
/// `None` when the PCM is closed (`closed`) or the card is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmStatus {
    /// The PCM state (`RUNNING`, `SETUP`, `PREPARED`, `PAUSED`, `SUSPENDED`,
    /// `XRUN`).
    pub state: String,
    /// The owning process id (`owner_pid`), present since kernel v2.6.33.
    /// `None` only on a kernel that does not print it (defensive).
    pub owner_pid: Option<u32>,
    /// `snd_pcm_delay`: the ring fill plus the in-flight URB queue, in frames.
    pub delay: Option<u32>,
    /// The ring fill (`appl_ptr − hw_ptr`), in frames: the audio queued ahead
    /// of the hardware read pointer.
    pub fill: Option<u32>,
}

/// Pure parser for the playback PCM `status` file. `None` for `closed`.
pub fn parse_pcm_status(text: &str) -> Option<PcmStatus> {
    if text.trim() == "closed" {
        return None;
    }
    let field = |name: &str| -> Option<u64> {
        let line = text
            .lines()
            .find(|l| l.split(':').next().map(|k| k.trim()) == Some(name))?;
        line.split(':').nth(1)?.trim().parse().ok()
    };
    let state = text
        .lines()
        .find(|l| l.starts_with("state:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let fill = match (field("appl_ptr"), field("hw_ptr")) {
        (Some(appl), Some(hw)) => Some(appl.saturating_sub(hw) as u32),
        _ => None,
    };
    Some(PcmStatus {
        state,
        owner_pid: field("owner_pid").map(|v| v as u32),
        delay: field("delay").map(|v| v as u32),
        fill,
    })
}

/// The open playback PCM status of the ALSA card whose `id` is `card_id`.
/// `None` when the card is absent or the PCM is closed.
pub fn card_pcm_status(card_id: &str) -> Option<PcmStatus> {
    let dir = card_proc_dir(card_id)?;
    let text = std::fs::read_to_string(dir.join("pcm0p/sub0/status")).ok()?;
    parse_pcm_status(&text)
}

/// The `comm` of a process, read from `/proc/<pid>/comm`. `None` when the
/// process is gone or unreadable. Kernel `comm` is bounded to 15 characters.
pub fn process_comm(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(text.trim().to_string())
}

/// The PipeWire node ids of the RME Pro Audio playback and capture nodes,
/// from `pw-dump`. `None` when PipeWire is absent or does not own the
/// device (a JACK client holding the PCM leaves no Pro Audio nodes).
pub fn fireface_pro_node_ids() -> Option<(u32, u32)> {
    let out = pw_output(Command::new("pw-dump").stdin(Stdio::null()))?;
    if !out.status.success() {
        return None;
    }
    parse_pro_node_ids(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parser for [`fireface_pro_node_ids`]: the `pw-dump` JSON array.
pub fn parse_pro_node_ids(json: &str) -> Option<(u32, u32)> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut output = None;
    let mut input = None;
    for obj in value.as_array()? {
        let name = obj["info"]["props"]["node.name"].as_str().unwrap_or("");
        if name.contains(".monitor") {
            continue;
        }
        let id = obj["id"].as_u64()? as u32;
        if name.contains("pro-output-0") {
            output = Some(id);
        } else if name.contains("pro-input-0") {
            input = Some(id);
        }
    }
    Some((output?, input?))
}

/// The node's `api.alsa.headroom` as its Props parameter reports it.
pub fn read_headroom(node: u32) -> Option<u32> {
    let out = pw_output(
        Command::new("pw-cli")
            .args(["enum-params", &node.to_string(), "Props"])
            .stdin(Stdio::null()),
    )?;
    if !out.status.success() {
        return None;
    }
    parse_headroom(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parser for [`read_headroom`]: the `pw-cli enum-params … Props`
/// text, where the value line follows the `api.alsa.headroom` key line.
pub fn parse_headroom(text: &str) -> Option<u32> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.contains("\"api.alsa.headroom\"") {
            let value = lines.next()?.trim();
            return value.strip_prefix("Int ").and_then(|v| v.trim().parse().ok());
        }
    }
    None
}

/// Set the node's `api.alsa.headroom`. The SPA plugin applies the value at
/// the next PCM re-prepare (rate/quantum change, recovery, suspend/resume),
/// not to the running stream. The `params` property takes the array form
/// `[key, value]` (desk-verified on PipeWire 1.6.11; the object form builds
/// an empty pod in `pw-cli`).
pub fn set_headroom(node: u32, frames: u32) -> Result<(), String> {
    let pod = format!("{{ \"params\": [ \"api.alsa.headroom\", {frames} ] }}");
    let status = pw_output(
        Command::new("pw-cli")
            .args(["set-param", &node.to_string(), "Props", &pod])
            .stdin(Stdio::null()),
    )
    .ok_or_else(|| format!("pw-cli set-param headroom {frames} timed out"))?
    .status;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pw-cli set-param headroom {frames} failed ({status})"))
    }
}

/// The scheduling policy for a Pro Audio node (C1 three-tier model):
/// IRQ + per-rate cushion on the Device-authority pro path, timer + a fixed
/// generous margin on the PipeWire convenience path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedPolicy {
    /// `true` = IRQ (period-interrupt) scheduling; `false` = timer + DLL.
    pub disable_tsched: bool,
    pub headroom: u32,
    pub start_delay: u32,
    pub period_num: u32,
}

/// The `params` Props pod for a scheduling policy. The multi-key array form
/// `[key, value, …]` is desk-verified on PipeWire 1.6.11 (the object form
/// builds an empty pod in `pw-cli`). Pure, for tests.
pub fn sched_policy_pod(p: &SchedPolicy) -> String {
    format!(
        "{{ \"params\": [ \"api.alsa.disable-tsched\", {}, \"api.alsa.headroom\", {}, \
         \"api.alsa.start-delay\", {}, \"api.alsa.period-num\", {} ] }}",
        p.disable_tsched, p.headroom, p.start_delay, p.period_num
    )
}

/// Set the scheduling policy on a node. The value lands at the next PCM
/// re-prepare; the caller triggers one (a rate/quantum force, or a quantum
/// toggle on a mode switch — a same-value force does not re-prepare).
pub fn set_sched_policy(node: u32, p: &SchedPolicy) -> Result<(), String> {
    let pod = sched_policy_pod(p);
    let status = pw_output(
        Command::new("pw-cli")
            .args(["set-param", &node.to_string(), "Props", &pod])
            .stdin(Stdio::null()),
    )
    .ok_or_else(|| "pw-cli set-param sched policy timed out".to_string())?
    .status;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pw-cli set-param sched policy failed ({status})"))
    }
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
                force_quantum: 0,
                min_quantum: 32,
                max_quantum: 2048,
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
    fn parses_the_playback_pcm_status() {
        // The desk-verified `status` layout: `key   : value`, pointers after a
        // `-----` rule. delay = fill + the in-flight URB queue.
        let text = "state: RUNNING\nowner_pid   : 2109270\n\
                     trigger_time: 428157.725532926\ntstamp      : 430338.914271538\n\
                     delay       : 1797\navail       : 6491\navail_max   : 7211\n\
                     -----\nhw_ptr      : 104697003\nappl_ptr    : 104698704\n";
        let s = parse_pcm_status(text).unwrap();
        assert_eq!(s.state, "RUNNING");
        assert_eq!(s.owner_pid, Some(2109270));
        assert_eq!(s.delay, Some(1797));
        assert_eq!(s.fill, Some(1701));
        assert_eq!(parse_pcm_status("closed\n"), None);
        // A kernel without owner_pid still reports the delay and fill.
        let no_owner = "state: RUNNING\ndelay       : 96\n-----\nhw_ptr      : 100\nappl_ptr    : 148\n";
        let s = parse_pcm_status(no_owner).unwrap();
        assert_eq!(s.owner_pid, None);
        assert_eq!(s.fill, Some(48));
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
    fn parses_the_graph_quantum_from_pw_top() {
        let output = "S   ID  QUANT   RATE    WAIT    BUSY\n\
R  134     64  48000  23.8us   0.7us  alsa_input.usb-RME_Fireface_UCX__x-00.pro-input-0\n\
R   71      0      0   4.8us   5.7us  + alsa_output.usb-RME_Fireface_UCX__x-00.pro-output-0\n";
        assert_eq!(parse_graph_quantum(output), Some(64));
        // No Fireface node: unknown.
        assert_eq!(parse_graph_quantum("S   ID  QUANT\n"), None);
        // A zero quantum (idle) is not a reading.
        let idle = "R  134      0      0   ---  alsa_input.usb-RME_Fireface_UCX__x-00.pro-input-0\n";
        assert_eq!(parse_graph_quantum(idle), None);
    }

    #[test]
    fn no_pipewire_settings_is_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("Found \"settings\" metadata 31\n"), None);
    }

    #[test]
    fn parses_the_pro_node_ids_from_pw_dump() {
        let json = r#"[
          { "id": 32, "type": "PipeWire:Interface:Device", "info": { "props": { "node.name": "alsa_card.usb-RME_Fireface_UCX-00" } } },
          { "id": 73, "info": { "props": { "node.name": "alsa_output.usb-RME_Fireface_UCX-00.pro-output-0" } } },
          { "id": 55, "info": { "props": { "node.name": "alsa_input.usb-RME_Fireface_UCX-00.pro-input-0" } } },
          { "id": 90, "info": { "props": { "node.name": "alsa_output.usb-RME_Fireface_UCX-00.pro-output-0.monitor" } } }
        ]"#;
        assert_eq!(parse_pro_node_ids(json), Some((73, 55)));
        // No Pro Audio nodes (JACK owns the PCM, or another card).
        assert_eq!(parse_pro_node_ids("[]"), None);
        assert_eq!(parse_pro_node_ids("not json"), None);
    }

    #[test]
    fn parses_the_headroom_from_enum_params() {
        let text = "Object: size 1512, type Spa:Pod:Object:Param:Props (262146)\n\
    Prop: key Spa:Pod:Object:Param:Props:params (524289), flags 00000000\n\
        String \"api.alsa.period-num\"\n\
        Int 32\n\
        String \"api.alsa.headroom\"\n\
        Int 96\n\
        String \"api.alsa.start-delay\"\n\
        Int 0\n";
        assert_eq!(parse_headroom(text), Some(96));
        assert_eq!(parse_headroom("String \"api.alsa.headroom\"\n"), None);
        assert_eq!(parse_headroom(""), None);
    }

    #[test]
    fn builds_the_sched_policy_pod() {
        let irq = SchedPolicy { disable_tsched: true, headroom: 96, start_delay: 0, period_num: 32 };
        assert_eq!(
            sched_policy_pod(&irq),
            "{ \"params\": [ \"api.alsa.disable-tsched\", true, \"api.alsa.headroom\", 96, \
             \"api.alsa.start-delay\", 0, \"api.alsa.period-num\", 32 ] }"
        );
        let timer = SchedPolicy { disable_tsched: false, headroom: 336, start_delay: 144, period_num: 16 };
        assert!(sched_policy_pod(&timer).contains("\"api.alsa.disable-tsched\", false"));
        assert!(sched_policy_pod(&timer).contains("\"api.alsa.start-delay\", 144"));
    }
}
