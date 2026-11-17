# Fireface UC — open work backlog

A single, shared hand-off document for the Fireface UC (and UCX where noted).
It carries two things: a concise summary of what has been **verified** by
experiment, and the **open work** that remains. Pick an item, implement it,
and update its status here.

This is a working note, not a specification. The authoritative model lives in
`docs/spec/FIREFFACE-UC-PROTOCOL-SPEC.md`; the kernel transport contract and
its own open items live in `docs/spec/DRIVER-DESIGN.md` (§10–12); device
profiles, MIDI, OSC and the GUI have their own specifications. Mirror
established behaviour into the specification when you close an item.

Conventions: one Fireface at a time; preserve the user scene and
`~/.config/totalcontrol/gui.json`; keep captures and raw device state outside
the repository; no personal data, serials, hostnames or USB IDs in commits or
notes. Protocol items need a capture before code: state the implemented
mapping positively in the specification and keep the open question here.

Status legend: **open** (actionable), **needs-capture** (blocked on evidence),
**needs-device** (blocked on a specific unit/firmware), **optional** (not
required for correct behaviour), **done** (recently closed).

## Verified results

Summary of experiment outcomes. The specification is the reference for the
law behind each; these lines say what was measured.

### Initialization and sample format

- The `0x16` channel-init `wValue` is the per-channel sample format:
  `0 = S32`, `1 = float32`. It must equal the wire format. A mismatch passes
  only the sample sign bit and outputs a full-scale square wave (fundamental
  2/π, odd harmonics 2/nπ, THD ≈ −6 dB). All four value/format combinations
  were verified clean-or-distorted exactly by this rule on UC PC firmware 127
  at 44.1 kHz through the internal loopback.
- Slot count is independent of the pairing: 36 slots and 60 slots both play
  clean when the value matches the wire format.
- The wire format is keyed to the transport: interrupt (PC) carries S32,
  the Windows reference pairing; isochronous (AP) carries float32, the
  macOS reference pairing (#191). The format is not readable from the
  device status registers (`0x11`, `0x10`); the host imposes it via `0x16`.
- The AP init is 36 `0x16` writes (slots `0..17`, `32..49`) with `0x0001`,
  then `0x10 0x0800/0xb800`, then `0x20 0x0001/0x0000`. The Windows PC init is
  60 `0x16` writes (slots `0..29`, `32..61`) with `0x0000`, four DDS words and
  the masked settings group, with no `0x20` and no `0x0800/0xb800`.
- The TotalMix connect burst writes exactly the same content the host replays
  (channel init, DDS, settings, loopback, reference, levels, 666 matrix
  words). There is no hidden restore state; the earlier distortion was the
  sample-format pairing.
- The UC PC float pairing (the earlier host-OS rule) was verified clean:
  internal loopback returned a pure sine (fundamental = RMS + 3 dB, zero
  discontinuities), rate transitions 44.1↔48 kHz tracked the device rate
  index, and the transport counters stayed zero. The S32 pairing was
  verified clean on the same unit; the transport rule (#191) ships S32 on
  PC.

### Rate and clock

- DDS word = `0x5f5e10000000 / rate`, shifted left 1 above 57 kHz and 2 above
  114 kHz; `wValue` = bits 31..16, `wIndex` = bits 15..8 | family. Verified
  for all nine standard rates and eight varispeed rates.
- The `0x11` top byte is the rate index: family + 4 × (multiplier − 1), so
  `0 = 32k`, `1 = 44.1k`, `2 = 48k`, `4 = 64k`, `5 = 88.2k`, `6 = 96k`,
  `8 = 128k`, `9 = 176.4k`, `10 = 192k`. Family word `0x10` index 48 selects
  the 32/44.1/48 kHz base; the alternate setting selects the multiplier.
- The fourth DDS word is 25/24 of the 48 kHz-family clock (50 kHz at nominal
  pitch) and tracks pitch with the other three.
- The device keeps its rate across sessions and personality switches; the
  engine commits the rate on every cold start.

### Transport and streaming

- The interrupt (PC) and isochronous (AP) streaming cores are verified clean
  and share the gated period signal, ring-pressure counters and
  transferred-frame validation. Quantum-48 residual zero-runs were attributed
  to the host (PipeWire follower), not the driver.
- ISO IN body is planar (channel-major); OUT is frame-major with one lead
  word per frame; the wire format is float32. `start_frame` is in microframes.
  The IN header is a 4-byte value: bits 0..4 frame count, bits 5..17 position,
  bits 18..31 a 14-bit fractional-sample accumulator (modulus 10000).
- Interfaces 3 and 4 are dormant in normal Windows operation. Interface 3 is
  the firmware programming channel (verified against a Flash Update Tool run;
  its page is volatile). Interface 4 is a sample-clocked page stream.

### Metering

- Meter pages are 324 bytes and alternate as a heartbeat: one peak page, one
  RMS page, at roughly 190 pages per second. Role is positional.
- Analog input records are a linear 16-bit peak in the upper halfword; digital
  inputs are a linear 24-bit peak in bits 8..31 with a low-nibble
  full-scale-sample count. Playback and hardware-output records use the linear
  16-bit peak and an OVR word.
- The peak page header carries output RMS for AN5..ADAT8 groups; the RMS page
  carries the input, playback and AN1..AN4 banks. Output RMS words follow the
  row's `0x1a` output fader.
- Device power-up anchors the heartbeat at the RMS half. Slips occur at
  streaming engine starts and stops; the driver's shape check realigns them.

### Matrix and control decode

- Matrix cells: `wIndex = block × 36 + source`; level `0x2000` is 0 dB. Output
  masters use `wIndex = 992 + block` with four submix banks in bits 14..15.
- Output level uses `0x1a` with `code = 6 − dB`; input phase uses `0x18`
  (coefficient `0x0156` normal, inverted variant); `0x17` is the switch and
  reference register.
- Loopback uses `0x15`; every change writes the complete 30-entry bank.
- MIDI is on interface 2: two logical ports on bulk OUT `0x07` / IN `0x86`,
  USB-MIDI packets, and the unit accepts both the class code indices and
  RME's length codes.

### Front panel and latency

- The encoder cycles inputs, playback, `PH`/`SP` and outputs; a long press
  toggles stereo setup. Front-panel events are 4-byte `0x8b` frames (byte 2
  level code, byte 3 channel). The AP personality delivers them on interface 4
  bulk.
- Measured device round-trip is about 215 frames at 1× (32/44.1/48/96/192 kHz
  tables), with a separate AP-mode table.
- Float wire is the cheaper host path on Linux: it matches PipeWire's
  `FLOAT_LE` with a vectorizable copy (~0.02 ns/sample) whereas S32 forces a
  per-sample integer conversion (~1 ns/sample, 50–70×). The format does not
  change audio latency; latency is set by the USB queue, ALSA period/buffer
  and device round trip. The transport rule accepts the S32 conversion cost
  on PC for the authentic reference pairing; the conversion runs in the
  URB completion, off the control path.

## Open work

### Driver (see `docs/spec/DRIVER-DESIGN.md` §10–12)

Driver-level open items, risks, TODOs and heuristic waits live in the driver
design specification. Do not duplicate them here.

### Initialization and sample format

| Item | Status | Notes |
|---|---|---|
| UC PC S32 regression | needs-device | The transport rule carries S32 on every PC personality (#191); the S32 pairing was verified clean on firmware 127. Confirm on the firmware 133/138 units when they are on the bench. |
| UCX interrupt (IF5) format and mode | done | S32 is a strict requirement: float wire distorts. The authentic Windows connect block (60-slot init, DDS quad, settings group with settle) and alt 1 streaming are implemented; bench-verified with sine, capture and music (#191). |
| AP-mode bench pass | open | Bench-pass the UC float path in AP mode (the last session ran in PC mode). |
| Power-on default format and rate | needs-capture | Retained state survives a USB reset; a cold power cycle is untested. |
| Undecoded field semantics | open | The masked settings-group bits (`0x3000`/`0x0800` masks, bits 11..13), the fourth DDS word's clock purpose, the periodic status reads (`0x19` Mac, `0x1e`/`0x1f` Windows), and the `0x10 0x0800/0xb800` settings word. The init sequence itself is complete. |

### Clock status and LTC

| Item | Status | Notes |
|---|---|---|
| Lock/no-lock source | needs-capture | Which request or stream carries the input clock lock state (`0x11`, `0x8b` or `0x85`). |
| Lock bit layout | needs-capture | Bits for optical, S/PDIF coax, word clock and LTC lock. |
| LTC transport | needs-capture | LTC packet format, rate and endpoint; current captures show no LTC payload. |
| GUI lock mapping | open | How the status maps to the clock-source picker and the lock indicator. |

### Front panel

| Item | Status | Notes |
|---|---|---|
| `SU` position in the cycle | needs-capture | Where the setup entry sits in the encoder cycle. |
| Bottom-two output codes | needs-capture | Event codes for −58 dB and mute. |

### Loopback and matrix

| Item | Status | Notes |
|---|---|---|
| Loopback destination indexing | needs-capture | Destination indexing does not follow "same-numbered" everywhere: indices 8/9 fed the S/PDIF input records, 10/11 produced nothing, and 4/5 fed the AN1/2 records in an earlier session. |
| Metering freeze above 0 dB | needs-capture | A matrix write near +6 dB froze the looped-back input's meter; the `0x1a` channel level tracked cleanly. Characterise the >0 dB matrix path or the loopback of an over-driven output. |
| Loopback vs input meter | needs-capture | The looped-back signal drove the destination input's meter record, which needs reconciling with the reported placement. |
| Three readouts | open | Matrix cell, per-submix source fader and output master as three distinct readouts. |

### Interfaces 3 and 4

| Item | Status | Notes |
|---|---|---|
| Pre-erase page reader | open | The tool reads the page buffer twice before erase; the purpose is not decoded. |
| Interface 4 field decode | needs-capture | What the two clocked fields and the 32 slots represent; correlate with the meter page and stream positions. |
| Zero-length `0x88` command | needs-capture | The zero-length write on `0x88` differs from the data write on `0x09`; whether it carries a command, and which, is not decoded. |

### Protocol decode (transport)

| Item | Status | Notes |
|---|---|---|
| Read form of `0x10` | needs-capture | The options bits under `0x05cf` read back and the rate commit preserves them (#195). Bit 0 is word-clock input termination (#205). The readback carries undocumented bits above the mask, so a raw readback is never written back; the host writes the model value under the mask. The other bits are undecoded. |
| Word-clock termination revision scope | needs-probe | The UC had two main-board revisions. **Verified on an earlier UC (FireFace 400 board, firmware 127, AP/ISO, recessed switch + "75" LED): bit 0 is inert** — writable and reads back as written, but it neither drives the termination LED nor mirrors the switch; termination is hardware-switch-only. The later UC (UCX board) implements the switch in software (contributor's firmware 138 unit, #205). Consequence: the settings row must not be claimed as software control for every UC; on an earlier UC it neither controls nor reflects termination, so it should be hidden. Open: how to detect the revision in software (firmware threshold, capability, or USB id). |
| UCX termination lost at connect | open | The UCX has the same class of defect: the settings read uses a length the control endpoint does not answer, so the model keeps its defaults and a connect overwrites the device's termination. Read `0x10` with the length the endpoint answers, keep the options under `0x05cf`, and stop overwriting from a stale scene. The read form returns the options under the mask (measured on UCX firmware 46), contradicting the UCX spec §11.5 "status word" claim, which needs correcting. Fast-follow to #205. |
| Status bytes 0 and 1 | needs-capture | Front-panel events use bytes 2 and 3. |
| Bulk role of `0x03`/`0x84` | needs-capture | Bulk at 1x/2x, interrupt at 4x; the bulk role is undecoded. |
| PC interrupt URB size | open | Documentation only: 48/96/192 frames is the host's choice, not a device contract. |

### Metering follow-ups

| Item | Status | Notes |
|---|---|---|
| Beat-word semantics | needs-capture | Bytes 4..8 are session-dependent engine fields with no page-identity role. |
| Three uses of `0x1fff` | open | No-update sentinel, RMS no-value and beat-word value; whether one code is intentional is unknown. |
| Header status-slot / RMS-bank inventory | needs-capture | The fields drift within a session and do not track the signal; no law is published. |
| Digital ADAT 3..7 identity | needs-capture | The ADAT input record identity is unverified; needs an ADAT source. |
| Heartbeat re-anchor / meter re-arm | needs-capture | Which event re-anchors the heartbeat mid-session; whether the Windows driver sends a re-arm request on meter open; the source of free-running status events. |

### GUI: gated controls

Resolved by #190: the gate is capability-based (`protocol_feature_allowed`
in `app.rs`), and the UC profile declares what the backend implements.

| Item | Status | Notes |
|---|---|---|
| Pitch | done | Implemented through the `0x1b` DDS mapping; works on UC and UCX (bench-confirmed). |
| Output phase | done | Per-side output phase words, measured; un-gated via `caps.output_phase`. |
| Width / MS Proc / per-channel width | done | UC matrix rewrites implemented and unit-tested; un-gated via `caps.width`/`caps.channel_width`/`caps.ms_proc`. |
| Dim, Speaker B, Talkback (send and receive) | done | Host-side monitor operations implemented, including `SpeakerBLinked`. |
| Input link (global), AN 1>2 | done | Resolved as unsupported on the UC (`caps` false); the per-pair stereo link remains. |
| EQ / dynamics / FX / AutoSet / reference-level switch | done | Decision: keep gated — the UC has no DSP and no decoded write. |
| Reverse checks (`Dim`, `SpeakerBLinked`, `TalkbackReceiveChanged`) | done | They reach implemented setters. |

### GUI: channel controls

| Item | Status | Notes |
|---|---|---|
| Input Link | done | Unsupported on the UC (`caps.input_link` false); the stereo button links input pairs. |
| Pitch steps | done | Implemented with pitch (#190); normal 0.1 %, Shift fine 0.01 %, wheel 0.1 %, double-click 0.0 %. |
| `0x17` index `0x1000` evidence | open | The live read returned zero at the documented indices; decide whether the existing capture evidence is enough or a focused capture is wanted. |

### GUI: settings card

| Item | Status | Notes |
|---|---|---|
| Graph xruns row | done | `graph_xruns` with baseline and reset in the device sheet, separate from the transport-error rows. |

### Validation and captures

| Item | Status | Notes |
|---|---|---|
| Physical-loopback bit-exact check | needs-capture | The audio-content gate for both transports; needs a cable. Scheduling claims are sound on the dither substrate; pitch/level/mapping under load are unverified until this runs. |
| Mode-2 early capture-content zeros | open | Likely a channel-mapping artifact (ADAT 5/6 are ALSA indices 12/13; the old trace checksummed a silent channel). Verify against the PC scene routing. |
| q48 escalation experiment | done | Driver-side attribution is complete: the gate is serialized (#197), the event drain is protected (#193), and the stress runs show zero gate violations. The residual zero-runs are host-side (PipeWire follower and client buffering); characterising them further is outside the driver backlog. |
| AP meter profile verification | done | AP meters behave the same as PC meters: same DSP firmware, alternation verified in AP, RMS-then-peak order carried from the PC verification; bench-confirmed. |

### Tooling and consolidation

| Item | Status | Notes |
|---|---|---|
| Counters into the hwdep UAPI | open | Move the session counters into `ffuc_hwdep_counters` and extend `ffuc-counters` so a non-root reader sees them. |
| Session scripts into `scripts/` | open | Generalise the battery/phase-lead/loopback procedures; document in `DEVELOPMENT-TOOLS.md`. |

## Recently closed

- UC channel controls completed: pitch (UC and UCX), output phase, width,
  MS Proc, per-channel width, Dim, Speaker B, talkback; capability-based
  gate; graph-xruns row (#190).
- Transport-keyed wire format: interrupt S32, ISO float32; the host-OS rule
  and the `INT32_SAMPLES` quirk replaced; `0x16` derived from the resolved
  format for every model. UCX IF5 authentic Windows mode setup and alt 1
  streaming, bench-verified (#191).
- Host event drain separated from control I/O: shared control-I/O worker,
  ordered scene batches, bounded coalescing command queue; bench freeze
  629 ms → 0 ns (#193).
- UC options preserved across the rate commit (read-back under `0x05cf`),
  with the read form verified on hardware (#195).
- Duplex period gate serialized (`gate_lock`); hwdep teardown ordered
  (quiesce at disconnect, frees at card release); hotplug stress with an
  active reader clean (#197).
- UC protocol spec wire-format sections aligned with the transport rule
  (#196, #198).
- Kernel log hygiene: per-start/stop narration to dynamic debug, anomalies
  ratelimited (#199).
- Channel-init sample format derived from the wire format (#183, superseded
  by the transport rule in #191).
- Mode detection (UC PC / UC AP) verified.
- Interface 3/4 role established (firmware programming channel / page stream)
  and moved into the specification.
- ISO transport consolidation and closure (#176, #177).
- Meter heartbeat slip realign (#175), ballistics timing and per-device rates
  (#174).
- Configurable VU fallback (#178, #179).
