# Fireface UC, UC_WIN_RME: open questions

Working notes for the PC personality matched as `FFUC_PRODUCT_UC_WIN_RME`
(firmware 133 and 138 on the test units).  Not part of the specification.

## Sample format and hardware revision

The RME Flash Update Tool maps the two UC hardware revisions to firmware
127 (Hw Rev 3) and 138 (Hw Rev 6).  Firmware 133 on `UC_WIN_RME` carries
float32; firmware 138 carries S32.  Firmware 134 to 137 is unmeasured and
treated as float32.

The `UC_WIN_RME` descriptor therefore carries the S32 format from firmware
138 upward and float32 below it.  Other UC descriptors keep their declared
float32 format; the firmware revision does not override them.  The driver
resolves the choice once at probe into `chip->s32_samples`; the capture and
playback handlers only read that field.

## Period signalling on the isochronous transport

PipeWire drops out at quanta of 512 and above when capture drives the graph
and playback follows on the same card: it reads the follower with
`snd_pcm_avail_update()`, which returns the follower's hw_ptr as of its last
period, and the two directions' periods end within one URB of each other.
The interrupt transport now updates the other direction's hw_ptr before it
signals a period (`ffuc_period_elapsed()`).  The isochronous transport
(UCX, AP personalities) still signals only its own direction; whether it shows
the same dropouts has not been tested.

## Quantum-48 capture-side dropouts

The following observations use the PC personality, firmware 127, the interrupt
transport, and an internal ADAT 5/6 loopback.  The test plays a stereo noise
file and records the loopback at 18 channels, 48 kHz, S32.  Captures and raw
state remain outside the repository.

### Reproduction

- At quantum 48, four runs produced three to five internal zero runs per run.
  Each zero run is exactly one quantum long.
- At quantum 64, three of four runs had a zero run.
- At quantum 80, one of four runs had a zero run.
- At quantum 96, three of eight runs had a zero run.
- The quantum change alters the failure rate; it does not remove the error.

These results rule out a quantum choice as a fix.

### PipeWire and WirePlumber settings

The Fireface Pro nodes use `api.alsa.disable-tsched = true`,
`api.alsa.period-num = 2`, and `api.alsa.headroom = 0`.
At quantum 48, each of these alternatives still produced zero runs:

- `api.alsa.period-num = 4`;
- `api.alsa.headroom = 96`;
- `api.alsa.disable-tsched = false`.

The settings are not the root cause.

### Driver timing trace

A bpftrace trace at quantum 48 recorded every capture and playback URB and
every `ffuc_period_elapsed()` call.

- Every URB carried 3456 bytes, which is 48 frames of 18 channels at four
  bytes per sample.
- Period signals for both directions were present at about one-millisecond
  intervals.
- The largest observed interval between period signals was about 1.44 ms.
  A missing 48-frame period would produce an interval of about 2 ms.  No such
  gap was present at a zero run.
- At every playback-boundary signal, the capture count was one URB behind.
- At every capture-boundary signal, the playback count was current.

The driver is not losing a period signal or a URB at the zero-run positions.

### Buffer-content trace

A second trace sampled the playback and capture copies at quantum 48.  The
checksum covered the 48-frame URB on playback channel 1 and capture channel
15 in the one-based channel labels.

- At the later internal zero runs, the playback copy and the capture copy
  both contained nonzero signal.  The recorded WAV still contained zeros.
  Those zeros were inserted after `ffuc_capture_copy()`.
- At the first three internal zero runs, the capture copy checksum was zero
  while the playback copy was nonzero.  This is a separate capture-content
  mode and is still open.

The content-present zeros are therefore in the host capture path: the
PipeWire capture follower or the recording client.  They are not explained by
a driver period-signalling defect.

### Tested driver variant

A test variant signalled only the direction whose URB had completed and
removed the cross-direction signal.  At quantum 48 it made the result worse.
One four-run batch contained a run with forty zero runs.  A later eight-run
batch produced between three and fourteen zero runs per run.  The variant was
reverted and not merged.

This is evidence against the cross-direction signal being the whole error.
It does not explain the content-present zeros.

### Why a period-signalling patch does not fix this mode

The period signal can only wake an ALSA reader.  It cannot remove a zero that
is inserted after the driver has copied nonzero samples into the capture ring.
A patch that changes the signal boundary or its order can help a unit where the
loss occurs at the driver wake.  It cannot fix the content-present mode on this
unit.

### Open items

1. Distinguish a PipeWire capture-follower resync from a `pw-record` client
   xrun at the exact zero positions.
2. Explain the early capture-content mode, where the capture copy is zero and
   playback is nonzero.
3. Repeat the quantum-128 control enough times to state its failure rate.
   The quantum sweep is not a fix.
4. Determine whether the zeros affect the device output, or only the captured
   and monitored streams.

## q48 instrumentation build (branch `test/uc-q48-instrumentation`)

A diagnostics-only driver build for the quantum-48 zero runs. No logic
change: it counts and logs, everything else is identical to the reference
signalling. The counters appear in the existing debugfs `stats` file next to
the transport counters.

- `in_overruns`: capture completions that overwrote ring frames the reader
  had not consumed yet (`[appl_ptr, hw_ptr)` span plus the new payload past
  `buffer_size`, compared modulo `runtime->boundary`).
- `out_underruns`: playback completions that transmitted from a ring span
  the application never wrote.
- `in_bad_length`: status-0 IN URBs whose `actual_length` is not a whole
  frame multiple or exceeds the configured URB size.
- `hw_ptr_mismatch`: `snd_pcm_period_elapsed` refreshes where the ALSA
  core's published `status->hw_ptr` differs from this driver's pointer;
  the first occurrence prints both.
- Poison fill: `prepare` fills the PCM ring with `0x00005A5A` (near-silent
  in float32 and S32) under the `ffuc_poison` module parameter. A recorded
  zero therefore means explicit silence, never inherited memory.
- Each stream start logs rate, the URB queue (`nurbs` x `urb_frames`), the
  granted `buffer_size`, `period_size`, `stop_threshold`, `start_threshold`,
  `avail_min` and `delay`.
- The payload-format question is closed by arithmetic: a wire-format
  misread of a streaming payload yields continuous noise or full-scale
  bursts, not clean audio with isolated one-quantum zero runs. Both
  conversion paths are reviewed and unit-correct. Firmware 127 carries
  float32.

### What one capture session discriminates

- `in_overruns` rising at zero positions: the ring margin is the owner.
  Test standing fill next (`period-num` 4 or 8 together with a headroom of
  about one period; the earlier knob matrix never raised the standing
  fill), then the `urb_frames` sweep 8/16/48 through the module parameter.
- `in_overruns` and `out_underruns` at zero with `hw_ptr_mismatch` rising:
  the core's pointer heuristic is the owner.
- Both counters at zero: the zeros are inserted above ALSA, and PR #160's
  wake ordering is the mechanism. The capture-side repro decides it.
- `stop_threshold` equal to `boundary`: the ALSA core's own overrun and
  underrun detection is disabled; silent ring corruption is then expected
  core behaviour, not a driver defect.

## First instrumented session (2026-11-14)

Findings from the first session on the firmware 127 bench unit. Captures
and session state stay outside the repository.

Negotiated geometry, from the per-start log line. Both directions at a
quantum of 48 run `buffer 192, period 48, stop_threshold 192, avail_min
144 playback / 48 capture, delay 96`; `stop_threshold` is therefore
`buffer_size`, not `boundary`, and the core's own overrun and underrun
detection is enabled. The reviewer's q48 geometry premise (a 96-frame
ring) does not hold; the standing fill target stays at about one period
regardless of ring size.

- Ring pressure: `in_overruns` and `out_underruns` stayed at zero through
  roughly 13 runs (18-channel and stereo tone, quanta 48/64/96/256, with
  and without a concurrent full-tree `cargo build` on processors 0-15).
  The ring margins never went bad in any run.
- Payload integrity: `in_bad_length` stayed zero.
- Pointer divergence: the `hw_ptr_mismatch` instrument fired twice, both
  on the playback direction at a quantum of 256 (`buffer 512`). The first
  event reported `status 3680240` against `driver 3679728`: the ALSA
  core's period-elapsed heuristic injected exactly one `buffer_size`
  into the published pointer. The mechanism is real. In the observed
  instance the injected pointer produced no zero run in the recording.

### The zero run reproduced once, above ALSA

One capture run reproduced the failure: a 4-second phase lead — capture
started first, playback joined later, quantum 48, 18 channels, S32,
capture driving the graph. One interior zero run of exactly 48 frames
appeared at stream frame 738960 (about 15.4 s in, `offset % 48 == 0`).
At that position all four driver instruments stayed silent:
`in_overruns` did not rise, `out_underruns` did not rise,
`in_bad_length` stayed zero, and no `hw_ptr_mismatch` was logged in the
window. The zero was therefore inserted above ALSA by the PipeWire
capture follower, while the driver's ring stayed healthy throughout —
the mode-1 attribution the work notes asked for, now evidenced by
instruments rather than inference.

### Next discriminations

1. Re-run the phase-lead variant several times with the per-second
   `pw-top` ERR column alongside; the zero positions should line up with
   ERR movements on the capture node if PipeWire's follower deadline
   misses.
2. Comment on PR #160 with the session result: the wake-ordering
   mechanism is not the sole owner; its capture-boundary signalling may
   still be the right fix, but the bench now shows a healthy-driver
   instance where the follower still produced a quantum of silence.
3. `urb_frames`/`nurbs` sweep stays useful to bound the driver-side
   margin, but no longer carries the diagnosis; the ring counters say
   the margin has never been at risk in this session.



## Endpoints that differ from the original UC

The descriptor is copied from the original UC.  On the test unit interface 1
has only bulk IN `0x85`; there is no `0x0c` OUT or `0x8d` IN.  Interface 3
has bulk `0x88` IN and `0x09` OUT, interface 4 bulk `0x8a` IN.  Audio,
status (`0x8b`) and EP0 control work; the state and meter paths are
untested.

## AP personality

The unit's AP personality enumerates under a product ID the driver does not
match.  Its interface 0 is isochronous, with alternate settings 1–3.  Not
supported yet; a capture is needed.
