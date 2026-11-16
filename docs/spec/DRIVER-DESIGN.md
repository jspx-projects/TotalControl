# Fireface USB driver design

This document describes the `snd-fireface-uc` kernel module: what it owns,
how it streams audio, how it talks to the host, and how to extend it to
another RME device. It is the low-level driver reference. Device protocol
encodings live in the protocol specifications; host-app policy lives in the
host-app specifications. This document does not describe mixer panels,
scenes or GUI behaviour.

Terminology:

| Term | Meaning |
|---|---|
| host | Any userspace client of the driver's PCM/hwdep interface: TotalControl, a command-line tool, or the vendor reference host on Windows or macOS. |
| reference host | The vendor's host driver, used as protocol evidence. |
| transport | The USB streaming path: interrupt or isochronous. |
| personality | A device firmware/USB variant: UC PC, UC AP, UCX IF0, UCX IF5, Babyface-classic. |
| engine | The running URB set for one card. |
| quirk | A descriptor flag that selects a device-specific transport behaviour. |

## 1. Boundary

The kernel module is a transport driver, not a mixer.

| Layer | Owns | Does not own |
|---|---|---|
| `kernel/ffuc.c` | USB probe, descriptor table, alt/rate selection, URB engines, ALSA PCM, status/latch watchdog, recovery, suspend/resume | mixer model, matrix, faders, EQ, meters, scenes |
| `kernel/ffuc-hwdep.c` | raw EP0 vendor requests, IF1 word bursts, meter/state readers, event queue, backpressure | parsing register values or meter layouts |
| `kernel/ffuc-midi.c` | USB-MIDI rawmidi ports | MIDI policy |
| host | device model, protocol semantics, routing, scenes, meter decoding | USB/PCM transport |

The kernel creates no ALSA mixer controls. The only interfaces are:

- the ALSA PCM device (`hw:<card>,0`);
- one `snd-hwdep` control node per card (`/dev/snd/hwC<card>D0`);
- rawmidi ports where the device has USB-MIDI.

The host opens the hwdep node and implements the device model. Direct
libusb control while the module is bound is unsupported; the hwdep node is
the single owner.

## 2. Device descriptors and quirks

`struct ffuc_device_desc` is the per-device table. USB IDs point at
descriptors through `.driver_info`. Each probe allocates its own
`struct ffuc`; no model state is global.

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
| `INT32_SAMPLES` | device requires S32 wire format, not float32 |
| `STATUS_LATCH` | status byte 3 bit 7 is the output-latch watchdog |
| `IF5_PRE_START` | send `0x10 0x8000` + `0x1d` before audio |
| `IF5_PLAYBACK_MASK` | request 19/20 channel mask; IN URBs are 73728 bytes |
| `WIN_SESSION` | Windows-personality session timing: stop->start settle and close->rate / close->trio gaps |

Do not add model-specific branches to the shared PCM/URB path unless they
are a descriptor quirk or an `ffuc_ops` callback.

### Adding a device

1. Add a descriptor entry: USB ID, interface, endpoints, modes, channel
   counts, meter length, init ranges.
2. If the streaming sequence differs, add an `ffuc_ops` implementation.
3. Add quirks only for behaviour that the shared path must select.
4. Add the host-app profile and protocol module separately.
5. Document every new register/offset in the protocol specification and
   mark uncertain items as open.
6. Add encoding tests in the host; add a kernel transport test if the
   URB path changes.

## 3. Personalities and wire formats

| Personality | Transport | Wire | Notes |
|---|---|---|---|
| UC PC | interrupt IF0 | float32 (Linux/macOS), S32 (Windows reference host) | status ticker IF5; session words after a live rate change |
| UC AP | ISO IF0 | float32 | IN header + planar frames; OUT per-frame lead word |
| UCX IF0 | ISO IF0 | float32 | default UCX path |
| UCX IF5 | interrupt IF5 | S32 | opt-in `ucx_if5=1` |
| Babyface-classic | ISO IF0 | float32 | UCX-family packet format; 12 capture / 14 playback words at 1x |

The wire format is resolved at probe (`ffuc_wire_s32`). The host imposes the
format through the channel-init value; the kernel converts between the wire
format and the ALSA format.

PC vs AP limitations:

- PC uses interrupt URBs and the Windows session words. A live rate change
  arms `session_reopen`/`uc_session`; later opens/closes replay the session
  trio/close word. The Windows-personality gaps are device-mandated timing
  (see §7).
- AP uses ISO implicit feedback. The IN header carries position and phase;
  the OUT lead word places playback against the captured clock. There is no
  Windows session settle on AP.
- The UC keeps its rate family across personality switches; a USB reset
  restores the 1x multiplier and forces a session replay on PC.

## 4. Streaming engine

### 4.1 URB and ring geometry

Module parameters: `urb_frames` (default 48), `nurbs` (default 2),
`ucx_if5_frames` (default 1024), `fixed_rate`.

- Interrupt: one URB carries `urb_frames` scaled by the rate band
  (1x/2x/4x). The ring covers `(nurbs + 2) * urb_frames`, rounded up to
  whole periods.
- ISO: one URB carries up to 8 packets, one per 125 us microframe. The
  feedback delay is `(nurbs + 1) * iso_packets`. The UCX ring also covers
  the output lead plus two URBs of margin.
- The period floor is one URB. The graph quantum follows at 48 frames and
  above.

The ring must cover the URBs already in flight plus margin; otherwise a
completion overwrites unread capture frames or sends unwritten playback
frames. The driver counts these as `in_overruns` / `out_underruns`; it does
not prevent them. Prevention is the host's buffer/quantum choice.

### 4.2 Period signalling gate

A period signal announces one completed period. The driver never signals a
direction for zero completed periods (`delta_zero` counts violations).

In duplex, the wake is withheld until both running directions have completed
a period, so the follower's pointer is not stale when the reader wakes. A
direction whose wake has been withheld twice signals alone, so a stalled
stream cannot hold the other hostage. Single-direction operation signals at
its own boundary.

Counters: `signal_deferred`, `signal_overdue`, `max_pending`, `delta_zero`,
`hw_ptr_mismatch`.

The gate adds at most about one URB of latency at any period size. It is a
correctness mechanism, not a tuning knob.

### 4.3 ISO implicit feedback and dropout prevention

The ISO IN header carries a position and a phase accumulator. The driver
records every IN packet's position and frame count in a feedback ring
(`fb[]`), indexed by packet number. The OUT path reads the feedback entry
`fb_delay` packets behind the current OUT packet and places the OUT lead
word accordingly. This is implicit feedback: the device's captured clock
schedules playback, so the two directions do not drift.

Fields that assist dropout prevention:

| Field | Role |
|---|---|
| IN position | the device's capture ring position; anchors the feedback timeline |
| IN phase | fractional-sample accumulator; predicts frame add/drop at the wrap |
| IN frame count | frames in this packet; the OUT path mirrors it |
| OUT lead word | the ring position this OUT packet plays at; keeps the device's output latch fed |
| `out_lead` | host-side margin added to the mirrored position; absorbs scheduling jitter |
| `fb_delay` | how far OUT lags IN; covers the URBs in flight |
| cadence | mean frames/packet * 8000; detects a rate write that did not take effect |
| output-latch bit | status byte 3 bit 7; the device signals that its output latch ran dry |

How the vendor likely uses them: the macOS reference host reads the IN position/phase
to schedule OUT packets against the device clock, and uses the output-latch
status to detect underrun. The Windows interrupt path has no IN header; it
relies on the session words and the status ticker. The driver's ISO path
reproduces the macOS implicit-feedback model; the interrupt path reproduces
the Windows session model.

The cadence guard is a diagnostic: a sustained disagreement between the
packet cadence and `chip->rate` means the rate write did not take effect or
the device was reprogrammed externally. It does not itself recover the
stream.

### 4.4 Recovery and watchdog

- URB errors above `FFUC_MAX_URB_ERRORS` mark the engine broken and queue
  `recover_work`.
- ISO URB queue discontinuity (`ffuc_iso_contiguous`) marks the engine
  broken.
- The status latch watchdog arms after `FFUC_STATUS_GRACE_MS`; a latched
  output while playback runs marks the engine broken.
- `recover_work` stops the streams, stops the engine, and restarts it if
  any direction is prepared.

Recovery is a restart, not a sample-accurate repair. The host sees an xrun
and re-prepares.

## 5. Control transports and hwdep ABI

The hwdep ABI is versioned (`FFUC_ABI_VERSION`). The host checks it at open.

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
The last reader closing stops the pump and drains the partner page so the
device's page phase is not shifted for the next session.

Only one meter reader should drain the stream. Concurrent readers break the
pair phase model. The host owns this discipline; the driver does not
enforce a single reader.

### 5.2 Host obligations

The host must:

- drain the event FIFO promptly; a stalled reader drops events;
- keep one meter reader;
- choose a buffer/quantum that covers the URB queue plus margin;
- not issue control writes that assume a running engine without checking
  state;
- treat recovery as an xrun and re-prepare;
- not use direct libusb while the module is bound.

The driver must not block the host: control ioctls are synchronous but
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
| stop -> start settle | 250 ms | a start immediately after the close word leaves the unit silent |
| close -> rate write | 200 ms | the rate write after a close wedges the unit |
| close -> session trio | 200 ms | a back-to-back reopen leaves the unit silent |

These are device-mandated timing measured on the Windows reference-host captures.
They are not arbitrary host tuning. The AP personality does not use them.

Design principle: prefer a deterministic readiness condition over a timed
guess. Where the device exposes a status that can be polled (rate index,
session state, output latch), the driver should poll it with a bounded
timeout instead of sleeping a fixed interval. Where the device provides no
acknowledgement, the wait is a documented device quirk with capture
evidence, not a scattered magic number. See the TODO section for the
readiness-poll work.

## 8. Suspend, resume and hotplug

- Suspend records the running directions, stops the engine, suspends MIDI
  and the raw transports. ALSA suspend is entered only when every prepared
  direction was running.
- Resume re-runs hardware init and restarts the engine if prepared. A USB
  reset forces alt reselection and a UC session replay.
- Disconnect marks the card disconnected, kills URBs, cancels recovery,
  cleans up hwdep/MIDI, and frees the card when closed.

## 9. Latency inputs

The driver does not compute round-trip latency. It exposes the queue
geometry (`urb_frames`, `nurbs`, `iso_packets`, `fb_delay`, `out_lead`) and
the transport counters. The host owns the latency estimate and its
loopback calibration; that model is defined in
[`../TOTALCONTROL-ARCHITECTURE.md`](../TOTALCONTROL-ARCHITECTURE.md) §10.

## 10. Audit findings

### Fixed in this branch

- Meter URB buffer was allocated as `2 * meter_len` but freed as
  `meter_len` in the error path and cleanup. The free size now matches the
  allocation.
- `ucx_if5` static initializer removed (checkpatch error).

### Open defects / risks

- The duplex period gate reads and increments `deferrals` without a lock
  shared across directions. Concurrent URB completions on different CPUs
  could race the gate counters. The signal itself is under the stream lock;
  the gate decision is not.
- `ffuc_hwdep_cleanup` runs in disconnect and again in `private_free`. A
  reader that passed the disconnected check could race the FIFO free.
  Cleanup should be ordered against hwdep unregistration.
- `ffuc_iso_contiguous` uses a module-global `start_logged` counter shared
  across devices/endpoints. It should be per-endpoint.
- The hwdep read path allocates a temporary buffer per event. A per-reader
  buffer would remove the allocation from the event path.
- The PCM trigger stop path emits verbose diagnostic logs on every stop.
  These should be debugfs/ratelimited or behind a diagnostic parameter.
- `ffuc_poison` memsets the whole PCM ring on every prepare. This is a
  diagnostic; it should be optional for production.
- `COUNTERS.status_urbs` is always zero; the status URB counter is not
  exposed.
- `INFO.outputs` uses the playback channel count; on Babyface-classic the
  hardware output count differs. Verify the ABI field semantics.

### Heuristic waits

The timed gaps in §7 are device-mandated but currently fixed sleeps. The
status/meter pump uses 4 ms delayed work and 10 ms retry. The host burst
pacing uses a fixed gap. These should be replaced by deterministic
readiness/flow-control where the device exposes it, or documented as
device-mandated timing where it does not.

## 11. TODO / future optimizations

- Replace fixed session gaps with a bounded poll of a device-visible
  readiness condition where one exists; keep documented quirks where the
  device has no acknowledgement.
- Add a rate-readiness poll after the rate commit instead of assuming the
  write took effect.
- Serialize the duplex gate decision or make the deferral counters atomic.
- Order hwdep cleanup against card unregistration; remove the double
  cleanup path.
- Move ISO continuity logging to per-endpoint state.
- Preallocate the hwdep read buffer per reader.
- Make ring poison and verbose stop diagnostics optional module parameters.
- Resubmit meter/state URBs from completion where backpressure allows,
  instead of a fixed 4 ms delayed work.
- Expose the status URB counter; verify the INFO output/playback fields.
- Run `sparse` (`make C=1`) and lockdep/UBSAN on the module; fix warnings.
- Reduce checkpatch block-comment warnings to meet kernel style.

## 12. Open protocol items

The phase/position fields and meter page phase are documented in the UC
protocol specification. Items that remain unverified there stay open there;
this document does not restate them as driver facts. The driver forwards
raw packets and exposes the counters; the host owns the decoding.

## 13. Code map

| Concern | Source |
|---|---|
| descriptor table, quirks, modes | `kernel/ffuc.c` |
| URB engines, period gate, ISO feedback | `kernel/ffuc.c` |
| PCM ops, ring geometry | `kernel/ffuc.c` |
| status/latch watchdog, recovery | `kernel/ffuc.c` |
| hwdep ABI, event queue, meter pump | `kernel/ffuc-hwdep.c`, `kernel/ffuc-uapi.h` |
| meter heartbeat pairing | `kernel/ffuc-meter-pair.h` |
| USB-MIDI | `kernel/ffuc-midi.c` |
| host ABI binding | `ui/totalcontrol/totalcontrol-core/src/fireface/abi.rs` |
| host protocol/model | `ui/totalcontrol/totalcontrol-core/src/fireface/` |
