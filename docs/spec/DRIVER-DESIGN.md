# Fireface USB driver design

This document describes the `snd-fireface-uc` kernel module: what it owns,
how it streams audio, how it talks to the host, and how to extend it to
another RME device. It is the low-level driver reference. Device protocol
encodings live in the protocol specifications. Mixer panels, scenes and GUI
behaviour belong to the host specifications.

Terminology:

| Term | Meaning |
|---|---|
| host | Any userspace client of the driver's PCM/hwdep interface: TotalControl, a command-line tool, or the vendor reference host on Windows or macOS. |
| reference host | The vendor's host driver, used as protocol evidence. |
| transport | The USB streaming path: interrupt or isochronous. |
| USB interface | A numbered `bInterfaceNumber` in the active configuration (IF0 audio, IF1 state/meter, IF2 MIDI, IF3 firmware programming, IF4 page stream, IF5 status). The protocol specifications own the numbering. |
| userspace interface | An ALSA node the host opens: PCM, hwdep or rawmidi. |
| personality | A device firmware/USB variant: UC PC, UC AP, UCX IF0, UCX IF5, Babyface-classic. |
| engine | The running URB set for one card. |
| quirk | A descriptor flag that selects a device-specific transport behaviour. |

Terms follow the USB core and ALSA conventions. Driver-specific terms are
defined above.

## 1. Boundary

The kernel module is a transport driver. The host owns the device model and
policy.

| Layer | Owns |
|---|---|
| `kernel/ffuc.c` | USB probe, descriptor table, alt/rate selection, URB engines, ALSA PCM, status/latch watchdog, recovery, suspend/resume |
| `kernel/ffuc-hwdep.c` | raw EP0 vendor requests, IF1 word bursts, meter/state readers, event queue, backpressure |
| `kernel/ffuc-midi.c` | USB-MIDI rawmidi ports |
| host | device model, protocol semantics, routing, scenes, meter decoding |

The kernel exposes these userspace interfaces:

- the ALSA PCM device (`hw:<card>,0`);
- one `snd-hwdep` control node per card (`/dev/snd/hwC<card>D0`);
- rawmidi ports where the device has USB-MIDI.

The host opens the hwdep node and implements the device model. The hwdep
node is the single control owner while the module is bound.

The driver claims the audio, state/meter, MIDI and status USB interfaces.
The firmware programming USB interface (IF3) and the page stream USB
interface (IF4) remain unclaimed; the firmware update tool uses IF3
directly.

![Driver and host architecture](driver-host-architecture.svg)

The diagram's blocks carry stable IDs. The tables below define each
block and each path; the normative text references the IDs.

### 1.1 Blocks

| ID | Block | Responsibility | Source |
|---|---|---|---|
| D1 | audio OUT/IN | the device's streaming endpoints (IF0 ISO or IF5 interrupt) | device |
| D2 | EP0 vendor | the device's control request endpoint | device |
| D3 | IF1 words OUT | register-word sink, bulk `0x0c` | device |
| D4 | IF1 state IN | state-echo source, bulk `0x8d` | device |
| D5 | IF1 meters IN | meter-heartbeat source, bulk `0x85` | device |
| D6 | status IN | status ticker and output-latch bit, `0x8b` | device |
| D7 | IF2 USB-MIDI | MIDI packet endpoints | device |
| D8/D9 | IF3 / IF4 page stream | firmware programming and page stream; unclaimed | device |
| K1 | URB engine | submits and completes audio URBs; ISO implicit feedback (§4.3) or interrupt handshake (§4.4); cadence and continuity guards | `ffuc.c` |
| K2 | ALSA PCM | the coherent PCM buffer and the duplex period gate (§4.2); fed exclusively by K1 | `ffuc.c` |
| K3 | EP0 passthrough | one synchronous vendor request per `VENDOR` ioctl, bounded by the USB timeout | `ffuc-hwdep.c` |
| K4 | IF1 word writer | one `WRITE_WORDS` burst per ioctl, ≤63 words, odd parity added | `ffuc-hwdep.c` |
| K5 | state reader | resubmits the `0x8d` URB; emits STATE events | `ffuc-hwdep.c` |
| K6 | meter pump | reader-triggered `0x85` submission with backpressure (§5.1); UC pair model | `ffuc-hwdep.c` |
| K7 | status + watchdog | the `0x8b` URB; STATUS and LATCH events; the output-latch watchdog (§4.6) | `ffuc.c` |
| K8 | event FIFO | the 64 KiB event queue; `read()`/`poll()` drain it; a full FIFO drops events | `ffuc-hwdep.c` |
| K9 | USB-MIDI engine | bulk MIDI transfers, one stream per transfer | `ffuc-midi.c` |
| K10 | recovery | marks the engine broken and restarts it (§4.6) | `ffuc.c` |
| U1 | ALSA PCM node | `hw:<card>,0` | ALSA |
| U2 | hwdep node | `/dev/snd/hwC<card>D0`; INFO/VENDOR/WRITE_WORDS/COUNTERS ioctls and the event stream | `ffuc-hwdep.c` |
| U3 | rawmidi nodes | one per logical MIDI port | ALSA |
| H1 | PipeWire / DAW | the ALSA PCM client | host |
| H2 | MIDI clients | rawmidi clients | host |
| H3 | device service thread | owns the device handle; applies commands, drains events, publishes snapshots | host |
| H4 | IF1 writer thread | paced word bursts (UCX today) | host |
| H5 | UI thread | the mirror and the canvases | host |

H3–H5 and the host channels C1–C4 are defined in
[`../TOTALCONTROL-ARCHITECTURE.md`](../TOTALCONTROL-ARCHITECTURE.md) §5.

### 1.2 Paths

| Path | Route | Payload | Direction |
|---|---|---|---|
| F1 audio | D1 ↔ K1 ↔ K2 ↔ U1 ↔ H1 | sample frames; period signals pace H1 | bidirectional |
| F2 control | H3/H4 → U2 → K3/K4 → D2/D3 | vendor requests and register words | host to device |
| F3 events | D4/D5/D6 → K5/K6/K7 → K8 → U2 → H3 | STATE, METER, STATUS, LATCH events | device to host |
| F4 MIDI | D7 ↔ K9 ↔ U3 ↔ H2 | USB-MIDI packets | bidirectional |

F1 carries audio only; F2 and F3 share U2 but are independent: ioctls
never wait on the event drain, and the drain never waits on an ioctl
held by another reader.

### 1.3 Invariants

1. **Single control owner.** U2 is the control owner while the module is
   bound; the host opens it exclusively for control (H3).
2. **Prompt drain.** K6 submits the next meter page only while K8 is
   below its high-water mark (backpressure). H3 drains K8 promptly; a
   stalled H3 pauses K6 and, beyond the FIFO capacity, drops events
   (`events_dropped`).
3. **Single meter reader.** One reader drains F3; concurrent readers
   break the UC pair phase (§5.1).
4. **Control ordering.** F2 operations reach the device in submission
   order; a vendor request never overtakes queued words.
5. **Bounded control.** K3 and K4 ioctls are synchronous and bounded;
   F3 reads wait interruptibly.
6. **Recovery is an xrun.** K10 restarts the engine; H1 re-prepares
   (§4.6).

## 2. Device descriptors and quirks

`struct ffuc_device_desc` is the per-device table. USB IDs point at
descriptors through `.driver_info`. Each probe allocates its own
`struct ffuc`; model state is per-card.

A descriptor carries:

- identity: name, card id, vendor/product;
- transport: interface number, interrupt or ISO, audio/status/state/meter/MIDI
  endpoint numbers;
- modes: one `ffuc_mode` per alternate setting (words, packet sizes, max
  frames);
- channel counts, max rate, meter length;
- channel-init ranges (`0x16` slot ranges);
- quirks;
- `ffuc_ops` callbacks: `init`, `set_rate`, `stream_start`, `stream_stop`,
  `hw_mute`.

Quirks:

| Quirk | Meaning |
|---|---|
| `ALT_RESELECT` | select alt 0 then alt N at engine start (clears the UCX IF5 output latch) |
| `STATUS_LATCH` | status byte 3 bit 7 is the output-latch watchdog |
| `IF5_PRE_START` | send `0x10 0x8000` + `0x1d` before audio |
| `IF5_PLAYBACK_MASK` | request 19/20 channel mask; IN URBs are 73728 bytes |
| `WIN_SESSION` | Windows-personality session timing: stop->start settle and close->rate / close->trio gaps |

Model-specific behaviour belongs in a descriptor quirk or an `ffuc_ops`
callback; the shared PCM/URB path stays model-independent.

## 3. Personalities and wire formats

| Personality | Transport | Wire | Notes |
|---|---|---|---|
| UC PC | interrupt IF0 | S32 | status ticker IF5; session words after a live rate change |
| UC AP | ISO IF0 | float32 | IN header + planar frames; OUT per-frame lead word |
| UCX IF0 | ISO IF0 | float32 | default UCX path |
| UCX IF5 | interrupt IF5 | S32 | opt-in `ucx_if5=1`; Windows mode-setup block, alt 1 at 1x |
| Babyface-classic | ISO IF0 | float32 | UCX-family packet format; 12 capture / 14 playback words at 1x |

The wire format is keyed to the transport: interrupt personalities carry
S32 (the Windows reference pairing), ISO personalities carry float32
(the macOS reference pairing). The device accepts either format when the
channel-init value matches. The driver resolves the format at probe
(`ffuc_wire_s32`) and writes the matching `0x16` value, so wire and init
always agree. The kernel converts between the wire format and the ALSA
format.

The transport operation of each personality is defined in §4.3 (AP mode),
§4.4 (PC mode) and the comparison in §4.5.

The UC keeps its rate family across personality switches; a USB reset
restores the 1x multiplier and forces a session replay on PC.

## 4. Streaming engine

### 4.1 URB and buffer geometry

Module parameters: `urb_frames` (default 48), `nurbs` (default 2),
`ucx_if5_frames` (default 1024), `fixed_rate`.

- Interrupt: one URB carries `urb_frames` scaled by the rate band
  (1x/2x/4x). The PCM buffer covers `(nurbs + 2) * urb_frames`, rounded up
  to whole periods.
- ISO: one URB carries up to 8 packets, one per 125 us microframe. The
  feedback delay is `(nurbs + 1) * iso_packets`. The UCX PCM buffer also
  covers the output lead plus two URBs of margin.
- The period floor is one URB. The graph quantum follows at 48 frames and
  above.

The PCM buffer covers the URBs already in flight plus margin. A completion
that overwrites unread capture frames or sends unwritten playback frames is
counted as `in_overruns` / `out_underruns`. The host prevents these through
its buffer/quantum choice.

### 4.2 Period signalling gate

A period signal announces one completed period. A direction is signalled
only when its period index advanced (`delta_zero` counts violations).

In duplex, the wake is withheld until both running directions have completed
a period, so the follower's pointer is current when the reader wakes. A
direction whose wake has been withheld twice signals alone, releasing the
other direction. Single-direction operation signals at its own boundary.

Counters: `signal_deferred`, `signal_overdue`, `max_pending`, `delta_zero`,
`hw_ptr_mismatch`.

The gate adds at most about one URB of latency at any period size. It is a
correctness mechanism.

### 4.3 ISO implicit feedback (AP mode)

![AP implicit feedback loop](ap-implicit-feedback.svg)

The diagram details K1 on path F1; block IDs continue §1.1. The device
captures and plays from one clock, so the captured stream is the
playback clock reference. The loop:

1. **A1.** D1 delivers IN packets; the header carries position, phase
   and frame count.
2. **A2.** K1.1 records every IN packet's position and frame count in
   K1.2 (`fb[]`), indexed by packet number.
3. **A3.** K1.3 reads the entry `fb_delay` packets behind the current
   OUT packet.
4. **A4.** K1.3 places the OUT lead word at the recorded position plus
   `out_lead`, mirrors the frame count, and sends the packet to D1.

This is implicit feedback: the device's captured clock schedules
playback, keeping the two directions aligned.

Parameters and fields:

| Term | Definition | Used by |
|---|---|---|
| IN position | the device's capture buffer position; anchors the feedback timeline | K1.1, K1.3 |
| IN phase | fractional-sample accumulator; predicts frame add/drop at the wrap | K1.1 |
| IN frame count | frames in this packet | K1.3 mirrors it |
| OUT lead word | the buffer position this OUT packet plays at; keeps the device's output latch fed | K1.3 |
| `fb_delay` | how far OUT lags IN, in packets; covers the URBs in flight | K1.3 |
| `out_lead` | margin added to the mirrored position; absorbs scheduling jitter | K1.3 |

Guards:

| Guard | Input | Action |
|---|---|---|
| K1.4 cadence (A5) | mean frames/packet × 8000 against `chip->rate` | diagnostic: a sustained disagreement marks a rate mismatch |
| K1.5 continuity (A8) | URB start-frame sequence | a discontinuity marks the engine broken → K10 |
| K7 latch watchdog (A6, A7) | D6 status byte 3 bit 7 | a latched output while playback runs, after the grace, marks the engine broken → K10 |
| URB error burst | completion status | above `FFUC_MAX_URB_ERRORS` marks the engine broken → K10 (§4.6) |

The macOS reference host reads the IN position/phase to schedule OUT
packets against the device clock and uses the output-latch status to
detect underrun. The driver's ISO path reproduces that model; the
interrupt path (§4.4) reproduces the Windows session model.

### 4.4 Interrupt transport (PC mode)

The interrupt personalities (UC PC, UCX IF5) carry sample words only: an
OUT URB holds `urb_frames` playback frames, an IN URB holds up to
`urb_frames` capture frames.

Regulation is the interrupt endpoint handshake. The device accepts an OUT
URB when its output stage can consume it, and delivers an IN URB when a
capture block is ready. The driver keeps `nurbs` URBs queued per direction.
On completion it refills the OUT URB from the PCM buffer at the stream
position and resubmits immediately. The completion cadence follows the
device clock; the host is paced by the period signals (§4.2). The cadence
diagnostics `in_gaps` / `out_gaps` count completion intervals longer than
three nominal URB durations.

Rate and session handling on the UC:

- The rate write is the family word plus the commit word. The DDS word is
  part of the ISO rate sequence only.
- A live rate change closes the session, writes the rate, and replays the
  session trio on the next open. The `WIN_SESSION` gaps (§7) apply.

Status and watchdog:

- The UC PC status ticker (interrupt, IF5) stays open while the card is
  bound; it carries front-panel events. The UCX IF5 ticker reports
  stream-period ticks and stops with the engine.
- On `STATUS_LATCH` personalities, status byte 3 bit 7 is the output-latch
  watchdog. A latched output while playback runs, after the
  `FFUC_STATUS_GRACE_MS` grace, marks the engine broken (§4.6).

### 4.5 Transport comparison

| Aspect | PC mode (interrupt) | AP mode (ISO) |
|---|---|---|
| Playback clock reference | interrupt handshake: the device accepts OUT URBs at its own rate | implicit feedback: IN position/phase schedule the OUT lead word (§4.3) |
| OUT payload | sample words | lead word + sample words |
| IN payload | sample words | header (position, phase, frame count) + planar frames |
| URB granularity | `urb_frames` per URB, scaled by rate band | up to 8 packets per URB, one per microframe |
| UC rate sequence | family + commit; session close/reopen with `WIN_SESSION` gaps | DDS + family + commit; session words back-to-back |
| UCX rate sequence | family + commit (IF5) | family (IF0) |
| UC status transport | interrupt ticker IF5, always open | bulk IF4 |
| Output-latch watchdog | `STATUS_LATCH` personalities | UC AP carries no latch quirk; UCX IF0 uses the ISO guards |
| Dropout detection | `out_gaps`, output latch, URB errors | cadence guard, ISO continuity, URB errors |

### 4.6 Recovery and watchdog

A broken engine queues `recover_work`, which stops the streams, stops the
engine, and restarts it if any direction is prepared. Recovery restarts
the engine. The host sees an xrun and re-prepares.

| Trigger | Scope |
|---|---|
| URB error burst above `FFUC_MAX_URB_ERRORS`, or a failed resubmit | all personalities |
| ISO URB queue discontinuity (`ffuc_iso_contiguous`) | ISO personalities |
| Output latch while playback runs, after the `FFUC_STATUS_GRACE_MS` grace | `STATUS_LATCH` personalities (UC PC, UCX IF5) |

## 5. Control transports and hwdep ABI

The hwdep ABI is versioned (`FFUC_ABI_VERSION`). The host checks it at open.

U2 carries two transfer modes with opposite characteristics:

| | F2: ioctl | F3: read()/poll() |
|---|---|---|
| Initiative | host commands the device | device produces information; the host consumes it |
| Shape | one command → one USB transaction → result | drain of events already in K8 |
| Timing | synchronous, bounded by the USB timeout | immediate when events are queued |
| Load | bursty, a few per user action | continuous: ~245 meter packets/s plus state and status, ~98% of the read load |
| Continuity | none; writes may be queued and paced, ordering preserved | hard: K6 advances only while H3 drains; K8 holds ~1 s, then drops |

`ioctl` is the Linux device-command syscall: a typed, synchronous
request/response into the driver. `read()` is a stream drain that causes
no device transaction. The host therefore protects F3 continuity by
keeping F2's blocking waits off the drain thread (§5.2).

| ioctl | Function |
|---|---|
| `INFO` | identity, capabilities, endpoint numbers, rate, firmware |
| `VENDOR` | one EP0 vendor request (read or write, up to 64 bytes) |
| `WRITE_WORDS` | up to 63 IF1 register words; the kernel adds odd parity |
| `COUNTERS` | transport health counters |

`read()`/`poll()` deliver events:

| Event | Payload |
|---|---|
| `METER` | raw meter packet; UC heartbeats are paired (RMS then peak) |
| `STATE` | raw IF1 `0x8d` words |
| `STATUS` | raw status endpoint packet |
| `LATCH` | output-latch transition |

The event FIFO is 64 KiB. A full FIFO drops events and increments
`events_dropped`; the host must drain promptly.

### 5.1 Meter backpressure

The UC meter endpoint is reader-triggered. The driver submits the next page
only while the FIFO is below the high-water mark; it pauses at a pair
boundary. A reader that drains below the low-water mark resumes the pump.
The last reader closing stops the pump and drains the partner page to
preserve the device's page phase for the next session.

The host owns the single-reader discipline; concurrent readers break the
pair phase model.

### 5.2 Host obligations

The host:

- drains the event FIFO promptly; a stalled reader drops events;
- keeps one meter reader;
- chooses a buffer/quantum that covers the URB queue plus margin;
- checks engine state before control writes that assume a running engine;
- treats recovery as an xrun and re-prepares;
- uses the hwdep node as the control owner.

The driver keeps the host responsive: control ioctls are synchronous but
bounded; event reads wait interruptibly; the meter pump is backpressured.

## 6. MIDI

USB-MIDI is on the vendor MIDI interface (bulk OUT/IN). The driver exposes
one rawmidi port per logical port. Both directions use USB-MIDI code index
numbers. The unit parses each bulk transfer as a separate MIDI stream; a
message must fit one transfer. Output is polled by a delayed work at 1 ms.

## 7. Rate, session and startup timing

Engine start selects the alternate setting, commits the rate, sends the
session-open words where required, sets up URBs, submits them, and unmutes.

The UC Windows-personality path uses three timed gaps:

| Gap | Value | Reason |
|---|---|---|
| stop -> start settle | 250 ms | minimum gap the device requires after the close word before a start |
| close -> rate write | 200 ms | minimum gap the device requires after the close word before the rate write |
| close -> session trio | 200 ms | minimum gap the device requires after the close word before the session trio |

These are device-mandated timing measured on the Windows reference-host
captures. The AP personality sends the session words back-to-back.

Design principle: prefer a deterministic readiness condition over a timed
guess. Where the device exposes a status that can be polled (rate index,
session state, output latch), the driver polls it with a bounded timeout.
Where the device provides no acknowledgement, the wait is a documented
device quirk with capture evidence, expressed as a named quirk constant.
See the TODO section for the readiness-poll work.

## 8. Suspend, resume and hotplug

- Suspend records the running directions, stops the engine, suspends MIDI
  and the raw transports. ALSA suspend is entered only when every prepared
  direction was running.
- Resume re-runs hardware init and restarts the engine if prepared. A USB
  reset forces alt reselection and a UC session replay.
- Disconnect marks the card disconnected, kills URBs, cancels recovery,
  cleans up hwdep/MIDI, and frees the card when closed.

## 9. Latency inputs

The driver exposes the queue geometry (`urb_frames`, `nurbs`,
`iso_packets`, `fb_delay`, `out_lead`) and the transport counters. The host
computes round-trip latency from these inputs and its loopback calibration;
that model is defined in
[`../TOTALCONTROL-ARCHITECTURE.md`](../TOTALCONTROL-ARCHITECTURE.md) §10.

## 10. Open items

These items narrow the remaining investigation. They are implementation
risks or tuning questions, not shipped behaviour.

Scope labels used in this section and in the TODO:

| Label | Personalities |
|---|---|
| all | every personality |
| ISO | UC AP, UCX IF0, Babyface-classic |
| `WIN_SESSION` | UC PC, UC Win, UCX IF5 |
| UC | UC PC and UC AP |
| status | personalities with a status endpoint (all except Babyface-classic) |
| host | TotalControl, independent of personality |

### Driver risks

Terms used here are defined in the sections referenced.

- **[all]** The **duplex period gate** (§4.2) is the wake rule that signals
  both running directions together. Its **deferral counters** (§4.2) count
  withheld wakes per direction. The gate reads and increments these
  counters without a lock shared across directions. Concurrent URB
  completions on different CPUs could race the counters. The period signal
  itself is under the stream lock; the gate decision is not.
- **[all]** The **event FIFO** (§5) is freed by `ffuc_hwdep_cleanup`, which
  runs in disconnect and again in `private_free`. A reader that passed the
  disconnected check could race the FIFO free. Cleanup should be ordered
  against hwdep unregistration.
- **[ISO]** The **ISO continuity check** (K1.5, `ffuc_iso_contiguous`, §4.3)
  verifies URB start-frame continuity. Its log counter is module-global,
  shared across devices and endpoints. It should be per-endpoint.
- **[all]** The **hwdep read path** (§5) allocates a temporary buffer per
  event. A per-reader buffer would remove the allocation from the event
  path.
- **[all]** The **PCM trigger stop** path (§4) emits verbose diagnostic
  logs on every stop. These belong in debugfs, ratelimited, or behind a
  diagnostic parameter.
- **[all]** **Buffer poison** (`ffuc_poison`) memsets the whole PCM buffer
  on every prepare (§4.1). It is a diagnostic; production should make it
  optional.
- **[status]** The **COUNTERS** ioctl (§5) reports `status_urbs` as zero;
  the status URB counter is unexposed.
- **[Babyface-classic]** The **INFO** ioctl (§5) reports `outputs` as the
  playback channel count. On Babyface-classic the hardware output count
  differs. Verify the ABI field semantics.

### Host interaction

The **service loop** is the host thread that owns the hwdep handle
([`../TOTALCONTROL-ARCHITECTURE.md`](../TOTALCONTROL-ARCHITECTURE.md) §5).
It drains commands and events on one thread. A blocking control burst can
delay event draining and drop events. Separating control writes from the
event drain would keep the meter/state pump responsive under heavy write
load. **[host]**

### Heuristic waits

A **heuristic wait** is a fixed sleep used where a readiness condition or
flow control could decide the next step.

- The session gaps (§7) are device-mandated timing, implemented as fixed
  sleeps. **[`WIN_SESSION`]**
- The **meter pump** (§5.1) resubmits pages from a 4 ms delayed work and
  retries a paused pump after 10 ms. The pair-boundary pause is UC-specific;
  the pump itself serves every personality. **[all]**
- **Burst pacing** is the fixed gap the host inserts between `WRITE_WORDS`
  bursts (§5). It applies to the UCX write path. **[host, UCX]**

Each wait should become a deterministic readiness poll or bounded flow
control where the device exposes a condition, or stay a named quirk with
capture evidence where it does not.

## 11. TODO / future optimizations

Scope labels are defined in §10.

| Item | Scope |
|---|---|
| Replace fixed session gaps with a bounded poll of a device-visible readiness condition where one exists; keep documented quirks where the device has no acknowledgement | `WIN_SESSION` |
| Add a rate-readiness poll after the rate commit word (UC PC, UC AP, UCX IF5 send it; UCX IF0 commits the family alone) | UC, UCX IF5 |
| Serialize the duplex gate decision or make the deferral counters atomic | all |
| Order hwdep cleanup against card unregistration; remove the double cleanup path | all |
| Move ISO continuity logging to per-endpoint state | ISO |
| Preallocate the hwdep read buffer per reader | all |
| Make buffer poison and verbose stop diagnostics optional module parameters | all |
| Resubmit meter/state URBs from completion where backpressure allows, instead of a fixed 4 ms delayed work; preserve the UC pair-boundary pause | all |
| Expose the status URB counter | status |
| Verify the INFO output/playback field semantics | Babyface-classic |
| Run `sparse` (`make C=1`) and lockdep/UBSAN on the module; fix warnings | all |
| Reduce checkpatch block-comment warnings to meet kernel style | all |

Host (TotalControl) driver-communication work:

| Item | Scope |
|---|---|
| Separate control writes from the event drain in the service loop | host |
| Bound the command queue with backpressure | host |
| Replace the UCX burst-pacing sleep with bounded flow control | host, UCX |
| Enforce or document the single meter reader for the UC pair model | host, UC |
- Expose the status URB counter; verify the INFO output/playback fields.
- Run `sparse` (`make C=1`) and lockdep/UBSAN on the module; fix warnings.
- Reduce checkpatch block-comment warnings to meet kernel style.

## 12. Open protocol items

The driver forwards raw packets and exposes the counters; the host owns
the decoding. The items below are the protocol gaps that touch the
driver's transports. The owning sections carry the full detail:
[`FIREFFACE-UC-PROTOCOL-SPEC.md`](FIREFFACE-UC-PROTOCOL-SPEC.md) §11.10
(transport) and §12 (metering),
[`TOTALCONTROL-MIDI-SPEC.md`](TOTALCONTROL-MIDI-SPEC.md) §13 (MIDI).

### Transport

| Item | Scope | Evidence that decides it |
|---|---|---|
| AP descriptor: the AP interface and packet rows are the implemented mapping | UC AP | a descriptor capture of the AP personality |
| AP MIDI: the driver exposes rawmidi where the descriptor carries USB-MIDI | UC AP | the AP descriptor capture; RME documents the ports as class compliant |
| Page stream field meanings on IF4; the driver leaves IF4 unclaimed | UC PC | a capture of the page stream with a known device state |
| Read form of request `0x10`; the implemented path writes it | UC | a reference-host capture that reads it |
| Status bytes 0 and 1; front-panel events use bytes 2 and 3 | UC PC | a capture correlating bytes 0/1 with a device action |
| Bulk role of `0x03`/`0x84` at 1x/2x; they are interrupt at 4x | UC PC | a capture of the bulk traffic at 1x/2x |
| Interrupt URB size: 48/96/192 frames is the host's choice; the device contract is the packet size and the rate class | UC PC | documentation item |
| AP header phase on the UCX: the 12-bit position, 11-bit phase and 3072 wrap are the implemented mapping | UCX IF0 | a UCX capture; on the UC the accumulator is verified at 48 kHz, isolated add/drop events away from the wrap are unattributed |
| IF5 alternate selection at 2x/4x: alt 1 is the verified 1x path; alt 2/3 carry the higher bands in the descriptor | UCX IF5 | a reference-host capture streaming at 96/192 kHz |
| DDS quad rate dependence: the quad is written once at connect; rate changes write the family word alone across the captured 44.1↔48 transitions | UCX IF5 | a connect capture at a 32 kHz-family rate |
| UC PC connect init: the shipped 36-slot init is bench-verified; the UC Windows reference uses a 60-slot block | UC PC | a UC Windows connect capture comparison |
| MIDI port 2 physical output: packets carry both ports | UC | a loopback on the candidate output |

### Metering

The UC pair model (§5.1) is implemented and verified on the interrupt
personality. These items remain open in the metering specification:

| Item | Scope | Evidence that decides it |
|---|---|---|
| Beat words (bytes 4..8): observed formats vary; they carry no page identity and no synchronisation role | UC | a session-level correlation capture |
| RMS-then-peak pair order on AP: alternation is verified there; the order is carried from the interrupt verification | UC AP | a live AP capture with a decidable signal |
| Header status-slot values and RMS slot-internal engine fields: they drift within a session and track no input signal | UC | an inventory capture with controlled inputs |
| Slip cause: hypothesis is a meter-endpoint data-toggle reset on the alternate-setting change; a usbmon capture across 48->96 kHz showed no duplicate page, so the slip is intermittent | UC | a bus-analyser trace at data-toggle level across a slipping engine start; the shape check realigns regardless |
| Shape check with live ADAT 3..7: decidable with live AN5/SPDIF (748/748 heartbeats) | UC | an ADAT source carrying signal on ADAT 3..7 |

## 13. Code map

| Concern | Source |
|---|---|
| descriptor table, quirks, modes | `kernel/ffuc.c` |
| URB engines, period gate, ISO feedback | `kernel/ffuc.c` |
| PCM ops, buffer geometry | `kernel/ffuc.c` |
| status/latch watchdog, recovery | `kernel/ffuc.c` |
| hwdep ABI, event queue, meter pump | `kernel/ffuc-hwdep.c`, `kernel/ffuc-uapi.h` |
| meter heartbeat pairing | `kernel/ffuc-meter-pair.h` |
| USB-MIDI | `kernel/ffuc-midi.c` |
| host ABI binding | `ui/totalcontrol/totalcontrol-core/src/fireface/abi.rs` |
| host protocol/model | `ui/totalcontrol/totalcontrol-core/src/fireface/` |

## 14. Adding support for a device

A new device is a descriptor entry plus, where the streaming sequence
differs, an `ffuc_ops` implementation. The shared PCM/URB path stays
model-independent.

### 14.1 Evidence first

Capture the device before writing code:

- USB descriptors: configurations, interfaces, alternate settings,
  endpoint addresses and packet sizes.
- The streaming wire format: sample word format, packet layout, any header
  or lead word, channel counts per rate band.
- The control transports: EP0 vendor requests, any bulk register/meter
  channel, the status endpoint.
- The init and rate sequences from the reference host.

Document every register/offset in a protocol specification. Mark uncertain
mappings as open items; the driver carries only verified behaviour.

### 14.2 Descriptor

Add an entry to `ffuc_devices[]` and a USB ID to `ffuc_ids[]`:

| Field | Source |
|---|---|
| `vendor`, `product`, `iface` | USB descriptors |
| `transport` | interrupt or ISO |
| `audio_iface`, `out_ep`, `in_ep` | streaming interface |
| `status_iface`, `status_ep`, `status_bulk` | status ticker, if present |
| `state_iface`, `state_out_ep`, `state_in_ep`, `meter_in_ep` | bulk control/meter channel, if present |
| `midi_iface`, `midi_out_ep`, `midi_in_ep`, `midi_ports` | USB-MIDI, if present |
| `modes[]` | one `ffuc_mode` per alternate setting: words, out_words, pkt_frames, in_mps, out_mps, max_frames |
| `max_channels`, `max_playback`, `max_rate`, `meter_len` | channel counts and meter packet size |
| `chan_init[]`, `n_chan_init` | `0x16` slot ranges |
| `quirks` | only behaviour the shared path must select |
| `ops` | `init`, `set_rate`, `stream_start`, `stream_stop`, `hw_mute` |

The wire format is resolved at probe by `ffuc_wire_s32()` from the
transport: interrupt carries S32, ISO carries float32. The matching
`0x16` channel-init value is derived from the same decision.

### 14.3 Ops and quirks

Implement `ffuc_ops` only for the sequences that differ from the existing
models. Reuse an existing ops table where the sequence matches.

Add a quirk only when the shared path must branch on a device property.
Name the quirk after the behaviour, document the capture that establishes
it, and keep the timing constants named.

### 14.4 Host side

Add the host profile and protocol module separately. The driver exposes raw
transports and counters; the host owns the device model, meter decoding and
scenes. Follow `docs/CONTRIBUTING-NEW-DEVICE.md` for the host integration.

### 14.5 Tests

- Kernel: build with `make -C kernel`, run `make -C kernel test`, run
  `checkpatch.pl` and `sparse` (`make C=1`).
- Transport: verify URB geometry, period signalling and recovery on the
  device; record the counters.
- Host: add encoding tests for the new protocol module.
- Hardware: verify each rate band, both directions, suspend/resume and
  hotplug. Preserve the user scene and configuration.
