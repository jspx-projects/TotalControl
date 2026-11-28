//! Device service — the first stage of the device/UI split.
//!
//! The device currently lives on the UI thread.  This module introduces the
//! boundary that will own it in the next stage:
//!
//! - one thread owns the `DeviceHandle`;
//! - the UI will send [`Command`]s instead of calling the device;
//! - the UI will read a [`Snapshot`] of the channel model;
//! - meter canvases will read [`MeterSnapshot`] without a view rebuild.
//!
//! The service is the running path: the device handle is moved onto the
//! service thread and the UI keeps a mirror.  `TOTALCONTROL_DEVICE_SERVICE=0`
//! selects the direct path (the UI owns the device and is the meter
//! consumer); the test suite defaults to the direct path so it can exercise
//! the backend without a service thread.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use totalcontrol_core::channel::{EqBandType, InputEq};
use totalcontrol_core::fireface::abi::HwdepCounters;
use totalcontrol_core::{
    ChannelId, DeviceSettings, Dynamics, Echo, Error, InputChannel, InputLevel, InputSyncState,
    OutputChannel, PlaybackChannel, Reverb, RmeDevice, Scene, Sensitivity, TalkbackConfig,
};

use crate::app::DeviceHandle;
use crate::widgets::fader::MeterBallistics;

/// Wakes the UI subscription when a model snapshot is published.  The
/// snapshot itself travels through the `std` channel on `DeviceService`;
/// this is only the "there is something to drain" edge, so an idle mixer
/// does not rebuild its view at the polling rate.
static SNAPSHOT_NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();

fn snapshot_notify() -> Arc<Notify> {
    Arc::clone(SNAPSHOT_NOTIFY.get_or_init(|| Arc::new(Notify::new())))
}

/// Wakes the UI subscription when meter activity starts after silence.  A
/// meter frame itself never publishes a message; this edge only tells an
/// idle UI to draw the first frame of a new burst.
static METER_NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();

fn meter_notify() -> Arc<Notify> {
    Arc::clone(METER_NOTIFY.get_or_init(|| Arc::new(Notify::new())))
}

/// The meter-wake stream consumed by `app::subscription`.  It yields once
/// when the service sees the first activity after an idle period.
pub fn meter_stream() -> impl iced::futures::Stream<Item = ()> {
    iced::futures::stream::unfold(meter_notify(), |notify| async move {
        notify.notified().await;
        Some(((), notify))
    })
}

/// The model-update stream consumed by `app::subscription`.  It yields only
/// when the service has actually published a snapshot.
pub fn update_stream() -> impl iced::futures::Stream<Item = ()> {
    iced::futures::stream::unfold(snapshot_notify(), |notify| async move {
        notify.notified().await;
        Some(((), notify))
    })
}

/// A command sent from the UI to the device service.
pub enum Command {
    SetVolume(ChannelId, usize, f32),
    SetMute(ChannelId, bool),
    ApplyScene(Box<Scene>),
    /// A control that needs the device thread.  Used by the mirror for
    /// setters that have no dedicated variant.
    Run(Box<dyn FnOnce(&mut DeviceHandle) + Send>),
    Stop,
}

/// The bounded, coalescing command inbox (C1). Latest-value commands
/// (volume, mute, scene) replace their pending predecessor; other commands
/// queue in order. Writes never block the GUI thread: when the queue is full
/// the oldest command is dropped so the latest state survives. Stop preempts
/// the queue through a flag.
pub struct CommandInbox {
    queue: Mutex<VecDeque<Command>>,
    cv: Condvar,
    stop: AtomicBool,
    /// Write errors reported by the service thread's command closures.  The
    /// UI drains them from the next published snapshot.
    errors: Mutex<VecDeque<String>>,
}

const COMMAND_CAP: usize = 256;

impl CommandInbox {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            errors: Mutex::new(VecDeque::new()),
        }
    }

    fn send(&self, command: Command) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        let mut q = self.queue.lock().unwrap();
        let replace = match &command {
            Command::SetVolume(ch, out, _) => q.iter().position(
                |c| matches!(c, Command::SetVolume(ch2, out2, _) if ch2 == ch && out2 == out),
            ),
            Command::SetMute(ch, _) => q
                .iter()
                .position(|c| matches!(c, Command::SetMute(ch2, _) if ch2 == ch)),
            Command::ApplyScene(_) => q.iter().position(|c| matches!(c, Command::ApplyScene(_))),
            _ => None,
        };
        if let Some(pos) = replace {
            q[pos] = command;
            self.cv.notify_one();
            return true;
        }
        // The producer is the GUI thread: never wait. Dropping the oldest
        // command keeps the GUI responsive; the device service drains at its
        // own pace.
        while q.len() >= COMMAND_CAP {
            q.pop_front();
        }
        q.push_back(command);
        self.cv.notify_one();
        true
    }

    fn recv_timeout(&self, timeout: Duration) -> Option<Command> {
        let mut q = self.queue.lock().unwrap();
        if q.is_empty() && !self.stop.load(Ordering::Relaxed) {
            q = self.cv.wait_timeout(q, timeout).unwrap().0;
        }
        let cmd = q.pop_front();
        if cmd.is_some() {
            self.cv.notify_all();
        }
        cmd
    }

    fn drain(&self) -> Vec<Command> {
        let cmds: Vec<Command> = self.queue.lock().unwrap().drain(..).collect();
        if !cmds.is_empty() {
            self.cv.notify_all();
        }
        cmds
    }

    fn report_error(&self, message: String) {
        let mut errors = self.errors.lock().unwrap();
        // Keep the notice bounded; the oldest unwritten error is the least
        // useful when a burst of writes fails.
        while errors.len() >= 8 {
            errors.pop_front();
        }
        errors.push_back(message);
    }

    fn drain_errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().drain(..).collect()
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cv.notify_all();
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

/// The sending half of a [`CommandInbox`], held by the mirror.
#[derive(Clone)]
pub struct CommandSender(Arc<CommandInbox>);

impl CommandSender {
    pub fn send(&self, command: Command) -> Result<(), ()> {
        if self.0.send(command) {
            Ok(())
        } else {
            Err(())
        }
    }

    fn report_error(&self, message: String) {
        self.0.report_error(message);
    }

    fn request_stop(&self) {
        self.0.request_stop();
    }
}

/// Static properties of the device, captured once when the service starts.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub model_name: String,
    pub canonical_model: String,
    pub sample_rates: &'static [u32],
    pub capabilities: totalcontrol_core::DeviceCapabilities,
    pub protocol_features: bool,
    pub class_compliant: bool,
    pub low_cut_supported: bool,
    pub phase_supported: bool,
    pub output_balance_supported: bool,
    pub loopback_supported: bool,
    pub stereo_split_supported: bool,
    pub matrix_uses_display_order: bool,
    pub output_pair_count: usize,
    pub output_display_order: Vec<usize>,
    pub is_uc: bool,
    pub is_ucx: bool,
    pub is_uc_family: bool,
    pub is_mock: bool,
    pub kind: crate::app::DeviceKind,
    pub target: crate::app::DeviceTarget,
    /// Short personality label (`PC`, `AP`, `ISO`, `IF5`), when the
    /// connection reports one.
    pub personality: Option<&'static str>,
    pub card_id: String,
    pub firmware: Option<u32>,
    pub serial: Option<String>,
    pub rate_follows: bool,
    pub has_sensitivity: bool,
    pub outputs_one_per_pair: bool,
    /// Per-input meter availability on the backend that was captured.
    pub input_meters: Vec<bool>,
    /// The backend supplies its own software-playback metering, so the
    /// PipeWire sink tap is not needed.
    pub device_playback_meters: bool,
}

/// The channel model as the UI should draw it.  Replaced wholesale when the
/// service sees a change; cheap enough to clone at the UI's housekeeping
/// rate.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub scene: Scene,
    pub info: DeviceInfo,
    pub connected: bool,
    pub hardware_volume: Option<f32>,
    pub panel_selection: Option<(bool, usize, usize)>,
    pub at: Instant,
    /// Device write errors since the previous snapshot.  The UI turns each
    /// one into a visible notice instead of the service discarding it.
    pub errors: Vec<String>,
}

/// The UI-side copy of the model.
///
/// Getters return references into the local copy, so the view never locks or
/// blocks on the device thread.  Setters enqueue a command; the next
/// [`Snapshot`] replaces the local copy.  The service updates it from
/// `DeviceService::take_updates`.
pub struct Mirror {
    scene: Scene,
    info: DeviceInfo,
    commands: CommandSender,
    meters: Arc<Mutex<MeterSnapshot>>,
    connected: bool,
    hardware_volume: Option<f32>,
    panel_selection: Option<(bool, usize, usize)>,
    /// Write errors waiting for the UI to show them.
    errors: VecDeque<String>,
}

impl Mirror {
    pub fn new(
        initial: Snapshot,
        commands: CommandSender,
        meters: Arc<Mutex<MeterSnapshot>>,
    ) -> Self {
        Self {
            scene: initial.scene,
            info: initial.info,
            commands,
            meters,
            connected: initial.connected,
            hardware_volume: initial.hardware_volume,
            panel_selection: initial.panel_selection,
            errors: initial.errors.into(),
        }
    }

    pub fn meters(&self) -> MeterSnapshot {
        *self.meters.lock().unwrap()
    }

    pub fn meter_bus(&self) -> Arc<Mutex<MeterSnapshot>> {
        Arc::clone(&self.meters)
    }

    /// Driver-side transport health as last reported by the service.
    pub fn transport_counters(&self) -> Option<HwdepCounters> {
        self.meters.lock().ok()?.counters
    }

    /// The rate the unit reports it is set to, as last polled by the service.
    pub fn device_rate(&self) -> Option<u32> {
        self.meters.lock().ok()?.device_rate
    }

    /// Lock, sync and detected rate for each digital input, as last polled
    /// by the service.
    pub fn input_sync_status(&self) -> [Option<InputSyncState>; 3] {
        self.meters
            .lock()
            .map(|m| m.input_sync)
            .unwrap_or([None; 3])
    }

    /// Apply a snapshot published by the service.  Returns false once the
    /// service has stopped.
    pub fn update(&mut self, snapshot: Snapshot) -> bool {
        self.scene = snapshot.scene;
        self.info = snapshot.info;
        self.connected = snapshot.connected;
        self.hardware_volume = snapshot.hardware_volume;
        self.panel_selection = snapshot.panel_selection;
        self.errors.extend(snapshot.errors);
        true
    }

    /// The next device write error, if the service reported one.  The UI
    /// shows it as a notice and calls this until it returns `None`.
    pub fn take_error(&mut self) -> Option<String> {
        self.errors.pop_front()
    }

    pub fn inputs(&self) -> &[totalcontrol_core::InputChannel] {
        &self.scene.inputs
    }

    pub fn playbacks(&self) -> &[totalcontrol_core::PlaybackChannel] {
        &self.scene.playbacks
    }

    pub fn outputs(&self) -> &[totalcontrol_core::OutputChannel] {
        &self.scene.outputs
    }

    pub fn settings(&self) -> &totalcontrol_core::DeviceSettings {
        &self.scene.settings
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn hardware_volume(&self) -> Option<f32> {
        self.hardware_volume
    }

    pub fn panel_selection(&self) -> Option<(bool, usize, usize)> {
        self.panel_selection
    }

    pub fn set_volume(&self, channel: ChannelId, output: usize, value: f32) -> bool {
        self.commands
            .send(Command::SetVolume(channel, output, value))
            .is_ok()
    }

    pub fn set_mute(&self, channel: ChannelId, mute: bool) -> bool {
        self.commands.send(Command::SetMute(channel, mute)).is_ok()
    }

    fn run(&self, command: impl FnOnce(&mut DeviceHandle) + Send + 'static) {
        let _ = self.commands.send(Command::Run(Box::new(command)));
    }

    /// Run a write on the device thread and report its error through the
    /// next snapshot.  A command that the queue rejects is a stopped
    /// service, so there is nothing left to report.
    fn run_checked(
        &self,
        command: impl FnOnce(&mut DeviceHandle) -> Result<(), Error> + Send + 'static,
    ) {
        let sender = self.commands.clone();
        self.run(move |d| {
            if let Err(e) = command(d) {
                sender.report_error(e.to_string());
            }
        });
    }

    /// The server-free idle device-rate write (the JACK / direct-ALSA pro
    /// path). Updates the local scene rate at once — the rate-change reapply
    /// captures the mirror scene immediately after this call — and queues the
    /// idle write on the service thread. The device readback confirms the rate.
    pub fn set_sample_rate_idle(&mut self, rate: u32) -> Result<(), Error> {
        self.scene.settings.sample_rate = rate;
        self.run_checked(move |d| d.set_sample_rate_idle(rate));
        Ok(())
    }

    /// Apply a connect or restore scene on the service thread without
    /// rewriting output levels the unit already holds.
    pub fn apply_scene_at_connect(&mut self, scene: &Scene) -> Result<(), Error> {
        self.scene = scene.clone();
        let scene = scene.clone();
        self.run_checked(move |d| d.apply_scene_at_connect(&scene));
        Ok(())
    }

    pub fn set_line_in_ref(&self, code: u16) {
        self.run_checked(move |d| d.set_line_in_ref(code));
    }

    pub fn set_line_out_ref(&self, code: u16) {
        self.run_checked(move |d| d.set_line_out_ref(code));
    }

    pub fn set_phones_ref(&self, code: u16) {
        self.run_checked(move |d| d.set_phones_ref(code));
    }

    pub fn set_line_in_ref_58(&self, code: u16) {
        self.run_checked(move |d| d.set_line_in_ref_58(code));
    }

    pub fn set_spdif_professional(&self, on: bool) {
        self.run_checked(move |d| d.set_spdif_professional(on));
    }

    pub fn set_word_clock_single_speed(&self, on: bool) {
        self.run_checked(move |d| d.set_word_clock_single_speed(on));
    }

    pub fn set_word_clock_termination(&self, on: bool) {
        self.run_checked(move |d| d.set_word_clock_termination(on));
    }

    pub fn set_input_ms_proc(&self, idx: usize, on: bool) {
        self.run_checked(move |d| d.set_input_ms_proc(idx, on));
    }

    pub fn set_playback_ms_proc(&self, idx: usize, on: bool) {
        self.run_checked(move |d| d.set_playback_ms_proc(idx, on));
    }

    pub fn set_talkback(&self, on: bool) {
        self.set_talkback_with(on, &TalkbackConfig::default());
    }

    pub fn set_talkback_with(&self, on: bool, config: &TalkbackConfig) {
        let config = *config;
        self.run_checked(move |d| d.set_talkback_with(on, &config));
    }

    pub fn set_talkback_receive(&self, output: usize, on: bool) {
        self.run_checked(move |d| d.set_talkback_receive(output, on));
    }

    /// Push the local settings block to the service.  Link groupings and
    /// the Control Room mono flag are host-model settings with no dedicated
    /// trait setter; without this the next service snapshot would replace
    /// the local edit with the old value.
    pub fn sync_settings(&self) {
        let settings = self.scene.settings.clone();
        self.run(move |d| {
            // Only the host-model settings travel here.  The device-owned
            // clock source, options, references and sample rate come from
            // the unit; a stale scene copy must not overwrite the live
            // model.  A later scene apply would then write the stale value
            // back to the unit.
            let mut live = d.settings().clone();
            if live.clock_source != settings.clock_source {
                log::info!(
                    "sync_settings keeps live clock {} (scene {})",
                    live.clock_source,
                    settings.clock_source
                );
            }
            live.input_link = settings.input_link;
            live.output_link = settings.output_link;
            live.input_pair_link = settings.input_pair_link;
            live.mono = settings.mono;
            *d.settings_mut() = live;
        });
    }
}

/// The pair-model default for one mono matrix cell, read from the mirror's
/// snapshot.  UC per-pair arrays are in display order; UCX arrays are in
/// internal Phones-first order.
fn scene_matrix_default(
    scene: &Scene,
    info: &DeviceInfo,
    channel: ChannelId,
    block: usize,
) -> Option<f32> {
    if block >= info.output_pair_count * 2 {
        return None;
    }
    let pair = if info.matrix_uses_display_order {
        block / 2
    } else {
        info.output_display_order.get(block / 2).copied()?
    };
    let side = block % 2;
    let level = match channel {
        ChannelId::Input(i) => *scene.inputs.get(i)?.volumes.get(pair)?,
        ChannelId::Playback(i) => *scene.playbacks.get(i)?.volumes.get(pair)?,
        ChannelId::Output(_) => return None,
    };
    if level <= 0.0 {
        // A zero send is no routing, so no cell (GUI spec 8.1.1).
        return None;
    }
    match channel {
        ChannelId::Input(i) => {
            let ch = scene.inputs.get(i)?;
            let volume = *ch.volumes.get(pair)?;
            if i < 4 {
                let pan = ch.pans.get(pair).copied().unwrap_or(0);
                let (left, right) = (volume, volume);
                let p = pan.clamp(-100, 100) as f32 / 100.0;
                Some(if side == 0 {
                    left * if p > 0.0 { 1.0 - p } else { 1.0 }
                } else {
                    right * if p < 0.0 { 1.0 + p } else { 1.0 }
                })
            } else if side == i % 2 {
                Some(volume)
            } else {
                None
            }
        }
        ChannelId::Playback(i) => {
            let ch = scene.playbacks.get(i)?;
            let volume = *ch.volumes.get(pair)?;
            if info.is_uc {
                let pan = ch.pans.get(pair).copied().unwrap_or(0);
                let p = pan.clamp(-100, 100) as f32 / 100.0;
                let (left, right) = (volume, volume);
                Some(if side == i % 2 {
                    if i % 2 == 0 {
                        left * if p > 0.0 { 1.0 - p } else { 1.0 }
                    } else {
                        right * if p < 0.0 { 1.0 + p } else { 1.0 }
                    }
                } else {
                    0.0
                })
            } else if side == i % 2 {
                Some(volume)
            } else {
                None
            }
        }
        ChannelId::Output(_) => None,
    }
}

impl RmeDevice for Mirror {
    fn open() -> Result<Self, Error> {
        unreachable!("the mirror is constructed from a snapshot")
    }

    fn model_name(&self) -> &str {
        &self.info.model_name
    }

    fn sample_rates(&self) -> &'static [u32] {
        self.info.sample_rates
    }

    fn capabilities(&self) -> totalcontrol_core::DeviceCapabilities {
        self.info.capabilities
    }

    fn protocol_features(&self) -> bool {
        self.info.protocol_features
    }

    fn class_compliant(&self) -> bool {
        self.info.class_compliant
    }

    fn canonical_model(&self) -> &str {
        &self.info.canonical_model
    }

    fn low_cut_supported(&self) -> bool {
        self.info.low_cut_supported
    }

    fn phase_supported(&self) -> bool {
        self.info.phase_supported
    }

    fn output_balance_supported(&self) -> bool {
        self.info.output_balance_supported
    }

    fn loopback_supported(&self) -> bool {
        self.info.loopback_supported
    }

    fn stereo_split_supported(&self) -> bool {
        self.info.stereo_split_supported
    }

    fn matrix_uses_display_order(&self) -> bool {
        self.info.matrix_uses_display_order
    }

    fn matrix_blocks_for_pair(&self, pair: usize) -> [usize; 2] {
        mirror_matrix_blocks(
            &self.info.output_display_order,
            self.info.matrix_uses_display_order,
            pair,
        )
    }

    fn matrix_cells_supported(&self) -> bool {
        self.info.capabilities.matrix_cells
    }

    fn matrix_cell(&self, channel: ChannelId, block: usize) -> Option<f32> {
        let source = match channel {
            ChannelId::Input(i) => i,
            ChannelId::Playback(i) => 18 + i,
            ChannelId::Output(_) => return None,
        };
        if let Some(cell) = self
            .scene
            .matrix_overrides
            .iter()
            .find(|cell| cell.source == source && cell.block == block)
        {
            // A routing exists while it has a level (GUI spec 8.1.1), so a
            // zero cell is no routing.
            return (cell.gain > 0.0).then_some(cell.gain);
        }
        scene_matrix_default(&self.scene, &self.info, channel, block).filter(|gain| *gain > 0.0)
    }

    fn set_matrix_cell(
        &mut self,
        channel: ChannelId,
        block: usize,
        gain: f32,
    ) -> Result<(), Error> {
        self.run_checked(move |d| d.set_matrix_cell(channel, block, gain));
        Ok(())
    }

    fn clear_all_routings(&mut self) {
        self.run(|d| d.clear_all_routings());
    }

    fn matrix_cell_routed(&self, channel: ChannelId, pair: usize) -> bool {
        self.matrix_blocks_for_pair(pair)
            .iter()
            .any(|block| self.matrix_cell(channel, *block).is_some())
    }

    fn output_display_order(&self) -> Vec<usize> {
        self.info.output_display_order.clone()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn begin_metering(&mut self) {
        self.run(|d| d.begin_metering());
    }

    fn output_pair_count(&self) -> usize {
        self.info.output_pair_count
    }

    fn inputs(&self) -> &[InputChannel] {
        &self.scene.inputs
    }

    fn inputs_mut(&mut self) -> &mut [InputChannel] {
        &mut self.scene.inputs
    }

    fn playbacks(&self) -> &[PlaybackChannel] {
        &self.scene.playbacks
    }

    fn playbacks_mut(&mut self) -> &mut [PlaybackChannel] {
        &mut self.scene.playbacks
    }

    fn outputs(&self) -> &[OutputChannel] {
        &self.scene.outputs
    }

    fn outputs_mut(&mut self) -> &mut [OutputChannel] {
        &mut self.scene.outputs
    }

    fn settings(&self) -> &DeviceSettings {
        &self.scene.settings
    }

    fn settings_mut(&mut self) -> &mut DeviceSettings {
        &mut self.scene.settings
    }

    fn set_volume(&mut self, channel: ChannelId, output: usize, volume: f32) -> Result<(), Error> {
        self.commands
            .send(Command::SetVolume(channel, output, volume))
            .map_err(|_| stopped())?;
        Ok(())
    }

    fn volume(&self, channel: ChannelId, output: usize) -> Result<f32, Error> {
        match channel {
            ChannelId::Input(i) => self
                .scene
                .inputs
                .get(i)
                .and_then(|c| c.volumes.get(output))
                .copied()
                .ok_or_else(|| missing(channel)),
            ChannelId::Playback(i) => self
                .scene
                .playbacks
                .get(i)
                .and_then(|c| c.volumes.get(output))
                .copied()
                .ok_or_else(|| missing(channel)),
            ChannelId::Output(i) => self
                .scene
                .outputs
                .get(i)
                .map(|c| c.volume)
                .ok_or_else(|| missing(channel)),
        }
    }

    fn set_pan(&mut self, channel: ChannelId, output: usize, pan: i8) -> Result<(), Error> {
        self.run_checked(move |d| d.set_pan(channel, output, pan));
        Ok(())
    }

    fn pan(&self, channel: ChannelId, output: usize) -> Result<i8, Error> {
        match channel {
            ChannelId::Input(i) => self
                .scene
                .inputs
                .get(i)
                .and_then(|c| c.pans.get(output))
                .copied()
                .ok_or_else(|| missing(channel)),
            ChannelId::Playback(i) => self
                .scene
                .playbacks
                .get(i)
                .and_then(|c| c.pans.get(output))
                .copied()
                .ok_or_else(|| missing(channel)),
            ChannelId::Output(i) => self
                .scene
                .outputs
                .get(i)
                .map(|c| c.balance)
                .ok_or_else(|| missing(channel)),
        }
    }

    fn set_mute(&mut self, channel: ChannelId, mute: bool) -> Result<(), Error> {
        self.commands
            .send(Command::SetMute(channel, mute))
            .map_err(|_| stopped())?;
        Ok(())
    }

    fn mute(&self, channel: ChannelId) -> Result<bool, Error> {
        match channel {
            ChannelId::Input(i) => self.scene.inputs.get(i).map(|c| c.mute),
            ChannelId::Playback(i) => self.scene.playbacks.get(i).map(|c| c.mute),
            ChannelId::Output(i) => self.scene.outputs.get(i).map(|c| c.mute),
        }
        .ok_or_else(|| missing(channel))
    }

    fn set_solo(&mut self, channel: ChannelId, solo: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_solo(channel, solo));
        Ok(())
    }

    fn solo(&self, channel: ChannelId) -> Result<bool, Error> {
        match channel {
            ChannelId::Input(i) => self.scene.inputs.get(i).map(|c| c.solo),
            ChannelId::Playback(i) => self.scene.playbacks.get(i).map(|c| c.solo),
            ChannelId::Output(i) => self.scene.outputs.get(i).map(|c| c.solo),
        }
        .ok_or_else(|| missing(channel))
    }

    fn capture_scene(&self) -> Scene {
        // The service snapshot carries the active monitor flags and the
        // saved pre-Dim levels, so the mirror can hand the complete scene
        // to a workspace without losing Dim or Speaker B.
        self.scene.clone()
    }

    fn apply_scene(&mut self, scene: &Scene) -> Result<(), Error> {
        self.scene = scene.clone();
        let scene = scene.clone();
        self.run_checked(move |d| d.apply_scene(&scene));
        Ok(())
    }

    fn poll_events(&mut self) -> Result<(), Error> {
        // The service thread polls; the mirror has no transport.
        Ok(())
    }

    fn set_clock_source(&mut self, source: &str) -> Result<(), Error> {
        log::info!("device service set_clock_source: {source}");
        let source = source.to_string();
        self.run_checked(move |d| d.set_clock_source(&source));
        Ok(())
    }

    fn set_sensitivity(&mut self, idx: usize, sensitivity: Sensitivity) -> Result<(), Error> {
        self.run_checked(move |d| d.set_sensitivity(idx, sensitivity));
        Ok(())
    }

    fn set_pitch(&mut self, pitch_percent: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_pitch(pitch_percent));
        Ok(())
    }

    fn write_pitch_clock(&mut self, pitch_percent: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.write_pitch_clock(pitch_percent));
        Ok(())
    }

    fn store_pitch_setting(&mut self, pitch_percent: f32) {
        // Store locally at once (the next command reads the fresh setting)
        // and on the device thread, so the published snapshot carries it.
        let pitch = pitch_percent.clamp(-5.0, 5.0);
        self.scene.settings.pitch_percent = pitch;
        self.run(move |d| {
            d.store_pitch_setting(pitch);
        });
    }

    // The remaining controls have no required trait body; forward the ones
    // the mixer uses.  Each one becomes a `Run` command on the device
    // thread, so the model snapshot is the single source of truth.
    fn set_output_linked(&mut self, pair: usize, linked: bool) -> Result<(), Error> {
        // Update the local link state at once: the next command (a balance
        // or pan move) must not choose the linked-pair semantics from a
        // stale mirror while the service snapshot is in flight.
        if self.scene.settings.output_link.len() <= pair {
            self.scene.settings.output_link.resize(pair + 1, true);
        }
        self.scene.settings.output_link[pair] = linked;
        self.run_checked(move |d| d.set_output_linked(pair, linked));
        Ok(())
    }

    fn set_input_pair_linked(&mut self, pair: usize, linked: bool) -> Result<(), Error> {
        if self.scene.settings.input_pair_link.len() <= pair {
            self.scene.settings.input_pair_link.resize(pair + 1, true);
        }
        self.scene.settings.input_pair_link[pair] = linked;
        self.run_checked(move |d| d.set_input_pair_linked(pair, linked));
        Ok(())
    }

    fn set_playback_linked(&mut self, pair: usize, linked: bool) -> Result<(), Error> {
        for channel in [pair * 2, pair * 2 + 1] {
            if let Some(playback) = self.scene.playbacks.get_mut(channel) {
                playback.split = !linked;
            }
        }
        self.run_checked(move |d| d.set_playback_linked(pair, linked));
        Ok(())
    }

    fn set_output_volume(&mut self, pair: usize, which: usize, volume: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_output_volume(pair, which, volume));
        Ok(())
    }

    fn set_output_mute(&mut self, pair: usize, which: usize, mute: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_output_mute(pair, which, mute));
        Ok(())
    }

    fn set_output_solo(&mut self, pair: usize, which: usize, solo: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_output_solo(pair, which, solo));
        Ok(())
    }

    fn set_input_volume(
        &mut self,
        pair: usize,
        which: usize,
        output: usize,
        volume: f32,
    ) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_volume(pair, which, output, volume));
        Ok(())
    }

    fn set_input_balance(&mut self, pair: usize, output: usize, balance: i8) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_balance(pair, output, balance));
        Ok(())
    }

    fn set_pair_block_balance(
        &mut self,
        channel: ChannelId,
        block: usize,
        balance: i8,
    ) -> Result<(), Error> {
        self.run_checked(move |d| d.set_pair_block_balance(channel, block, balance));
        Ok(())
    }

    fn set_input_mute(&mut self, pair: usize, which: usize, mute: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_mute(pair, which, mute));
        Ok(())
    }

    fn set_input_solo(&mut self, pair: usize, which: usize, solo: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_solo(pair, which, solo));
        Ok(())
    }

    fn set_playback_volume(
        &mut self,
        pair: usize,
        which: usize,
        output: usize,
        volume: f32,
    ) -> Result<(), Error> {
        self.run_checked(move |d| d.set_playback_volume(pair, which, output, volume));
        Ok(())
    }

    fn set_playback_balance(
        &mut self,
        pair: usize,
        output: usize,
        balance: i8,
    ) -> Result<(), Error> {
        self.run_checked(move |d| d.set_playback_balance(pair, output, balance));
        Ok(())
    }

    fn set_playback_mute(&mut self, pair: usize, which: usize, mute: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_playback_mute(pair, which, mute));
        Ok(())
    }

    fn set_playback_solo(&mut self, pair: usize, which: usize, solo: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_playback_solo(pair, which, solo));
        Ok(())
    }

    fn set_phantom(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_phantom(idx, on));
        Ok(())
    }

    fn set_pad(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_pad(idx, on));
        Ok(())
    }

    fn set_gain(&mut self, idx: usize, gain: u32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_gain(idx, gain));
        Ok(())
    }

    fn set_input_gain(&mut self, idx: usize, gain: u32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_gain(idx, gain));
        Ok(())
    }

    fn set_input_gain_right(&mut self, idx: usize, gain: u32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_gain_right(idx, gain));
        Ok(())
    }

    fn set_autoset(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_autoset(idx, on));
        Ok(())
    }

    fn set_instrument(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_instrument(idx, on));
        Ok(())
    }

    fn set_input_instrument(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_instrument(idx, on));
        Ok(())
    }

    fn set_input_phantom(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_phantom(idx, on));
        Ok(())
    }

    fn set_input_pad(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_pad(idx, on));
        Ok(())
    }

    fn set_input_level(&mut self, idx: usize, level: InputLevel) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_level(idx, level));
        Ok(())
    }

    fn set_ref_level(&mut self, idx: usize, raw: u16) -> Result<(), Error> {
        self.run_checked(move |d| d.set_ref_level(idx, raw));
        Ok(())
    }

    fn set_stereo_split(&mut self, pb: usize, split: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_stereo_split(pb, split));
        Ok(())
    }

    fn set_channel_fx_send(&mut self, channel: ChannelId, db: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_channel_fx_send(channel, db));
        Ok(())
    }

    fn set_fx_send(&mut self, db: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_fx_send(db));
        Ok(())
    }

    fn set_fx_return(&mut self, output: usize, db: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_fx_return(output, db));
        Ok(())
    }

    fn set_trim(&mut self, channel: ChannelId, db: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_trim(channel, db));
        Ok(())
    }

    fn set_phase(&mut self, idx: usize, invert: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_phase(idx, invert));
        Ok(())
    }

    fn set_output_phase(&mut self, idx: usize, invert: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_output_phase(idx, invert));
        Ok(())
    }

    fn set_no_trim(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_no_trim(idx, on));
        Ok(())
    }

    fn set_width(&mut self, width: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_width(width));
        Ok(())
    }

    fn set_channel_width(&mut self, channel: ChannelId, width: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_channel_width(channel, width));
        Ok(())
    }

    fn set_ms_proc(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_ms_proc(on));
        Ok(())
    }

    fn set_an12(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_an12(on));
        Ok(())
    }

    fn set_input_link(&mut self, linked: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_input_link(linked));
        Ok(())
    }

    fn set_eq_enabled(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_enabled(idx, on));
        Ok(())
    }

    fn set_eq_band_freq(&mut self, idx: usize, band: usize, freq: u16) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_band_freq(idx, band, freq));
        Ok(())
    }

    fn set_eq_band_q(&mut self, idx: usize, band: usize, q: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_band_q(idx, band, q));
        Ok(())
    }

    fn set_eq_band_gain(&mut self, idx: usize, band: usize, gain: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_band_gain(idx, band, gain));
        Ok(())
    }

    fn set_eq_band_type(&mut self, idx: usize, band: usize, kind: EqBandType) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_band_type(idx, band, kind));
        Ok(())
    }

    fn set_eq_low_cut_enabled(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_low_cut_enabled(idx, on));
        Ok(())
    }

    fn set_eq_low_cut_freq(&mut self, idx: usize, freq: u16) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_low_cut_freq(idx, freq));
        Ok(())
    }

    fn set_eq_low_cut_slope(&mut self, idx: usize, slope: u8) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_low_cut_slope(idx, slope));
        Ok(())
    }

    fn set_output_eq(&mut self, idx: usize, eq: &InputEq) -> Result<(), Error> {
        let eq = eq.clone();
        self.run_checked(move |d| d.set_output_eq(idx, &eq));
        Ok(())
    }

    fn apply_input_eq(&mut self, idx: usize, eq: &InputEq) -> Result<(), Error> {
        let eq = eq.clone();
        self.run_checked(move |d| d.apply_input_eq(idx, &eq));
        Ok(())
    }

    fn set_dynamics(&mut self, channel: ChannelId, dynamics: &Dynamics) -> Result<(), Error> {
        let dynamics = dynamics.clone();
        self.run_checked(move |d| d.set_dynamics(channel, &dynamics));
        Ok(())
    }

    fn set_reverb(&mut self, reverb: &Reverb) -> Result<(), Error> {
        let reverb = reverb.clone();
        self.run_checked(move |d| d.set_reverb(&reverb));
        Ok(())
    }

    fn set_echo(&mut self, echo: &Echo) -> Result<(), Error> {
        let echo = echo.clone();
        self.run_checked(move |d| d.set_echo(&echo));
        Ok(())
    }

    fn set_sample_rate(&mut self, rate: u32) -> Result<(), Error> {
        // Update the local rate at once: the rate-change reapply captures the
        // mirror scene immediately after this call.  A stale rate in that
        // capture would be written back to the unit when the reapply fires.
        self.scene.settings.sample_rate = rate;
        self.run_checked(move |d| d.set_sample_rate(rate));
        Ok(())
    }

    fn set_optical_out_format(&mut self, spdif: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_optical_out_format(spdif));
        Ok(())
    }

    fn set_eq_for_record(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_eq_for_record(on));
        Ok(())
    }

    fn set_loopback(&mut self, out: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_loopback(out, on));
        Ok(())
    }

    fn set_loopback_channel(&mut self, channel: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_loopback_channel(channel, on));
        Ok(())
    }

    fn set_cue(&mut self, idx: usize, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_cue(idx, on));
        Ok(())
    }

    fn set_dim(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_dim(on));
        Ok(())
    }

    fn set_dim_with(&mut self, on: bool, amount_db: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_dim_with(on, amount_db));
        Ok(())
    }

    fn set_main_recall(&mut self, db: f32) -> Result<(), Error> {
        self.run_checked(move |d| d.set_main_recall(db));
        Ok(())
    }

    fn set_main_out(&mut self, pair: Option<usize>) -> Result<(), Error> {
        self.run_checked(move |d| d.set_main_out(pair));
        Ok(())
    }

    fn set_main_out_b(&mut self, pair: Option<usize>) -> Result<(), Error> {
        self.run_checked(move |d| d.set_main_out_b(pair));
        Ok(())
    }

    fn set_mute_fx(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_mute_fx(on));
        Ok(())
    }

    fn set_external_input(
        &mut self,
        on: bool,
        source: Option<usize>,
        gain_db: f32,
    ) -> Result<(), Error> {
        self.run_checked(move |d| d.set_external_input(on, source, gain_db));
        Ok(())
    }

    fn set_cue_target(&mut self, pair: Option<usize>) -> Result<(), Error> {
        self.run_checked(move |d| d.set_cue_target(pair));
        Ok(())
    }

    fn set_pfl(&mut self, channel: ChannelId, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_pfl(channel, on));
        Ok(())
    }

    fn set_pfl_mode(&mut self, pfl: bool, exclusive: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_pfl_mode(pfl, exclusive));
        Ok(())
    }

    fn set_speaker_b(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_speaker_b(on));
        Ok(())
    }

    fn set_mono(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_mono(on));
        Ok(())
    }

    fn set_speaker_b_linked(&mut self, on: bool) -> Result<(), Error> {
        self.run_checked(move |d| d.set_speaker_b_linked(on));
        Ok(())
    }
}

fn missing(channel: ChannelId) -> Error {
    Error::InvalidChannel(format!("{channel:?} is not in the mirror"))
}

fn stopped() -> Error {
    Error::InvalidChannel("the device service has stopped".into())
}

/// Meter values written by the service and read by the fader canvases.
///
/// `None` for `at` means no meter packet has arrived yet.  The canvases use
/// `at` to interpolate between snapshots without a UI message.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeterSnapshot {
    pub inputs: [f32; 18],
    pub playbacks: [f32; 18],
    pub outputs: [f32; 18],
    /// Peak-hold values for the same channels, held and then falling as
    /// `ballistics` says while the service polls at the device rate.  A canvas may only draw 30 frames a second; these catch
    /// transients that fall between those frames.
    pub input_peaks: [f32; 18],
    pub playback_peaks: [f32; 18],
    pub output_peaks: [f32; 18],
    /// Device-backed input OVR flags: the analog low-word event or the digital
    /// over tag.  `None` means the backend has no input OVR word; the fader
    /// falls back to its peak threshold.
    pub input_ovr: Option<[bool; 18]>,
    /// Device-backed playback OVR flags, gated per the record's own
    /// consecutive-over count (the device counts the samples above full
    /// scale itself).  `None` means the backend has no OVR word; the fader
    /// falls back to its peak threshold.
    pub playback_ovr: Option<[bool; 18]>,
    /// Device-backed hardware-output OVR flags.
    pub output_ovr: Option<[bool; 18]>,
    pub fx: [f32; 2],
    pub fx_overload: [bool; 2],
    /// Whether the device is still connected.  A disconnected device has no
    /// signal: canvases fall to zero instead of freezing the last frame.
    pub connected: bool,
    /// Driver transport health, when the backend exposes it.
    pub counters: Option<HwdepCounters>,
    /// The rate the unit reports it is set to (device status), when known.
    pub device_rate: Option<u32>,
    /// Lock, sync and detected rate for each digital input, when the
    /// backend reports them.  `None` means the input carries no signal.
    pub input_sync: [Option<InputSyncState>; 3],
    /// Whether anything is visibly moving.  The service derives this from
    /// the levels, the held peaks and the FX values; a canvas that sees
    /// `false` stops scheduling frames and waits for [`meter_stream`].
    pub active: bool,
    pub at: Option<Instant>,
    /// The device's TotalMix meter ballistics, for the canvases' bar
    /// release.
    pub ballistics: MeterBallistics,
    /// The user's "VU fallback" RMS bar release choice, in dB/s.  `None`
    /// keeps the device's measured bar release.  A normal workspace setting
    /// published on the shared bus; the service applies it to
    /// `ballistics.bar_release_db_s`, so the canvases read the ballistics
    /// unchanged.  Only the bar release is affected — the peak hold and
    /// peak release keep the device ballistics.
    pub vu_release_db_s: Option<u32>,
}

/// A level or held peak above this keeps the meter canvases animating.
/// The visible meter floor is -60 dB (0.001); the threshold sits a little
/// below it so a decaying peak stops shortly after it leaves the scale.
const METER_ACTIVE_FLOOR: f32 = 0.0005;

/// Interim OVR count until the global host setting is wired into the
/// service.  This is the TotalMix default.
const OVR_FULL_SCALE_SAMPLES: u8 = 3;

/// Gate the device's own consecutive-over counts.  The UC counts the
/// samples at or above full scale inside each record window and clears
/// the count with the next sample below full scale, so an over flag in
/// the snapshot is the device's per-window count compared with the
/// global OVR sample preference; the host runs no sample counting of
/// its own.
fn over_flags(held: &mut Option<[bool; 18]>, fresh: Option<&[u8]>) {
    let Some(fresh) = fresh else {
        *held = None;
        return;
    };
    let out = held.get_or_insert([false; 18]);
    for i in 0..18 {
        out[i] = fresh.get(i).copied().unwrap_or(0) >= OVR_FULL_SCALE_SAMPLES;
    }
}

/// Whether the meter snapshot has anything worth drawing.  `at` is not a
/// change signal: it advances every device packet even in silence.
fn meter_activity(m: &MeterSnapshot) -> bool {
    let hot = |values: &[f32]| values.iter().any(|v| *v > METER_ACTIVE_FLOOR);
    hot(&m.inputs)
        || hot(&m.playbacks)
        || hot(&m.outputs)
        || hot(&m.input_peaks)
        || hot(&m.playback_peaks)
        || hot(&m.output_peaks)
        || hot(&m.fx)
}

/// A running device service.
pub struct DeviceService {
    commands: CommandSender,
    snapshot: Arc<Mutex<Snapshot>>,
    meters: Arc<Mutex<MeterSnapshot>>,
    updates: Option<Receiver<Snapshot>>,
    join: Option<JoinHandle<()>>,
}

/// Whether the device service owns the device.  The running application
/// defaults to it; `TOTALCONTROL_DEVICE_SERVICE=0` selects the old direct path.
/// Unit tests stay on the direct path unless they call
/// `attach_device_service` themselves, so the suite does not spawn a
/// service thread per case.
pub fn requested() -> bool {
    match std::env::var_os("TOTALCONTROL_DEVICE_SERVICE") {
        Some(value) => value != "0" && value != "false",
        None => !cfg!(test),
    }
}

impl DeviceService {
    /// Take ownership of `device` and start the service thread.
    pub fn spawn(mut device: DeviceHandle) -> Self {
        let initial = live_scene(&device);
        let info = capture_info(&device);
        let snapshot = Arc::new(Mutex::new(Snapshot {
            scene: initial,
            info,
            connected: device.is_connected(),
            hardware_volume: device.hardware_volume(),
            panel_selection: device.panel_selection(),
            at: Instant::now(),
            errors: Vec::new(),
        }));
        let meters = Arc::new(Mutex::new(MeterSnapshot::default()));
        let inbox = Arc::new(CommandInbox::new());
        let commands = CommandSender(Arc::clone(&inbox));
        let (updates_tx, updates_rx) = mpsc::channel();

        let thread_snapshot = Arc::clone(&snapshot);
        let thread_meters = Arc::clone(&meters);
        let join = thread::Builder::new()
            .name("totalcontrol-device-service".into())
            .spawn(move || {
                run(
                    &mut device,
                    inbox,
                    thread_snapshot,
                    thread_meters,
                    updates_tx,
                );
            })
            .expect("starting the device service");

        Self {
            commands,
            snapshot,
            meters,
            updates: Some(updates_rx),
            join: Some(join),
        }
    }

    /// The current model snapshot.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Take the update receiver; the UI drains it and applies the snapshots
    /// to its `Mirror`.
    pub fn take_updates(&mut self) -> Option<Receiver<Snapshot>> {
        self.updates.take()
    }

    /// A clone of the command sender, for a `Mirror`.
    pub fn command_sender(&self) -> CommandSender {
        self.commands.clone()
    }

    /// The shared meter bus, for a `Mirror` and the fader canvases.
    pub fn meter_bus(&self) -> Arc<Mutex<MeterSnapshot>> {
        Arc::clone(&self.meters)
    }

    /// The current meter snapshot.
    pub fn meters(&self) -> MeterSnapshot {
        *self.meters.lock().unwrap()
    }

    /// Queue a command.  Returns false once the service has stopped.
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }

    /// Ask the thread to stop and wait for it. Stop preempts any queued
    /// backlog.
    pub fn stop(mut self) {
        self.commands.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for DeviceService {
    fn drop(&mut self) {
        self.commands.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Capture the live model for the service snapshot.  The scene carries
/// the active monitor states and pre-Dim levels so the UI can latch the
/// Control Room buttons and a workspace can save them.
fn live_scene(device: &DeviceHandle) -> Scene {
    let mut scene = device.capture_scene();
    scene.settings.dim = device.settings().dim;
    scene.settings.speaker_b = device.settings().speaker_b;
    scene
}

/// The matrix block pair the mirror must use for an internal output pair.
///
/// The UCX stores the matrix in display/DSP order (Phones first), while the
/// output array and `sel_out` use the internal order.  The service applies
/// pair operations with the backend's mapping, so a stored balance is read
/// from the same block only when the mirror uses that mapping too.
fn mirror_matrix_blocks(
    output_display_order: &[usize],
    matrix_uses_display_order: bool,
    pair: usize,
) -> [usize; 2] {
    if matrix_uses_display_order {
        return [pair * 2, pair * 2 + 1];
    }
    output_display_order
        .iter()
        .position(|&internal| internal == pair)
        .map(|display| [display * 2, display * 2 + 1])
        .unwrap_or([pair * 2, pair * 2 + 1])
}

fn capture_info(device: &DeviceHandle) -> DeviceInfo {
    DeviceInfo {
        model_name: device.model_name().to_string(),
        canonical_model: device.canonical_model().to_string(),
        sample_rates: device.sample_rates(),
        capabilities: device.capabilities(),
        protocol_features: device.protocol_features(),
        class_compliant: device.class_compliant(),
        low_cut_supported: device.low_cut_supported(),
        phase_supported: device.phase_supported(),
        output_balance_supported: device.output_balance_supported(),
        loopback_supported: device.loopback_supported(),
        stereo_split_supported: device.stereo_split_supported(),
        matrix_uses_display_order: device.matrix_uses_display_order(),
        output_pair_count: device.output_pair_count(),
        output_display_order: device.output_display_order(),
        is_uc: device.is_uc_model(),
        is_ucx: device.is_ucx_model(),
        is_uc_family: device.is_uc_family(),
        is_mock: device.is_mock(),
        kind: device.kind(),
        target: device.target(),
        personality: device.personality_label(),
        card_id: device.card_id(),
        firmware: device.firmware(),
        serial: device.serial().map(str::to_owned),
        rate_follows: device.rate_follows_sound_server(),
        has_sensitivity: device.has_sensitivity_control(),
        outputs_one_per_pair: device.outputs_one_per_pair(),
        input_meters: (0..device.inputs().len())
            .map(|i| device.has_input_meter(i))
            .collect(),
        device_playback_meters: device.has_device_playback_meters(),
    }
}

fn run(
    device: &mut DeviceHandle,
    inbox: Arc<CommandInbox>,
    snapshot: Arc<Mutex<Snapshot>>,
    meters: Arc<Mutex<MeterSnapshot>>,
    updates: Sender<Snapshot>,
) {
    let mut last_scene = Instant::now();
    let mut last_connected = device.is_connected();
    let mut last_model = snapshot.lock().unwrap().scene.clone();
    let mut last_hw_volume = device.hardware_volume();
    let mut last_panel_selection = device.panel_selection();
    let mut last_meter_active = false;
    let mut last_overload = [false; 2];
    let mut last_counters = Instant::now() - Duration::from_secs(1);
    let mut last_rate = Instant::now() - Duration::from_secs(1);
    let mut last_sync = Instant::now() - Duration::from_secs(1);
    let mut peak_at = [[(Instant::now(), 0.0); 18]; 3];

    loop {
        if inbox.stopped() {
            break;
        }
        // The timeout paces the polling loop at about 120 Hz while still
        // waking immediately for a command.
        match inbox.recv_timeout(Duration::from_millis(8)) {
            Some(Command::Stop) => break,
            Some(command) => apply(device, command, &inbox),
            None => {}
        }
        for command in inbox.drain() {
            // Stop preempts the backlog even mid-drain: the flag is checked
            // per command, not only at the loop top, so a queued batch of
            // slow closures never delays shutdown by its full duration.
            if inbox.stopped() || matches!(command, Command::Stop) {
                return;
            }
            apply(device, command, &inbox);
        }
        if inbox.stopped() {
            break;
        }

        // A write that failed on the service thread is published with the
        // next snapshot so the UI can show it.  The error list is cleared
        // from the stored snapshot so a later scene publish does not repeat
        // the notice.
        let errors = inbox.drain_errors();
        if !errors.is_empty() {
            let update = {
                let mut s = snapshot.lock().unwrap();
                s.errors = errors;
                s.at = Instant::now();
                let mut update = s.clone();
                s.errors.clear();
                update
            };
            let _ = updates.send(update);
            snapshot_notify().notify_one();
        }

        // State/status events feed the model and the front panel.
        let _ = device.poll_events();

        // Meters are published independently of the model snapshot and of
        // the UI's view tick.
        {
            let now = Instant::now();
            let connected = device.is_connected();
            let inputs = if connected {
                device
                    .device_input_meters()
                    .unwrap_or_else(|| device.input_meters())
            } else {
                vec![0.0; 18]
            };
            let playbacks = if connected {
                device
                    .device_playback_meters()
                    .unwrap_or_else(|| device.playback_meters())
            } else {
                vec![0.0; 18]
            };
            let outputs = if connected {
                device.device_output_meters().unwrap_or_default()
            } else {
                vec![0.0; 18]
            };
            let input_peaks = if connected {
                device.input_peaks().unwrap_or_else(|| inputs.clone())
            } else {
                vec![0.0; 18]
            };
            let playback_peaks = if connected {
                device
                    .device_playback_peaks()
                    .unwrap_or_else(|| playbacks.clone())
            } else {
                vec![0.0; 18]
            };
            let playback_over_runs = if connected {
                device.device_playback_over_runs()
            } else {
                None
            };
            let output_over_runs = if connected {
                device.device_output_over_runs()
            } else {
                None
            };
            let input_ovr = if connected {
                device.device_input_ovr()
            } else {
                None
            };
            let output_peaks = if connected {
                device
                    .device_output_peaks()
                    .unwrap_or_else(|| outputs.clone())
            } else {
                vec![0.0; 18]
            };
            let fx = if connected {
                device.fx_meters().unwrap_or([0.0, 0.0])
            } else {
                [0.0, 0.0]
            };
            let overload = if connected {
                device.fx_overload().unwrap_or([false, false])
            } else {
                [false, false]
            };
            let over_flags_pb = if connected {
                device.device_playback_over_levels()
            } else {
                None
            };
            let over_flags_out = if connected {
                device.device_output_over_levels(true)
            } else {
                None
            };
            let mut m = meters.lock().unwrap();
            for (slot, value) in m.inputs.iter_mut().zip(inputs) {
                *slot = value;
            }
            for (slot, value) in m.playbacks.iter_mut().zip(playbacks) {
                *slot = value;
            }
            for (slot, value) in m.outputs.iter_mut().zip(outputs) {
                *slot = value;
            }
            // The red band (0..+6 dB) merges into the row bar: the device
            // counter's inverse value, or the row's written fader and
            // matrix state once the device reports the row over.  The
            // release fall brings the bar back below 0 dB when the band
            // clears.
            merge_over(m.playbacks.iter_mut(), over_flags_pb.as_deref());
            merge_over(m.outputs.iter_mut(), over_flags_out.as_deref());
            m.ballistics = device.meter_ballistics();
            if let Some(v) = m.vu_release_db_s {
                m.ballistics.bar_release_db_s = v as f32;
            }
            let b = m.ballistics;
            hold_peaks(&mut m.input_peaks, &input_peaks, &mut peak_at[0], b, now);
            hold_peaks(
                &mut m.playback_peaks,
                &playback_peaks,
                &mut peak_at[1],
                b,
                now,
            );
            hold_peaks(&mut m.output_peaks, &output_peaks, &mut peak_at[2], b, now);
            m.input_ovr = input_ovr.map(|flags| {
                let mut out = [false; 18];
                for (i, flag) in out.iter_mut().enumerate() {
                    *flag = flags.get(i).copied().unwrap_or(false);
                }
                out
            });
            over_flags(&mut m.playback_ovr, playback_over_runs.as_deref());
            over_flags(&mut m.output_ovr, output_over_runs.as_deref());
            m.fx = fx;
            m.fx_overload = overload;
            m.connected = connected;
            m.at = Some(now);
            // The transport counters are cumulative and change slowly; a
            // quarter-second poll is enough for the device sheet.
            if last_counters.elapsed() >= Duration::from_millis(250) {
                last_counters = Instant::now();
                m.counters = device.transport_counters();
            }
            // The device rate is polled at 50 ms so the rate-switch
            // feedback clears promptly once the unit accepts the rate
            // (the rate value is the confirmation; internal clock).
            if last_rate.elapsed() >= Duration::from_millis(50) {
                last_rate = Instant::now();
                m.device_rate = device.device_rate();
            }
            // Digital-input lock and rate: diagnostic, not a control.  A
            // quarter-second poll is enough for the Audio Config field.
            if last_sync.elapsed() >= Duration::from_millis(250) {
                last_sync = Instant::now();
                let sync = device.input_sync_status();
                if sync != m.input_sync {
                    log::info!("digital input sync: {sync:?}");
                }
                m.input_sync = sync;
            }
            let active = meter_activity(&m);
            m.active = active;
            drop(m);
            // Only edges wake the UI: while a burst is running the canvases
            // schedule their own frames.  A latched overload flag must not
            // keep the loop alive, so overload changes are their own edge.
            let overload_changed = overload != last_overload;
            if (active && !last_meter_active) || overload_changed {
                meter_notify().notify_one();
            }
            last_meter_active = active;
            last_overload = overload;
        }

        // The model changes rarely (a control command, a front-panel move,
        // a gain readback).  A 20 Hz capture is plenty for the UI.
        if last_scene.elapsed() >= Duration::from_millis(50) {
            last_scene = Instant::now();
            let connected = device.is_connected();
            let scene = live_scene(device);
            let hardware_volume = device.hardware_volume();
            let panel_selection = device.panel_selection();
            if scene != last_model
                || connected != last_connected
                || hardware_volume != last_hw_volume
                || panel_selection != last_panel_selection
            {
                last_model = scene.clone();
                last_connected = connected;
                last_hw_volume = hardware_volume;
                last_panel_selection = panel_selection;
                let update = {
                    let mut s = snapshot.lock().unwrap();
                    s.scene = scene;
                    s.connected = connected;
                    s.hardware_volume = hardware_volume;
                    s.panel_selection = panel_selection;
                    s.at = Instant::now();
                    let mut update = s.clone();
                    s.errors.clear();
                    update
                };
                let _ = updates.send(update);
                snapshot_notify().notify_one();
            }
        }
    }
}

/// Merge the red-band (0..+6 dB) bar values into the row bars.  A row
/// whose over feed carries a value takes that value when it exceeds the
/// row's own 0 dB state; the meter's release fall brings the bar back
/// when the band clears.
fn merge_over<'a, I>(slots: I, over: Option<&[Option<f32>]>)
where
    I: Iterator<Item = &'a mut f32>,
{
    let Some(over) = over else { return };
    for (slot, value) in slots.zip(over) {
        if let Some(value) = value {
            *slot = slot.max(*value);
        }
    }
}

/// Hold each channel's peak for the device's hold time, then fall at its
/// peak release.  The service sees every device packet, so a transient
/// shorter than a canvas frame still reaches the needle.
///
/// `held` keeps when each peak was caught and its level then.  The fall is
/// computed from that level, not from the last published one: this runs on
/// every ~8 ms poll, and decaying the published value by the whole time
/// since the hold compounded to about 600 dB in the first second.
fn hold_peaks(
    peaks: &mut [f32; 18],
    fresh: &[f32],
    held: &mut [(Instant, f32); 18],
    ballistics: MeterBallistics,
    now: Instant,
) {
    for (i, slot) in peaks.iter_mut().enumerate() {
        let value = fresh.get(i).copied().unwrap_or(0.0).max(0.0);
        if value >= *slot {
            *slot = value;
            held[i] = (now, value);
        } else {
            let (since, level) = held[i];
            let past = now.duration_since(since).as_secs_f32() - ballistics.peak_hold_s;
            if past > 0.0 {
                let fall = 10f32.powf(-ballistics.peak_release_db_s * past / 20.0);
                *slot = (level * fall).max(value);
            }
        }
    }
}

fn apply(device: &mut DeviceHandle, command: Command, inbox: &CommandInbox) {
    match command {
        Command::SetVolume(channel, output, value) => {
            if let Err(e) = device.set_volume(channel, output, value) {
                inbox.report_error(e.to_string());
            }
        }
        Command::SetMute(channel, mute) => {
            if let Err(e) = device.set_mute(channel, mute) {
                inbox.report_error(e.to_string());
            }
        }
        Command::ApplyScene(scene) => {
            if let Err(e) = device.apply_scene(&scene) {
                inbox.report_error(e.to_string());
            }
        }
        Command::Run(f) => f(device),
        Command::Stop => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        mirror_matrix_blocks, Command, CommandInbox, DeviceService, MeterBallistics, Mirror,
        COMMAND_CAP,
    };
    use crate::app::DeviceHandle;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use totalcontrol_core::{ChannelId, MockDevice, RmeDevice};

    fn mock() -> DeviceHandle {
        DeviceHandle::Mock(MockDevice::open_ucx().expect("mock opens"))
    }

    #[test]
    fn the_command_inbox_never_blocks_and_keeps_the_latest_volume() {
        let inbox = CommandInbox::new();
        let start = Instant::now();
        for i in 0..10_000 {
            assert!(inbox.send(Command::SetVolume(ChannelId::Input(0), 0, i as f32)));
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "sending from the GUI thread never waits"
        );
        let q = inbox.queue.lock().unwrap();
        assert!(q.len() <= COMMAND_CAP);
        assert_eq!(
            q.iter().filter(|c| matches!(c, Command::SetVolume(..))).count(),
            1
        );
        assert!(q
            .iter()
            .any(|c| matches!(c, Command::SetVolume(_, _, v) if *v == 9_999.0)));
    }

    #[test]
    fn the_mirror_uses_the_ucx_matrix_block_order() {
        // UCX internal output order: Phones, AN1/2, AN3/4, AN5/6...
        // Display/DSP order: AN1/2, AN3/4, AN5/6, Phones...
        let order = vec![1, 2, 3, 0, 4, 5, 6, 7, 8];
        assert_eq!(mirror_matrix_blocks(&order, false, 1), [0, 1]);
        assert_eq!(mirror_matrix_blocks(&order, false, 3), [4, 5]);
        assert_eq!(mirror_matrix_blocks(&order, false, 0), [6, 7]);
        // The UC already stores its channels in display order.
        assert_eq!(mirror_matrix_blocks(&order, true, 3), [6, 7]);
    }

    fn wait_for(mut check: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if check() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_failed_device_write_reaches_the_mirror() {
        let mut service = DeviceService::spawn(mock());
        assert!(
            wait_for(|| !service.snapshot().scene.outputs.is_empty()),
            "the initial snapshot arrives"
        );
        let mut updates = service.take_updates().expect("the update receiver");
        let mut mirror = Mirror::new(
            service.snapshot(),
            service.command_sender(),
            service.meter_bus(),
        );
        let report = service.command_sender();
        assert!(service.send(Command::Run(Box::new(move |_| {
            report.report_error("write rejected".into());
        }))));

        let start = Instant::now();
        let mut seen = None;
        while start.elapsed() < Duration::from_secs(2) {
            if let Ok(snapshot) = updates.recv_timeout(Duration::from_millis(50)) {
                mirror.update(snapshot);
                if let Some(error) = mirror.take_error() {
                    seen = Some(error);
                    break;
                }
            }
        }
        assert_eq!(seen.as_deref(), Some("write rejected"));
        service.stop();
    }

    #[test]
    fn the_service_publishes_a_snapshot_and_applies_commands() {
        let service = DeviceService::spawn(mock());

        assert!(
            wait_for(|| !service.snapshot().scene.outputs.is_empty()),
            "the initial snapshot arrives"
        );
        assert!(service.snapshot().connected);

        assert!(service.send(Command::SetVolume(ChannelId::Output(0), 0, 0.25)));
        assert!(
            wait_for(|| (service.snapshot().scene.outputs[0].volume - 0.25).abs() < 0.001),
            "the model snapshot follows the volume command"
        );

        assert!(service.send(Command::SetMute(ChannelId::Input(0), true)));
        assert!(
            wait_for(|| service.snapshot().scene.inputs[0].mute),
            "the model snapshot follows the mute command"
        );

        service.stop();
    }

    /// The mirror's rate setter updates the local scene at once.  The
    /// rate-change reapply captures the mirror scene right after the setter;
    /// a stale rate in that capture would be written back to the unit when
    /// the reapply fires.
    #[test]
    fn the_mirror_rate_setter_updates_the_local_scene_at_once() {
        let service = DeviceService::spawn(mock());
        assert!(
            wait_for(|| !service.snapshot().scene.outputs.is_empty()),
            "the initial snapshot arrives"
        );
        let mut mirror = Mirror::new(
            service.snapshot(),
            service.command_sender(),
            service.meter_bus(),
        );
        let before = mirror.settings().sample_rate;
        let rate = if before == 32_000 { 48_000 } else { 32_000 };
        mirror.set_sample_rate(rate).expect("rate setter");
        assert_eq!(mirror.settings().sample_rate, rate);
        assert_eq!(mirror.capture_scene().settings.sample_rate, rate);
        assert!(
            wait_for(|| service.snapshot().scene.settings.sample_rate == rate),
            "the service model follows the rate command"
        );
        service.stop();
    }

    #[test]
    fn meters_are_published_without_the_model_changing() {
        let service = DeviceService::spawn(mock());
        assert!(
            wait_for(|| service.meters().at.is_some()),
            "the meter snapshot is written"
        );
        assert!(service.meters().inputs.iter().any(|v| *v >= 0.0));
        service.stop();
    }

    /// A `Run` closure executes on the service thread; an intentionally
    /// slow closure pauses publication for its duration. Device I/O is
    /// queued on the control-I/O worker (core regression test
    /// `live_writes_return_before_the_device_pacing`); this test guards
    /// the loop's structural property.
    #[test]
    fn a_slow_command_freezes_meter_publication() {
        let service = DeviceService::spawn(mock());
        assert!(
            wait_for(|| service.meters().at.is_some()),
            "the meter snapshot is written"
        );

        assert!(service.send(Command::Run(Box::new(|_| {
            thread::sleep(Duration::from_millis(500));
        }))));

        // Publication normally advances every loop pass (~8 ms). Two
        // equal samples 80 ms apart mean the loop is stalled.
        assert!(
            wait_for(|| {
                let a = service.meters().at;
                thread::sleep(Duration::from_millis(80));
                a == service.meters().at
            }),
            "meter publication froze while the slow command ran"
        );
        let frozen_at = service.meters().at.unwrap();
        assert!(
            wait_for(|| service.meters().at.unwrap() > frozen_at + Duration::from_millis(300)),
            "publication resumes after the command completes"
        );
        service.stop();
    }

    /// `Run` closures execute serially on the service thread, so a
    /// volume move queued behind intentionally slow closures lands after
    /// them. Latest-value commands coalesce in the inbox; arbitrary
    /// closures keep their order.
    #[test]
    fn a_command_backlog_delays_later_commands() {
        let service = DeviceService::spawn(mock());
        assert!(
            wait_for(|| !service.snapshot().scene.outputs.is_empty()),
            "the initial snapshot arrives"
        );

        for _ in 0..20 {
            assert!(service.send(Command::Run(Box::new(|_| {
                thread::sleep(Duration::from_millis(25));
            }))));
        }
        let t0 = Instant::now();
        assert!(service.send(Command::SetVolume(ChannelId::Output(0), 0, 0.77)));
        assert!(
            wait_for(|| (service.snapshot().scene.outputs[0].volume - 0.77).abs() < 0.001),
            "the volume command eventually applies"
        );
        let lag = t0.elapsed();
        assert!(
            lag >= Duration::from_millis(400),
            "the backlog delayed the volume move by {lag:?}"
        );
        service.stop();
    }

    #[test]
    fn stop_preempts_a_command_backlog() {
        let service = DeviceService::spawn(mock());
        assert!(
            wait_for(|| !service.snapshot().scene.outputs.is_empty()),
            "the service starts"
        );
        for _ in 0..50 {
            assert!(service.send(Command::Run(Box::new(|_| {
                thread::sleep(Duration::from_millis(25));
            }))));
        }
        let t0 = Instant::now();
        service.stop();
        assert!(
            t0.elapsed() < Duration::from_millis(300),
            "stop preempted the backlog in {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn the_inbox_coalesces_latest_value_commands() {
        let inbox = CommandInbox::new();
        for k in 0..50 {
            assert!(inbox.send(Command::SetVolume(ChannelId::Output(0), 0, k as f32 / 50.0)));
        }
        assert!(inbox.send(Command::SetVolume(ChannelId::Output(1), 0, 0.9)));
        assert!(inbox.send(Command::SetMute(ChannelId::Input(0), true)));
        assert!(inbox.send(Command::SetMute(ChannelId::Input(0), false)));
        assert_eq!(inbox.len(), 3, "one slot per key, latest value wins");
        let cmds = inbox.drain();
        match &cmds[0] {
            Command::SetVolume(ChannelId::Output(0), 0, v) => {
                assert!((v - 49.0 / 50.0).abs() < 1e-6, "the latest volume survives");
            }
            _ => panic!("expected the latest volume"),
        }
        assert!(matches!(
            cmds[2],
            Command::SetMute(ChannelId::Input(0), false)
        ));
    }

    #[test]
    fn the_inbox_never_blocks_when_full() {
        let inbox = CommandInbox::new();
        for _ in 0..COMMAND_CAP {
            assert!(inbox.send(Command::Run(Box::new(|_| {}))));
        }
        assert_eq!(inbox.len(), COMMAND_CAP);
        let start = Instant::now();
        assert!(inbox.send(Command::Run(Box::new(|_| {}))));
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "the GUI sender never waits"
        );
        assert_eq!(inbox.len(), COMMAND_CAP, "the oldest command is dropped");
    }

    /// Bench evidence for the audit claim "a stalled control path drops
    /// device events". Run on a real unit:
    ///   cargo test -p totalcontrol-gui bench_stress_event_drops -- --ignored --nocapture
    /// Applies the current scene plus a 40-step volume drag on output 17,
    /// waits for each flood to drain, and repeats for 15 s. Reports the
    /// longest meter-publication freeze and the kernel event-drop counter
    /// delta. A freeze without drops narrows the claim; drops confirm it.
    /// Output 17's original volume is restored at the end.
    #[test]
    #[ignore]
    fn bench_stress_event_drops() {
        let device = match DeviceHandle::open_real() {
            Some(d) => d,
            None => {
                eprintln!("no real device present; nothing to measure");
                return;
            }
        };
        let service = DeviceService::spawn(device);
        assert!(wait_for(|| service.snapshot().connected), "device connects");
        let scene = service.snapshot().scene;
        let orig17 = scene.outputs[17].volume;
        eprintln!("output 17 volume before stress: {orig17}");

        let start = Instant::now();
        let mut drops0 = None;
        let mut longest_freeze = Duration::ZERO;
        let mut last_at = None;
        let mut freeze_start = None;
        let mut floods = 0u32;
        let (tx, rx) = std::sync::mpsc::channel();
        while start.elapsed() < Duration::from_secs(15) {
            let _ = service.send(Command::ApplyScene(Box::new(scene.clone())));
            for k in 0..40 {
                let v = 0.3 + 0.4 * (k as f32 / 40.0);
                let _ = service.send(Command::SetVolume(ChannelId::Output(17), 0, v));
            }
            let done = tx.clone();
            let _ = service.send(Command::Run(Box::new(move |_| {
                let _ = done.send(());
            })));
            // Sample publication while this flood drains.
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                let m = service.meters();
                if let Some(c) = &m.counters {
                    if drops0.is_none() {
                        drops0 = Some(c.events_dropped);
                    }
                }
                match (last_at, m.at) {
                    (Some(prev), Some(now)) if now == prev => {
                        if freeze_start.is_none() {
                            freeze_start = Some(Instant::now());
                        }
                        if let Some(fs) = freeze_start {
                            longest_freeze = longest_freeze.max(fs.elapsed());
                        }
                    }
                    (_, Some(now)) => {
                        last_at = Some(now);
                        freeze_start = None;
                    }
                    _ => {}
                }
                if rx.try_recv().is_ok() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            floods += 1;
            // A realistic scene-apply cadence: the worker drains each
            // scene's paced bursts within this gap.
            thread::sleep(Duration::from_millis(250));
        }
        let _ = service.send(Command::SetVolume(ChannelId::Output(17), 0, orig17));
        let done = tx.clone();
        let _ = service.send(Command::Run(Box::new(move |_| {
            let _ = done.send(());
        })));
        let _ = rx.recv_timeout(Duration::from_secs(3));
        let drops1 = service
            .meters()
            .counters
            .map(|c| c.events_dropped)
            .unwrap_or(0);
        eprintln!(
            "stress: {floods} floods in 15 s, longest publication freeze {longest_freeze:?}, \
             events_dropped {} -> {drops1}",
            drops0.unwrap_or(0)
        );
        service.stop();
    }

    #[test]
    fn the_meter_bus_carries_the_device_ballistics() {
        let uc = DeviceService::spawn(DeviceHandle::Mock(MockDevice::open_uc().unwrap()));
        assert!(
            wait_for(|| uc.meters().at.is_some()),
            "the UC meter snapshot is written"
        );
        assert_eq!(uc.meters().ballistics, MeterBallistics::TOTALMIX_UC);
        uc.stop();
        let ucx = DeviceService::spawn(mock());
        assert!(
            wait_for(|| ucx.meters().at.is_some()),
            "the UCX meter snapshot is written"
        );
        assert_eq!(ucx.meters().ballistics, MeterBallistics::TOTALMIX_UCX);
        ucx.stop();
    }

    #[test]
    fn the_vu_fallback_setting_sets_the_bar_release_only() {
        let uc = DeviceService::spawn(DeviceHandle::Mock(MockDevice::open_uc().unwrap()));
        assert!(
            wait_for(|| uc.meters().at.is_some()),
            "the UC meter snapshot is written"
        );
        let bus = uc.meter_bus();
        bus.lock().unwrap().vu_release_db_s = Some(20);
        assert!(
            wait_for(|| uc.meters().ballistics.bar_release_db_s == 20.0),
            "the chosen bar release is applied"
        );
        let b = uc.meters().ballistics;
        assert_eq!(b.peak_hold_s, MeterBallistics::TOTALMIX_UC.peak_hold_s);
        assert_eq!(
            b.peak_release_db_s,
            MeterBallistics::TOTALMIX_UC.peak_release_db_s
        );
        bus.lock().unwrap().vu_release_db_s = None;
        assert!(
            wait_for(|| {
                uc.meters().ballistics.bar_release_db_s
                    == MeterBallistics::TOTALMIX_UC.bar_release_db_s
            }),
            "None restores the device's measured bar release"
        );
        uc.stop();
    }

    #[test]
    fn the_mirror_derives_routing_from_the_scene_level() {
        let mut service = DeviceService::spawn(mock());
        let updates = service.take_updates().expect("the update receiver");
        let mut mirror = super::Mirror::new(
            service.snapshot(),
            service.command_sender(),
            service.meter_bus(),
        );
        assert!(
            wait_for(|| {
                while let Ok(snapshot) = updates.try_recv() {
                    mirror.update(snapshot);
                }
                !mirror.inputs().is_empty()
            }),
            "the initial snapshot arrives"
        );
        // A factory input starts unrouted; a level creates the routing.
        assert!(!mirror.matrix_cell_routed(ChannelId::Input(0), 0));

        assert!(mirror.set_volume(ChannelId::Input(0), 0, 1.0));
        assert!(
            wait_for(|| {
                while let Ok(snapshot) = updates.try_recv() {
                    mirror.update(snapshot);
                }
                mirror.matrix_cell_routed(ChannelId::Input(0), 0)
            }),
            "a level creates the mirror's routing"
        );

        assert!(mirror.set_volume(ChannelId::Input(0), 0, 0.0));
        assert!(
            wait_for(|| {
                while let Ok(snapshot) = updates.try_recv() {
                    mirror.update(snapshot);
                }
                mirror.matrix_cell(ChannelId::Input(0), 0).is_none()
            }),
            "a zero send removes the mirror's routing"
        );
        assert!(!mirror.matrix_cell_routed(ChannelId::Input(0), 0));

        service.stop();
    }

    #[test]
    fn the_mirror_forwards_the_captured_capabilities() {
        let device = mock();
        let caps = device.capabilities();
        let order = device.output_display_order();
        let model = device.canonical_model().to_string();
        let service = DeviceService::spawn(device);
        let info = service.snapshot().info;
        assert_eq!(info.capabilities, caps);
        assert!(info.capabilities.fx, "the UCX mock has an FX bus");
        assert_eq!(info.output_display_order, order);
        assert_eq!(info.canonical_model, model);
        service.stop();
    }

    #[test]
    fn a_live_signal_marks_the_meter_bus_active() {
        let service = DeviceService::spawn(mock());
        assert!(
            wait_for(|| service.meters().active),
            "the mock's live levels mark the bus active"
        );
        service.stop();
    }

    #[test]
    fn meter_activity_ignores_silence_and_follows_levels_peaks_and_overload() {
        let mut m = super::MeterSnapshot::default();
        assert!(!super::meter_activity(&m), "silence is idle");
        m.inputs[0] = super::METER_ACTIVE_FLOOR * 0.5;
        assert!(!super::meter_activity(&m), "below the floor is idle");
        m.inputs[0] = super::METER_ACTIVE_FLOOR * 2.0;
        assert!(super::meter_activity(&m), "a level wakes the bus");
        m.inputs[0] = 0.0;
        m.output_peaks[17] = 0.01;
        assert!(super::meter_activity(&m), "a held peak keeps it awake");
        m.output_peaks[17] = 0.0;
        m.fx_overload[1] = true;
        assert!(
            !super::meter_activity(&m),
            "a latched overload is a static indicator, not ongoing movement"
        );
    }

    #[test]
    fn a_held_peak_falls_ten_db_per_second_at_the_service_poll_rate() {
        // The service calls `hold_peaks` on every ~8 ms poll; the fall
        // after the hold must not depend on how often it is called.
        let ucx = MeterBallistics::TOTALMIX_UCX;
        let start = Instant::now();
        let mut peaks = [0.0; 18];
        let mut at = [(start, 0.0); 18];
        super::hold_peaks(&mut peaks, &[0.8], &mut at, ucx, start);
        let mut now = start;
        while now.duration_since(start) < Duration::from_millis(3000) {
            now += Duration::from_millis(8);
            super::hold_peaks(&mut peaks, &[0.0], &mut at, ucx, now);
        }
        let fell = now.duration_since(start).as_secs_f32() - 2.0;
        let db = 20.0 * (peaks[0] / 0.8).log10();
        assert!(
            (db + 10.0 * fell).abs() < 0.2,
            "{db:.2} dB {fell:.3} s after the hold, want {:.2}",
            -10.0 * fell
        );
    }

    #[test]
    fn the_uc_peak_line_holds_two_seconds_then_falls_fifteen_db_per_second() {
        // TotalMix FX with the UC: a 10 ms burst at -6 dBFS holds its
        // line 2.0 s, then the line falls 15 dB/s.
        let uc = MeterBallistics::TOTALMIX_UC;
        let start = Instant::now();
        let mut peaks = [0.0; 18];
        let mut at = [(start, 0.0); 18];
        super::hold_peaks(&mut peaks, &[0.5], &mut at, uc, start);
        let mut now = start;
        while now.duration_since(start) < Duration::from_millis(3000) {
            now += Duration::from_millis(8);
            super::hold_peaks(&mut peaks, &[0.0], &mut at, uc, now);
        }
        let fell = now.duration_since(start).as_secs_f32() - 2.0;
        let db = 20.0 * (peaks[0] / 0.5).log10();
        assert!(
            (db + 15.0 * fell).abs() < 0.2,
            "{db:.2} dB {fell:.3} s after the hold, TotalMix falls {:.2}",
            -15.0 * fell
        );
    }

    #[test]
    fn peak_hold_keeps_a_transient_and_then_falls() {
        let ucx = MeterBallistics::TOTALMIX_UCX;
        let start = Instant::now();
        let mut peaks = [0.0; 18];
        let mut at = [(start, 0.0); 18];
        super::hold_peaks(&mut peaks, &[0.8], &mut at, ucx, start);
        assert!((peaks[0] - 0.8).abs() < 1e-6);

        // The bar may be at zero between frames; the peak is still held.
        super::hold_peaks(
            &mut peaks,
            &[0.0],
            &mut at,
            ucx,
            start + Duration::from_millis(900),
        );
        assert!((peaks[0] - 0.8).abs() < 1e-6);

        // Still held at 1.9 s: the default TotalMix hold is two seconds.
        super::hold_peaks(
            &mut peaks,
            &[0.0],
            &mut at,
            ucx,
            start + Duration::from_millis(1900),
        );
        assert!((peaks[0] - 0.8).abs() < 1e-6);

        // After the two-second hold it falls at 10 dB/s (a factor
        // 10^(−1/2) per second against the 20 dB ruler).
        super::hold_peaks(
            &mut peaks,
            &[0.0],
            &mut at,
            ucx,
            start + Duration::from_millis(2900),
        );
        assert!((peaks[0] - 0.8 * 0.1f32.powf(0.45)).abs() < 1e-4);
    }

    #[test]
    fn runtime_dim_latches_and_reaches_the_saved_scene() {
        let mut service = DeviceService::spawn(mock());
        let updates = service.take_updates().expect("the update receiver");
        let mut mirror = super::Mirror::new(
            service.snapshot(),
            service.command_sender(),
            service.meter_bus(),
        );

        assert!(!mirror.settings().dim);
        assert!(mirror.set_dim(true).is_ok());
        assert!(
            wait_for(|| {
                while let Ok(snapshot) = updates.try_recv() {
                    mirror.update(snapshot);
                }
                mirror.settings().dim
            }),
            "the live Dim flag reaches the mirror so the button can latch"
        );
        assert!(
            mirror.capture_scene().settings.dim,
            "the saved scene carries the active Dim flag"
        );

        assert!(mirror.set_dim(false).is_ok());
        assert!(
            wait_for(|| {
                while let Ok(snapshot) = updates.try_recv() {
                    mirror.update(snapshot);
                }
                !mirror.settings().dim
            }),
            "turning Dim off clears the live flag"
        );
        service.stop();
    }

    #[test]
    fn a_mirror_applies_updates_and_sends_commands() {
        let mut service = DeviceService::spawn(mock());
        let updates = service.take_updates().expect("the update receiver");
        let mut mirror = super::Mirror::new(
            service.snapshot(),
            service.command_sender(),
            service.meter_bus(),
        );
        assert!(!mirror.outputs().is_empty());
        assert_eq!(mirror.info().output_pair_count, mirror.outputs().len() / 2);

        assert!(mirror.set_volume(ChannelId::Output(0), 0, 0.25));
        assert!(
            wait_for(|| {
                while let Ok(snapshot) = updates.try_recv() {
                    mirror.update(snapshot);
                }
                (mirror.outputs()[0].volume - 0.25).abs() < 0.001
            }),
            "the mirror follows the service snapshot"
        );

        // A trait setter that goes through `Command::Run`.
        assert!(mirror.set_eq_band_gain(0, 0, -3.0).is_ok());
        assert!(
            wait_for(|| {
                while let Ok(snapshot) = updates.try_recv() {
                    mirror.update(snapshot);
                }
                mirror.inputs()[0]
                    .eq
                    .is_some_and(|eq| (eq.bands[0].gain_db + 3.0).abs() < 0.01)
            }),
            "a mirror setter reaches the service and comes back in the snapshot"
        );
        service.stop();
    }
}
