# TotalControl architecture

This is the central architecture document for TotalControl: the runtime
layers, the device model, the control-surface boundary and the driver path.
The other specifications extend it with device-specific or user-facing
detail.

## Governing principles

Three principles govern every design decision, in strict priority order. A
lower-priority goal never overrides a higher one. They apply to the whole
project — driver, host, model and UI — not only to the audio path.

1. **Correctness (P1).** Every corner case behaves correctly: all rate bands
   and cross-family transitions, every device personality, startup, stop,
   recovery, and the diagnostics. Wrong output data or a wrong counter is a
   correctness defect regardless of load. TotalControl is a monitor
   controller and must earn the same trust as a physical switch: a muted
   destination is silent, a routed signal arrives exactly, and a counter
   means what it says. Correctness is never traded for latency or
   convenience. *Example:* the inactive monitor output carries its
   monitor-mute in every output-mode write, so applying a scene never
   transiently unmutes it.

2. **Stability (P2).** Robust under load and over time: no underruns or
   recoveries in normal use, honest diagnostics, clean recovery. The audio
   path must hold up under real system load — CPU pressure, disk IO, deep
   idle states — not only on a quiet bench. Where a stability margin costs
   latency, the trade-off is made explicitly and documented rather than
   hidden. *Example:* IRQ scheduling locks the ring fill deterministically,
   so reliability at the smallest fill needs no extra latency margin.

3. **Performance (P3).** Latency, throughput, CPU use and cosmetic cleanup.
   Pursued only once correctness and stability are secured. Low latency is a
   goal, but never at the cost of a wrong sample or a dropout. *Example:* the
   startup prefill gate decouples startup from steady state, so the steady
   fill can be minimal without startup gaps.

When the goals conflict, the higher priority wins. A performance change that
risks correctness is rejected. A stability margin that adds latency is
accepted and stated. The remaining latency is then minimised within the
correctness and stability constraints. Evidence decides: a claim is verified
by capture or test before it becomes a design rule, and a measurement that
contradicts a rule revises the rule.

## Contents

1. [Document map](#1-document-map)
2. [Layers](#2-layers)
3. [Device model](#3-device-model)
4. [Device backends](#4-device-backends)
5. [Device service and mirror](#5-device-service-and-mirror)
6. [Hardware to display](#6-hardware-to-display)
7. [Display to hardware](#7-display-to-hardware)
8. [Control surfaces](#8-control-surfaces)
9. [State, scenes and persistence](#9-state-scenes-and-persistence)
10. [Driver and latency architecture](#10-driver-and-latency-architecture)
11. [Development tools](#11-development-tools)
12. [Extension points](#12-extension-points)
13. [Code map](#13-code-map)

## 1. Document map

| Document | Owns |
|---|---|
| `TOTALCONTROL-ARCHITECTURE.md` | runtime structure, threads, data flow, state ownership, control-surface boundary and host latency model |
| `spec/DRIVER-DESIGN.md` | kernel transport contract, URB engines, hwdep ABI, quirks and driver extension |
| `spec/TOTALCONTROL-GUI-SPEC.md` | layout, controls, panels, gestures, motion and visible state |
| `spec/TOTALCONTROL-STATE-SPEC.md` | state ownership, current workspace, autosave, restore, host settings and layout files |
| `spec/TOTALCONTROL-DEVICE-PROFILE-SPEC.md` | profile table, selection, host operations, capabilities and scene compatibility |
| `spec/FIREFFACE-UCX-PROTOCOL-SPEC.md` | Fireface UCX transport, registers, writes and readback |
| `spec/FIREFFACE-UC-PROTOCOL-SPEC.md` | Fireface UC transport, registers, writes and readback |
| `spec/BABYFACE-CLASSIC-PROTOCOL-SPEC.md` | Babyface-classic transport, channel model, mixer writes, preamps, metering and hwdep ABI |
| `spec/TOTALCONTROL-OSC-SPEC.md` | OSC 1.96 and Global OSC address spaces, scales and feedback |
| `TOTALCONTROL-Manual.md` | installation, setup, use and troubleshooting |
| `DEVELOPMENT-TOOLS.md` | development-only backends, probes and capture helpers |

The architecture document does not contain protocol encodings, pixel geometry
or open questions. Unresolved work belongs in `work/`.

## 2. Layers

```text
 control surfaces                    device backends
 GUI, keyboard, tray, OSC, ...       hwdep, ALSA, vendor USB
        |                                    ^
        v                                    |
   Message + update()  ->  mixer model  ->  RmeDevice
                              |
                              v
                        view / canvases
```

| Layer | Responsibility | Main code |
|---|---|---|
| Mixer model | channels, routing, settings, scenes | `totalcontrol-core` |
| Device backend | read/write the model over one transport | `totalcontrol-core`, `DeviceHandle` |
| Device service | own the real handle on its own thread | `totalcontrol-gui/src/device_service.rs` |
| Application state | selection, view, undo, prefs, host scene | `totalcontrol-gui/src/app.rs` |
| Control adapters | translate external input into `Message` | `app.rs`, `osc.rs`, `osc_rme.rs` |
| View | build the iced element tree from state | `app.rs`, `widgets/` |

The mixer model is the centre. Control surfaces do not talk to the device,
and device backends do not know about control surfaces.

### GUI rendering

The GUI is iced 0.14 on wgpu, which selects a Vulkan adapter on Linux. One
thread runs the event loop: every `Message` rebuilds the view, lays it out and
records the frame; a build takes about a millisecond.

Rendering is full-window. The wgpu backend keeps no damage region, so each
redraw presents the whole surface and the compositor recomposites the window.
The cost of a frame therefore follows the window area, not the part that
changed, and the window is another client on the same GPU as the compositor.

Canvas metering drives its own frames from the shared `MeterSnapshot`: a
meter-only frame every 67 ms (15 fps) while a live level moves, a 10 Hz poll
while a live level is steady, no frame while the bus is idle, and the 33 ms
interaction tick while a control is dragged or an animation runs.  The `Tick` subscription (250 ms readout,
67 ms metered, 33 ms during drags, 4 Hz hidden, 10 Hz at rest) rebuilds the
view.  Ballistics and animations are time-based, so their speed does not
depend on the tick.

## 2.1 Meter data path

The meter feed has two consumer modes.

- **Service mode (default).** The device handle moves onto the service
  thread.  The service is the only reader: it drains every heartbeat at
  device rate, keeps the ballistics, peak holds and OVR latches, and
  publishes a `MeterSnapshot` under a mutex.  The GUI, OSC feedback and
  any other client read that snapshot.  The GUI's tick never paces the
  device.
- **Direct mode (`TOTALCONTROL_DEVICE_SERVICE=0`).** The GUI owns the
  device handle and is the only reader.  It drains all pending
  heartbeats on each tick and folds them into the meter state before
  drawing the newest.  This mode is a diagnostic and test path.

Per heartbeat the data moves as: USB DMA into a coherent URB page
buffer, copy into the driver's 648-byte pair buffer, `kfifo_in` of the
event record under the event lock, reader `read()` (kfifo-out copy and
copy-to-user), and a `to_vec` copy in the service.  The snapshot into
the GUI is one mutex-protected struct copy.  There is no zero-copy
path.

The event FIFO is 64 KiB, which holds about 98 heartbeats.  The driver
pauses at sixteen heartbeats (158 ms) and resumes below four, always at
a pair boundary.  The service keeps occupancy near one pair, so the
marks are a stall guard, not an operating point.  In direct mode the
15 Hz consumer sees about seven heartbeats per frame, which the marks
cover with jitter margin.

Rates measured on the desk: the UC publishes about 187-195 pages per
second, so about 93-97 heartbeats per second after pairing.  The UCX
packet is self-describing and is requested every 4 ms, up to about 250
packets per second; its actual rate is unmeasured.

Display latency: pairing adds at most one page interval, about 5 ms.
The 15 Hz frame adds 0-67 ms of sample age, and rendering adds at most
one frame.  The FIFO adds nothing, because the consumer drains it and
draws the newest state.

Antialiasing is off (`Settings::antialiasing`): iced maps it to MSAAx4, which
renders and resolves every canvas layer and multiplies the per-frame GPU cost.
Canvas shapes are tessellated either way; text comes from the glyph atlas.

## 3. Device model

`totalcontrol-core` defines the model and the `RmeDevice` trait. The trait
covers matrix volumes and pans, mute/solo, preamps, EQ, dynamics, FX, clock,
scenes, event polling and meters.

The serialisable model is a `Scene`:

- `inputs`: `InputChannel`
- `playbacks`: `PlaybackChannel`
- `outputs`: `OutputChannel`
- `settings`: `DeviceSettings`
- `matrix_overrides`: per-cell values the pair model cannot express

The device model owns routing, faders, preamps, EQ/dynamics, FX, meters, clock
options and scenes. The backend reports capabilities and the GUI exposes only
the controls that connection can drive.

## 4. Device backends

`DeviceHandle` is the runtime union of the available backends:

| Variant | Transport | Support |
|---|---|---|
| `Fireface` | Fireface UC/UCX over the kernel hwdep node | production |
| `BabyfaceClassic` | Babyface-classic over the kernel hwdep node | experimental |
| `Mirror` | UI-side copy owned by the device service | production |
| `Mock` | simulation | development and tests |

Every variant implements `RmeDevice`. The production installation is the
kernel driver plus hwdep path. Supported devices are those exposed through
that path: Fireface UC and Fireface UCX, with experimental Babyface-classic
support.

## 5. Device service and mirror

The block diagram in
[`spec/driver-host-architecture.svg`](spec/driver-host-architecture.svg)
maps the device interfaces (D), the driver blocks (K), the userspace
interfaces (U) and the host blocks (H); the driver design §1 defines
them. This section owns the host blocks H3–H5 and the channels C1–C4.

In the running application the device normally lives on the
`totalcontrol-device-service` thread (H3). The UI holds a `Mirror`
instead of the real handle (H5).

**Goal of H3.** The kernel meter pump (K6) is reader-triggered and
backpressured: it advances only while H3 drains the event FIFO (K8).
H3 moves device information into userspace promptly and continuously,
without blocking the kernel side, and keeps all device I/O off the UI
thread. H3 applies commands as model updates plus queued I/O; the
control-I/O worker (H4) executes every F2 operation in submission order
and owns the pacing, so the drain never waits on control writes. One
worker serves the Fireface and Babyface-classic backends; each supplies
its I/O surface, the orchestration is shared. The design rationale is
in [`spec/DRIVER-DESIGN.md`](spec/DRIVER-DESIGN.md) §15.

The service loop:

1. applies commands from C1 as model updates plus queued I/O (H4);
2. calls `device.poll_events()` (F3);
3. reads input, playback and output meters;
4. captures the model every 50 ms and publishes a `Snapshot` when it
   changed (C2);
5. publishes meters separately through a shared `MeterSnapshot` (C3).

### 5.1 Host channels

| Channel | Direction | Payload | Cadence | Semantics |
|---|---|---|---|---|
| C1 Command queue | H5 → H3 | `SetVolume`, `SetMute`, `ApplyScene`, `Run(closure)` | on user action | bounded (256), coalescing latest-value commands; senders block when full; Stop preempts |
| C2 Snapshot | H3 → H5 | the device model: scene, settings, connection, hardware volume, panel selection | 20 Hz, on change | the mirror updates on arrival; confirm-on-snapshot, not optimistic |
| C3 MeterSnapshot | H3 → H5 canvases | latest per-channel levels, peak-hold, OVR flags, transport counters, ballistics | every loop pass (~120 Hz) | a shared latest-value bus, not a queue; canvases read it directly, no view rebuild |
| C4 wake edges | H3 → H5 | notification only | on meter activity edges | starts canvas animation after silence; carries no data |

**Meter terminology chain.** The *meter heartbeat* is the device's page
pair on D5 (UC: RMS then peak; UCX/Babyface: one self-describing
packet). K6 turns it into a METER event in K8. H3 decodes the event
into per-channel levels and publishes the latest values on C3. C3
holds the current state with peak-hold; it is not a queue of
heartbeats.

The UI sends commands such as `SetVolume`, `SetMute`, `ApplyScene`, or a
`Run(closure)` for controls without a dedicated command. Setters on the mirror
queue the command and return. The local model changes when the confirming
snapshot arrives, not optimistically. A setter whose value a follow-up
action reads before the next snapshot updates the local model at once: the
rate setter (the rate-change reapply captures the scene immediately after
it) and the output-link setter (the next balance or pan move reads the link
state).

`TOTALCONTROL_DEVICE_SERVICE=0` keeps the device on the UI thread. Unit tests
use the direct path unless they attach the service explicitly.

Meters are not part of the model snapshot. The fader canvases read the shared
meter bus directly. `MeterWake` is only an edge that starts canvas animation
after silence; it does not rebuild the view for each meter packet.

## 6. Hardware to display

```text
front panel / device state
  -> kernel hwdep event queue or ALSA control / USB status poll
  -> RmeDevice::poll_events()
  -> model fields
  -> Snapshot (device service)
  -> Message::DeviceUpdated
  -> Mirror::update()
  -> view()
```

Backend-specific behaviour:

- **Fireface UCX over hwdep**: `Fireface::poll_events()` drains `EV_STATUS`,
  `EV_STATE` and meter events. The UCX heartbeat keeps the state stream
  flowing. Front-panel gain, output volume, DIM and Mono updates reach
  `apply_ucx_state_word()` and the corresponding model fields.
- **Legacy ALSA backends**: `poll_events()` refreshes output volume and preamp
  controls at a 100 ms interval.
- **Babyface USB**: `panel_tick()` reads the `0x17` panel status and runs the
  host-side panel state machine.
- **Meters**: the service copies device meter pages into `MeterSnapshot` at the
  polling rate. The UC and UCX both use their `0x85` hardware Peak/RMS pages;
  the UC page layout is defined in `docs/spec/FIREFFACE-UC-PROTOCOL-SPEC.md`
  §10. Peak hold lives in the snapshot, so a transient shorter than a canvas
  frame is still visible.

The view reads the model through `Mirror` getters, which never lock or block on
the device thread.

## 7. Display to hardware

```text
widget event
  -> Message (for example VolumeChanged, Mute, EqBandGain)
  -> update()
  -> model mutation helpers
  -> state.device.set_*()
  -> RmeDevice backend
  -> registers, vendor requests or ALSA controls
```

Examples:

- **Fireface UCX over hwdep**: `Fireface::set_volume()` computes the matrix
  coefficient or fader record and queues IF1 register words. Live writes and
  scene applies use the paced batch writer; the UI thread does not sleep
  through the pacing.
- **Fireface UC**: the same model operation becomes an EP0 vendor request.
- **ALSA backends**: the operation becomes an ALSA control write.
- **USB backend**: the operation becomes a proprietary vendor request.

All control adapters enter through `Message`, so they share the same clamps,
undo handling, host-scene merge and autosave. The update loop captures the
scene before an undoable change and merges the confirmed device state after the
command, either immediately on the direct path or after the next snapshot with
the service.

## 8. Control surfaces

A control surface is an adapter at the `Message` boundary.

### GUI, keyboard and tray

Widgets and global subscriptions emit `Message` values. They are the reference
path for the other adapters.

### OSC

OSC is optional. Without `--osc`, no worker or feedback sender is installed.

Incoming:

```text
UDP -> osc::worker -> osc_rme::parse()
    -> RmeState (bank, selected channel, submix)
    -> Vec<Message>
    -> update()
```

Outgoing:

```text
notify_osc() / notify_osc_raw()
    -> OscOutbound
    -> worker
    -> RmeFeedback::translate()
    -> RME OSC paths
    -> UDP
```

`RmeFeedback` keeps a mirror of the last known values and the page state. It
resends the current page after `/setBankStart`, `/setOffsetInBank` or
`/setSubmix`, and suppresses sends outside the visible bank or selected submix.

OSC uses the RME dialect because its bank/page/submix model maps onto
the mixer's channel rows and send matrix. The address space is defined in
`spec/TOTALCONTROL-OSC-SPEC.md`.

## 9. State, scenes and persistence

The device model is the runtime state. It is captured as a `Scene` — the
internal name for one mixer state — for:

- the current workspace;
- snapshot slots;
- undo and redo;
- host-scene merge after a control change;
- scene import and export.

The GUI state model is defined in
[`spec/TOTALCONTROL-STATE-SPEC.md`](spec/TOTALCONTROL-STATE-SPEC.md). The
current workspace is the live state for one unit and the single source of
workspace data. It is stored as a `.tcws` file and autosaved while dirty; an
explicit workspace is a saved copy of the same schema. Host settings hold
installation and per-unit operational values, while the GUI layout document
holds the presentation definitions.

A workspace includes the current scene, the eight snapshots and their names,
channel labels, the four groups, the six layout slots, the mute/solo master
sets and the temporary fader group, view and window state, Control Room state,
virtual-surround settings and the sample rate, pitch/varispeed and active Dim
and Speaker B states. The unit's six device setups and the device serial
number remain device state.

Factory surround Workspaces are `Workspace` values built in memory and
applied through the same `apply_workspace` scene path as a `.tcws` file.  They
start from a clean mixer (no sends, FX returns, Reverb/Echo, EQ, dynamics,
mutes, solos, labels, groups, layouts or snapshots) while retaining the
device topology and device-owned clock, rate, references and preamps.  They
have no file path, so the next Save opens Save As.  The layout code only
writes the mixer scene, labels and group; it does not add a second
persistence format or a device setup.

The direct 5.1/7.1 layouts use pair sends.  The 5.1/7.1 → 4.0 layouts also
use mono matrix overrides for Center and LFE into both front channels.  The
UC and UCX both apply these cells: the UCX writes `0x4000 +
dsp_output*0x40 + source`, while the UC uses its existing matrix path.  A
selected split output channel is stored with its pair and side, so the UI can
address and display the mono bus rather than both sides of the pair.

Active Talkback, Listenback, External Input, PFL and cue selections are
workspace state.  A snapshot stores the mixer scene; the workspace also
stores the active monitoring controls.

Startup sync compares the saved scene with the device where the backend can
read state back.  Groups without readback are written from the saved scene.

For the UCX the window is built from the saved scene model without writing the
device.  The pre-connect dump is requested asynchronously, collected while the
window opens, and decoded before any scene write.  This keeps dump words and
scene-write echoes from mixing on the same state stream.  If the decoded state
matches, the scene is written and startup continues; otherwise the mismatch
dialog is shown.  The direct device handle stays in the UI until that
comparison is resolved, then moves to the device service.

A connect restore leaves output registers that match the pre-connect dump
unchanged.  A reapply after a runtime rate change or stream restart writes the
full scene instead: the unit may have reloaded its own state, so the startup
dump no longer describes the registers, and skipping matching values can leave
effects or routing from the old rate active.  The reapply scene carries the
confirmed rate: the rate the card reopened at, in every rate mode.

The startup restore follows the same rule once the card stream has started.
The connect write happens at the card's previous rate; opening the stream
makes the unit reload its stored mixer, so `restore_engine` queues a
full-scene rewrite for one second later.  A launch therefore writes the scene
twice: once at connect and once after the reload.

## 10. Driver and latency architecture

The production transport is the kernel module `snd-fireface-uc`. It owns USB
enumeration, alternate settings, the isochronous and interrupt URB engines,
the ALSA PCM device and one hwdep control node per card. It covers the Fireface
UC/UCX and the Babyface-classic native transport. TotalControl owns the device
model and writes through hwdep; PipeWire and the DAW use the ALSA PCM.

Some devices already have working audio in the mainline kernel driver, in
their vendor mode. For those, TotalControl needs no audio driver of its own:
the mainline driver keeps the audio, and a small second kernel driver adds
only the control and meters through one hwdep node. TotalControl uses the two
drivers together and shows them as one device. The Digiface USB is the first
such device (in development).

The audio path has four parts:

```text
host ring -> USB URB queue -> device lead -> converters
```

The host ring is the ALSA buffer managed by PipeWire or the application. The
USB queue is the driver's feedback delay and queued URBs. The device lead is a
fixed vendor-path delay. The converters add a fixed small delay.

### Host audio path

The host audio path is who owns the device's PCM and clock. Three paths are
detected from kernel facts, never configured:

| Path | PCM owner | Rate authority |
|---|---|---|
| PipeWire | `pipewire` | TotalControl (Device) or the graph (PipeWire) |
| JACK | `jackd` / `jackdbus` | the JACK server / DAW |
| direct ALSA | a DAW opening `hw:` | the DAW |

The PCM owner is read from `/proc/asound/<card>/pcm0p/sub0/status` (`owner_pid`
resolved through `/proc/<pid>/comm`); the open stream's rate, period and buffer
come from `hw_params` and the live ring delay from `status`. With nobody holding
the PCM the path is idle. Detection follows the PCM owner, not a server's
presence: a DAW running on PipeWire's JACK emulation is a PipeWire client.

The `audio_path` module (core) holds the fact snapshot, the owner→path
classification and the clock-authority decisions as pure functions over that
snapshot. The `audio_path_service` (GUI) is the effectful shell: a kernel-fact
collector reads procfs on a fast cadence, never blocked by a server call, and a
PipeWire worker executes the engine intents and the graph reads, each bounded by
a timeout. Both publish one latest-value engine snapshot; the GUI reads it on
its own cadence and issues intents only. No sound-server call runs on the UI
thread, and the kernel facts stay live when PipeWire stalls. Under an external
owner TotalControl reads the rate and writes nothing to the clock; the mixer
scene is reapplied from the engine-run epoch (`run_seq`), server-free, on every
path.

The host clock source has one owner. The engine poll reads the graph clock
metadata (`clock.rate`, `clock.quantum`, the forced values and the
`min-quantum`/`max-quantum` policy bounds) and, while the device panel shows
the quantum or a change is being applied, the graph quantum the server
actually runs (`pw-top`). The graph quantum is the only truthful confirmation
of a quantum change: the ALSA period is a different quantity (the batch rule
and the driver floor let several quanta share one period). TotalControl is the
authority for the quantum and the rate: the user's value is forced, a refused
quantum request is forced once more, and if the host still runs another value
the actual value is adopted with a notice. The quantum picker offers the
driver floor intersected with the host policy bounds, so an entry it shows can
be honoured.

A forced rate or quantum applies even when the graph is idle. A suspended node
does not act on the force metadata, because the graph re-prepares only while a
stream runs. TotalControl applies the force by activating the graph with a brief
capture stream. The capture is held open until the device confirms the forced
rate and quantum, then closed; the apply is a closed loop of force, activate,
confirm and retry. The capture reads the inputs and injects nothing into the
playback path. PipeWire owns the PCM and multiplexes the capture, so a DAW keeps
its stream.

The GUI reports a round-trip latency, not a measured acoustic round trip. It
has two forms, and they are never added together:

- **Live:** `RTL = snd_pcm_delay + device lead + converter delay` (plus the
  mirrored capture chain for a round trip). `snd_pcm_delay` is the ring
  occupancy (`appl_ptr − hw_ptr`) plus the in-flight URB queue
  (`runtime->delay`); the graph cycle is already contained in the queued
  occupancy, so no separate quantum term is added.
- **Model estimate:** the server's queued target (about one quantum plus the
  headroom; two periods under timer scheduling) plus the in-flight queue, the
  device lead and the converter delay. This is the planning value; it must
  not be combined with a live `snd_pcm_delay`.

Every buffer is counted once. The host ring and the in-flight queue are
distinct terms.

The UC PC and AP device sides are loopback-measured at 32, 44.1, 48, 96 and
192 kHz; the display applies the live queue delta. The UCX and the
uncalibrated UC rates use the model with the same terms. The engine poll reads
the PipeWire inputs off the UI task and refreshes the module queue from sysfs.
The calibration is per personality; a firmware or device-path change needs a
repeat measurement. The measured device values are tabulated in
[`spec/TOTALCONTROL-GUI-SPEC.md`](spec/TOTALCONTROL-GUI-SPEC.md) §13. The
quantum/period/URB geometry is defined in
[`spec/DRIVER-DESIGN.md`](spec/DRIVER-DESIGN.md) §4.1.2.

The vendor path and the class-compliant path are mutually exclusive device
personalities. In class-compliant mode the stock `snd-usb-audio` driver drives
audio and the vendor mixer is unavailable.

### Stream position and period signalling

The driver's PCM position and period-signal contract — whole-frame advance,
signal ownership, the duplex wake gate, pointer coherence, the ISO implicit
feedback fields and the Windows-session timing — is defined once in
[`spec/DRIVER-DESIGN.md`](spec/DRIVER-DESIGN.md) §4 and §7. The wire sample
format and per-device init sequences are in the device protocol specs.

The host-side timing limits are separate from the driver contract. A graph
quantum equal to one period leaves no slack between a direction's period and
the follower's deadline; the daemon substitutes silence when its follower
cycle misses by about one period. A quantum of 96 frames or more at 48 kHz
gives the follower one period of slack, and the default quantum of 256 is
outside the window. This is a host tuning property, not a driver property.

In the Pro Audio profile the capture node is the graph driver and the
playback node is its follower (`priority.driver`; verified in the journal).
Nodes of one device share a clock and are not resampled against each other.
With IRQ scheduling (`api.alsa.disable-tsched`) the SPA plugin links the
playback follower PCM to the capture driver PCM (`api.alsa.auto-link`).
The driver's XRUN recovery re-prepares the driver and then each linked
follower; an unlinked follower is skipped and must recover on its own.

The host scheduling policy has three paths (DRIVER-DESIGN §4.1). The
**Device** pro path uses IRQ scheduling and a period-aligned cushion
`H = 2·period`, set before each rate/quantum change; a non-aligned IRQ wake
threshold collapses the ring fill at fractional rates. The **PipeWire**
convenience path (the default) uses timer scheduling and a fixed generous
margin the DLL servos — safe at every rate, no per-rate tracking. **JACK**
opens the PCM directly with a just-in-time fill. The refill wake cushion
(`api.alsa.headroom`) requires `buffer > period + headroom`; below that
limit the SPA wake thresholds degenerate and the graph cycles at buffer
granularity, so the period count keeps the inequality at every quantum.

The both-pending wake adds a deferral latency equal to the phase offset
between the two directions' period crossings. The offset is a property of
the implicit-feedback data path: the playback ring position leads capture
by the feedback lead, so the two directions' period boundaries fall at
different packets within the URB. It is not a scheduling offset — the host
controller schedules both directions' first URB at the same microframe
(measured: start frames equal at 1x and 4x). The offset is roughly 875 us,
stable and rate-independent.

A quantum equal to the 1 ms USB period (48 frames at 48 kHz, 96 at 96 kHz,
192 at 192 kHz) is the zero-slack corner: the follower's margin is the whole
quantum minus the wake offset, and a missed cycle substitutes silence. The
corner is inherent to the geometry, not a driver defect. The offset cannot
be scheduled away, and waking the graph earlier would expose the follower
to the other direction's stale pointer — the false-availability failure the
gate prevents.

The corner's severity scales with the rate (desk-verified, UCX, IRQ
scheduling, H = 2·period, controlled supply):

| Quantum = floor | Nominal playback | +5 % varispeed |
|---|---|---|
| 48 kHz / 48 | clean | ~1 underrun per 6 s (controlled sine) |
| 96 kHz / 96 | clean | ~1 underrun per 6 s (controlled sine) |
| 192 kHz / 192 | clean | ring collapse (hundreds of underruns) |

The severity depends on the supply path: program material at 48 kHz / 48
with positive pitch silenced playback entirely (every follower cycle
missed), while the controlled sine at the same point shows the milder
bound above. One period of slack removes the penalty in every case: at
96 kHz use 192 frames or more, at 192 kHz 384 frames or more. The floor
stays selectable — the driver accepts it and nominal playback is clean —
and the manual carries the varispeed guidance. The JACK and direct-ALSA
paths do not use the duplex gate; their just-in-time fill with the
application's write-ahead is the low-latency path at floor quanta.


## 11. Development tools

Everything except the kernel driver plus hwdep is a development tool:
the Babyface ALSA backend, the direct libusb backend, the ALSA PCM plugin,
probe programs, capture decoders and mock modes.

They are not user-facing installation options. `DEVELOPMENT-TOOLS.md`
describes what each tool does and how to run it.

## 12. Extension points

### Another OSC dialect

Add a module that parses its addresses into `Message`, keeps its own navigation
state, translates `OscOutbound` into its feedback paths, and runs as a
subscription on its own port.

### MIDI SysEx, MCU or another transport

The inbound side is generic: any transport that produces `Message` values can
drive the mixer. The MIDI controller engine is implemented this way: a
rawmidi worker (`midi.rs`) forwards byte blocks, the map-driven engine
(`controllers/`) decodes them against a definition file into ordinary
messages and encodes feedback, and the application dispatches and refreshes
the instance's window. Mackie Control is the first shipped definition; a
further controller adds a definition file, not code. A further transport
adds a worker and reuses the engine.

The feedback side still carries the names of its first user: `OscOutbound`
contains internal address strings and the application stores one `osc_tx`.
Each controller instance keeps its own feedback queue and last-sent state
beside it.

### Another device backend

A new device model or transport implements `RmeDevice` and adds a
`DeviceHandle` variant. Control surfaces do not change.

## 13. Code map

| Area | Location |
|---|---|
| Model and `RmeDevice` | `ui/totalcontrol/totalcontrol-core/src/device.rs` |
| Scene and persistence | `totalcontrol-core/src/scene.rs` |
| Fireface UC/UCX hwdep backend | `totalcontrol-core/src/fireface/` |
| USB resource layer | `kernel/` |
| Device service | `totalcontrol-gui/src/device_service.rs` |
| Application state and update loop | `totalcontrol-gui/src/app.rs` |
| OSC bridge | `totalcontrol-gui/src/osc.rs`, `osc_rme.rs` |
| MIDI controller engine | `totalcontrol-gui/src/controllers/`, `midi.rs`; definitions in `data/midi-controllers/` |
| GUI widgets | `totalcontrol-gui/src/widgets/` |
| Developer tools | `docs/DEVELOPMENT-TOOLS.md` |
