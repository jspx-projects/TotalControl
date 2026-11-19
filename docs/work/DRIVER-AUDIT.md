# Driver audit — snd-fireface-uc and host communication

Audit of the kernel transport driver and TotalControl's driver
communication. The authoritative driver reference is
`docs/spec/DRIVER-DESIGN.md`; this note records the audit result, the
open risks and the host work priorities.

## Scope and method

- Static review of `kernel/ffuc.c`, `kernel/ffuc-hwdep.c`,
  `kernel/ffuc-midi.c` and the ABI headers against USB core and ALSA
  conventions.
- Cross-check against the UC/UCX protocol specifications and the shared
  UC work backlogs.
- `make -C kernel`, `make -C kernel test`, checkpatch; the sparse
  result is recorded under Tooling.
- Live context: the UC test unit (firmware 127) passed PC and AP
  streaming in the recent channel-controls and buffer-size PRs. This
  audit added no new hardware captures.
- Comparison with mainline `snd_usb_audio` (`sound/usb/pcm.c`,
  `sound/usb/endpoint.c`), the ALSA core (`sound/core/pcm_native.c`,
  `include/sound/pcm.h`, `include/uapi/sound/asound.h`) and PipeWire's
  ALSA plugin (`spa/plugins/alsa/alsa-pcm.c`), 2026-11-19.
- The working tree at the time of this audit carries an uncommitted
  ring-fill trace (`ffuc_ring_trace`, the `ringtrace` debugfs file).
  Findings that depend on it are marked.

## Mainline comparison audit — criteria and findings

Basis: static comparison of the module with mainline `snd_usb_audio`
and the ALSA core, plus PipeWire's ALSA plugin and the packaged host
fragments. Code findings are established by inspection; bench impact is
marked hypothesis where no capture or counter exists.

Status vocabulary: **verified (code)** = established by reading the
named source; **verified (source)** = established from the named
external source; **hypothesis** = plausible mechanism, deciding evidence
named; **needs-bench** = requires the listed measurement; **open** = not
started.

### Criteria

#### A. PCM position, timing and xrun contract

| ID | Criterion | Mainline anchor |
|---|---|---|
| C1 | The pointer may lead the device by the in-flight queue, but it never runs past the application's committed `appl_ptr`. | `prepare_playback_urb()` caps at `avail` in low-latency mode; `hwptr_done` advances only with queued data. |
| C2 | `.pointer` returns a coherent position and refreshes `runtime->delay` from data actually in flight. | `snd_usb_pcm_pointer()` sets `runtime->delay = snd_usb_pcm_delay()`. |
| C3 | A period signal announces exactly one completed period; no delta-0 calls; `_under_stream_lock()` used where the lock is held. | `prepare_playback_urb()` tracks `transfer_done` and signals once per `period_size`. |
| C4 | Period and buffer constraints derive from URB geometry and agree across `open`, `hw_params`, `prepare` and the reported envelope. | `setup_hw_info()` constrains period time, 1 s period, 2 s buffer. |
| C5 | The ring minimum covers the device lead plus the host's occupancy target; the host wake threshold provides the scheduling margin. | Buffer sizing is left to the application within the 2 s ceiling. |
| C6 | A rate change is refused or serialised while either direction runs; one direction owns the clock rate. | The first `hw_params` fixes the rate; the other direction is constrained to it. |
| C7 | Starvation and overrun become `SNDRV_PCM_STATE_XRUN`; the driver never masks them with stale ring data. | `notify_xrun()` calls `snd_pcm_stop_xrun()`; `-EAGAIN` stops URB preparation at `appl_ptr`. |
| C8 | `prepare`/`trigger`/`hw_free` ordering is defined; a pending stop is synchronised with the core through `.sync_stop`. | `snd_usb_pcm_sync_stop()` and `sync_pending_stops()`. |
| C9 | Suspend, resume, reset-resume and disconnect unwind URBs, works and buffers without racing completions. | `snd_usb_endpoint_suspend()`, shutdown lock, `stop_endpoints()`. |
| C10 | Duplex linkage is declared when both directions share one clock (`SNDRV_PCM_INFO_JOINT_DUPLEX`, linked rate constraint). | Mainline sets `JOINT_DUPLEX` for implicit-feedback formats. |

#### B. URB and endpoint engine

| ID | Criterion | Mainline anchor |
|---|---|---|
| C11 | One owner and one lifecycle per URB; no resubmit after kill. | `ep_state_update()`, `active_mask`, `submitted_urbs`. |
| C12 | Completion status is classified: shutdown stops; `-EXDEV` and per-packet errors are data; transient errors do not restart the engine. | `snd_complete_urb()` treats only shutdown statuses as terminal. |
| C13 | ISO packet geometry comes from descriptors and is validated before submission. | `data_ep_set_params()`, `endpoint_set_syncinterval()`. |
| C14 | `start_frame` continuity is managed; `URB_ISO_ASAP` is used only where it is needed. | Mainline sets `URB_ISO_ASAP` only for a quirk; otherwise it submits in order. |
| C15 | Implicit feedback is packet-accurate: output sizes come from the sync source, one output packet per input packet, primed without empty OUT packets. | `snd_usb_handle_sync_urb()`, `snd_usb_queue_pending_output_urbs()`. |
| C16 | Zero-frame, errored and stale feedback packets are handled explicitly. | Empty-packet handling and per-packet status checks. |
| C17 | Completion context does no sleeping, allocation or unbounded work. | Retire/prepare only; heavy work outside completion. |
| C18 | DMA buffers are coherent, correctly sized and carried with `URB_NO_TRANSFER_DMA_MAP`. | `usb_alloc_coherent()` per URB context. |
| C19 | The engine stops submitting when the application is behind and resumes when it catches up. | `-EAGAIN`, ready list, `notify_xrun()`. |

#### C. ALSA API surface

| ID | Criterion | Mainline anchor |
|---|---|---|
| C20 | `info` accurately advertises `BATCH`, `PAUSE`, `SYNC_APPLPTR`, `RESUME`, `JOINT_DUPLEX`. | `snd_usb_hardware` sets `BATCH` and `PAUSE`; `SYNC_APPLPTR` for low latency. |
| C21 | Period/buffer time ceilings exist; format, channel and rate masks match the device. | 1 s period, 2 s buffer, per-descriptor format rules. |
| C22 | `avail_min` and poll semantics are honoured. | ALSA core poll compares `avail` with `avail_min`. |
| C23 | Locking is documented and consistent; no inversion between stream lock, engine mutex and completion path. | Endpoint lock, chip mutex, shutdown lock ordering. |

#### D. Probe, descriptors and lifecycle

| ID | Criterion | Mainline anchor |
|---|---|---|
| C24 | Matching validates interface class/subclass/protocol and endpoint shape. | `snd_usb_audio_probe()` checks class, subclass and protocol. |
| C25 | Endpoint pipes come from descriptors; no hardcoded endpoint addresses in the shared path. | Endpoint descriptors parsed per format. |
| C26 | Device differences live in data; every quirk is used; timing constants are named. | `quirk_flags`, `quirk_table`, `fmt->ep_attr`. |
| C27 | Probe and teardown unwind every allocation and claim exactly once. | `snd_usb_audio_probe()` error ladder. |

#### E. Diagnostics and correctness

| ID | Criterion | Mainline anchor |
|---|---|---|
| C28 | Counters name the pointer comparison they use, use signed deltas where the application can fall behind, and separate idle from active traffic. | `xrun_debug`, per-endpoint counters, tracepoints. |
| C29 | Position, lead and feedback arithmetic is correct at every rate band, including wrap width. | Width derived from format and rate. |
| C30 | Clean under `sparse`, `checkpatch --strict`, lockdep and UBSAN; no dead code or unused fields. | CI runs these for `sound/usb`. |

#### F. Host integration

| ID | Criterion | Why it matters |
|---|---|---|
| H1 | The driver's period floor and buffer minimum agree with PipeWire's `period-num`, quantum and `disable-tsched` policy. | A forced buffer minimum larger than the requested graph buffer changes periods behind the graph's back. |
| H2 | The host wake threshold (`api.alsa.headroom`) provides a real scheduling margin above the driver's ring minimum. | The wake threshold, not the ring size, decides how long the host has before starvation. |
| H3 | XRUN recovery is exercised: driver reports it, PipeWire re-prepares, device re-locks. | A driver that never reports xrun never triggers recovery. |

### Findings, priority order (P1–P3 are defined below)

| ID | Finding | Criteria | Evidence | Priority |
|---|---|---|---|---|
| R1 | Playback starvation is counted, not prevented. The fill path transmits stale ring data and never calls `snd_pcm_stop_xrun()`. | C7, C19 | verified (code) | P1 |
| R2 | At q48 the host reaction margin is one period (about 1.1 ms), because `api.alsa.headroom` is unset. | C5, H2 | verified (source, PipeWire) | P1 |
| R3 | The starvation counter compares against the published `status->hw_ptr` and uses modular subtraction, so it misses the "application behind" case. | C28 | verified (code) | P1 (investigation) |
| R4 | `runtime->delay` is set once at prepare and never refreshed in `.pointer`. | C2 | verified (code, ALSA) | P2 |
| R5 | `SNDRV_PCM_INFO_BATCH` and `SNDRV_PCM_INFO_JOINT_DUPLEX` are missing. | C20, C10 | verified (code, source) | P2 |
| R6 | No `.sync_stop` callback; URB teardown is synchronous in `hw_free`. | C8 | verified (code) | P2 |
| R7 | ISO playback priming submits zero-length OUT packets and re-anchors the lead sequence at `START`. | C15 | verified (code); impact hypothesis | P2 |
| R8 | The cadence guard uses a cumulative histogram, includes zero-frame packets and survives rate changes, so it fires at idle. | C28 | verified (code); observed on the bench | P2 |
| R9 | The ISO position wrap is fixed at 8192 at every rate; the reference widens it at 2x/4x. | C29 | verified (code, spec); impact needs capture | P2 |
| R10 | A burst of 16 non-shutdown URB statuses restarts the engine; mainline restarts only on shutdown statuses. | C12 | verified (code); impact hypothesis | P2 |
| R11 | Capture overrun is counted but not reported. | C7 | verified (code) | P2 |
| R12 | Smaller defects: dead ISO validation block, unused `IF5_PRE_START` quirk, hardcoded interrupt pipes, MIDI IN halt never cleared, `in_shorts` never incremented, `status_active` race, `status_urbs` always zero, 64 KiB allocation per hwdep read. | C25, C26, C30 | verified (code) | P3 |

#### R1 — Playback starvation is masked

`ffuc_iso_playback_fill()` and `ffuc_playback_urb()` fill every URB from
the ring. When `total > filled`, they increment `out_underruns` and copy
the old ring content into the URB anyway. No path calls
`snd_pcm_stop_xrun()` for a host-ring underrun. The graph receives a
period tick with no error and no recovery; the audible result is the
repeated stale buffer.

Mainline's low-latency playback path caps prepared frames at `avail`
(`prepare_playback_urb()` returns `-EAGAIN`), keeps the URB on a ready
list, and calls `notify_xrun()` when no URB can be prepared. Implicit
feedback playback in mainline relies on the sync-source packet FIFO and
still needs an adequately sized application buffer; it is not a complete
prevention model either.

Two-part fix, in this order:

1. Never transmit unwritten ring data. Fill the unwritten part with
   silence and count it. Silence is a defined signal; a repeated stale
   buffer is not.
2. Report the xrun with `snd_pcm_stop_xrun()` from outside the PCM
   stream lock, so the graph re-prepares and re-anchors. A grace of two
   consecutive starved URBs avoids a stop on one late packet.

#### R2 — The host wake threshold is the underrun budget

PipeWire's ALSA plugin sets playback `avail_min = buffer_frames -
(threshold + headroom)` and wakes when the free space reaches it. With
the packaged fragments (`api.alsa.headroom` unset, `disable-tsched=true`,
`period-num=2`), headroom is zero. At q48 PipeWire wakes with one period
of audio left: 48 frames at 44.1 or 48 kHz, about 1.1 ms. A wake-up
delay beyond that is R1.

The driver's ring minimum (288 frames at 44.1 kHz, 336 at 48 kHz) is not
a scheduling margin. PipeWire writes the buffers the graph delivers and
lets the ring drain to the wake threshold. That threshold is the
scheduling deadline. Raising `nurbs` raises the lead and the ring
together and does not move the wake threshold. Raising `period-num`
changes the write burst, not the threshold.

The fix is `api.alsa.headroom`. PipeWire caps the wake headroom at
`buffer_frames - threshold`; at that point `avail_min` reaches zero and
PipeWire keeps the ring full. The reported latency metric is separately
clamped to `buffer_frames / 2`. A moderate value gives a useful margin
without filling the ring completely. The value is in frames, so the same
number is a shorter time at a higher rate; scale it with the rate for a
constant margin.

#### R3 — The starvation counter reads the wrong pointer

`filled` uses `rt->status->hw_ptr`, the last published ALSA pointer,
instead of the ring position the completion is about to read
(`s->pos[...]`). The published pointer lags the driver position by up to
a period, so the check fires late or not at all.

Modular subtraction also cannot express "the application is behind".
When `appl < read_pos`, `(appl + boundary - read_pos) % boundary` is near
`boundary`, not negative. The uncommitted `ringtrace` `STARVED` flag has
the same defect. Use a signed difference, clamp at zero, and record
`s->pos` in the event.

#### R4 — Delay is static

`ffuc_pcm_prepare()` sets `runtime->delay` once to the queue duration.
ALSA computes `snd_pcm_delay()` as `hw_avail + runtime->delay`
(`sound/core/pcm_native.c`, `include/sound/pcm.h`). Mainline refreshes
it in `.pointer` from `inflight_bytes` minus the elapsed USB frames.
Clients that read delay (JACK, DAWs, latency reporting) see a constant.
PipeWire's ALSA plugin uses `snd_pcm_avail` for its own scheduling, so
this is a contract defect rather than the direct cause of R1.

#### R7 — Priming sends empty OUT packets

`ffuc_iso_setup_urbs()` calls `ffuc_iso_playback_fill()` before any
input feedback exists. `frames[i]` is zero, so the first playback URBs
are submitted with zero-length packets. The lead counter is initialised
only when the stream is running, so pre-`START` lead words start at zero
and are re-anchored when the trigger arrives.

Mainline does not submit implicit-feedback sink URBs until the sync
source returns packets; it queues one pending output packet per input
packet. The mechanism in this driver is verified by code. Whether it is
audible at start needs a usbmon capture of the first 20 ms.

#### R8 — The cadence guard fires at idle

`ffuc_iso_cadence()` averages the cumulative `iso_frames_hist`, which
includes zero-frame header-only packets and is not reset across a rate
change. Idle header-only packets lower the mean, and the guard reports a
rate mismatch. The handover recorded `rate_mismatch` rising at idle.
A windowed histogram that excludes `frames == 0` and resets at a rate
commit removes both causes.

#### R11 — Capture overrun is counted, not reported

The capture path counts `in_overruns` when a completion would overwrite
unread frames, then writes anyway. A recording client sees corrupted
audio with no error. The same policy decision as R1 applies.

#### R12 — Smaller defects

| Defect | Location | Impact |
|---|---|---|
| ISO `WARN_ON` validation block is inside the `else` of `if (chip->iso)` | `ffuc_probe()` | the checks never run; dead code |
| `FFUC_QUIRK_IF5_PRE_START` is set but never tested | `ffuc.c` | the words are sent unconditionally, including to the Babyface |
| interrupt pipes hardcoded as `1 + 2e` / `2 + 2e` | `ffuc_int_setup_urbs()` | a model with different endpoint addresses streams on the wrong pipe |
| MIDI bulk IN resubmits on `-EPIPE` without `usb_clear_halt()` | `ffuc_midi_in_complete()` | MIDI input stops permanently after a stall |
| `in_shorts` is never incremented | `ffuc_track_capture_urb()` | the counter is always zero |
| `status_active` is set after `usb_submit_urb()` | `ffuc_status_submit()` | an inline completion can wedge the ticker |
| `status_urbs` is never incremented | `ffuc_hwdep_counters()` | the COUNTERS field is always zero |
| 64 KiB buffer allocated per hwdep read | `ffuc_hwdep_read()` | reclaim latency in the event path; known item |

### The low-quantum underrun model

Terms used here:

| Term | Definition |
|---|---|
| period floor | one URB, in frames |
| lead | `out_lead`, the stream-position offset the driver labels OUT frames with |
| ring minimum | `ffuc_buffer_min()`, the smallest ALSA buffer the driver accepts |
| wake occupancy | frames left in the ring when PipeWire wakes: `threshold + headroom`, where `threshold` is the graph quantum |
| reaction time | wake occupancy divided by the rate |

The driver's ring minimum includes the lead because the reference
geometry was measured with a ring smaller than the lead and crackled.
That is recorded in the code. The exact relationship between the ring
size and the lead is a bench observation, not derived here. The host
reaction time is separate: it is the wake occupancy, not the ring size.

At the default `urb_frames=48`, `nurbs=2`, `iso_packets=8`, with
PipeWire's headroom at zero and at a fixed 128-frame headroom. Each row
uses a quantum equal to the period floor; at 96 and 192 kHz that is q96
and q192, not q48.

| Rate | Period floor | Lead | Ring minimum | Wake at h=0 | Reaction | Wake at h=128 | Reaction |
|---|---|---|---|---|---|---|---|
| 32000 | 32 | 156 | 224 | 32 | 1.0 ms | 160 | 5.0 ms |
| 44100 | 48 | 192 | 288 | 48 | 1.09 ms | 176 | 3.99 ms |
| 48000 | 48 | 204 | 336 | 48 | 1.0 ms | 176 | 3.67 ms |
| 96000 | 96 | 348 | 576 | 96 | 1.0 ms | 224 | 2.33 ms |
| 192000 | 192 | 636 | 1152 | 192 | 1.0 ms | 320 | 1.67 ms |

A 128-frame headroom is about 4 ms at 44.1 kHz and 1.7 ms at 192 kHz.
For a constant time margin the value must scale with the rate: 128 at
48 kHz, 256 at 96 kHz, 512 at 192 kHz. A value at or above
`ring minimum - period floor` makes `avail_min` zero; PipeWire then
keeps the ring full, which maximises stability and adds the full ring to
the latency.

The table is arithmetic from the driver functions and PipeWire's
`set_swparams()` and `recalc_headroom()`. It explains the observed
pattern: q48 ticks once per 30–60 s because the host has about 1 ms of
reaction time, and a routine scheduler stall exceeds it. At q256 the
buffer and the write burst are larger, but the wake occupancy still
follows the quantum plus headroom; the observed stability at q256 comes
from the larger quantum, not from the driver ring.

### Proposed driver changes

| Order | Change | Addresses | Priority |
|---|---|---|---|
| 1 | Silence-fill the unwritten part and stop with `snd_pcm_stop_xrun()` after a two-URB grace | R1, R11 | P1 |
| 2 | Replace the modular `filled` check with a signed `ahead` from the driver read position; log `s->pos` | R3 | P1 |
| 3 | Update `runtime->delay` in `.pointer` from in-flight frames | R4 | P2 |
| 4 | Add `SNDRV_PCM_INFO_BATCH` and `SNDRV_PCM_INFO_JOINT_DUPLEX` | R5 | P2 |
| 5 | Add `.sync_stop` that synchronises the URB kill | R6 | P2 |
| 6 | Window the cadence histogram and exclude zero-frame packets | R8 | P2 |
| 7 | Derive the 2x/4x wrap from the rate and verify with a capture | R9 | P2 |
| 8 | Prime from nominal frame counts or defer OUT submission until feedback exists | R7 | P2 |
| 9 | Clear the smaller defects in R12 | R12 | P3 |

### Proposed host policy

| Order | Change | Reason |
|---|---|---|
| 1 | Set `api.alsa.headroom` on the Fireface nodes; 128 frames gives about 4 ms at 44.1/48 kHz | raises the wake occupancy at q48 |
| 1a | Scale the headroom with the rate if the node runs at 96/192 kHz (256/512 frames) | the property is in frames, so a fixed value is a shorter time at a higher rate |
| 1b | At or above `ring minimum - period floor` the headroom makes `avail_min` zero, so PipeWire keeps the ring full | maximum stability, full-ring latency; use only if ticks persist |
| 2 | Keep `disable-tsched=true` until `BATCH` is added and tested | with `BATCH` and timers, PipeWire halves the period and adds a period of headroom |
| 3 | Keep q256 as the default; treat q48 as an expert setting | q48 relies on a 4 ms wake margin |
| 4 | Verify the logged `buffer frames`, `period frames`, `periods`, `headroom` and `batch` values | confirms the driver and PipeWire agree on the envelope |
| 5 | Treat idle `rate_mismatch` and `pkt_status` separately from active-playback events | the current guard counts idle traffic |
| 6 | Keep `session.suspend-timeout-seconds = 0` only if clock lock is required | it keeps the engine active while idle, which feeds R8 counters |

### Evidence needed

| Question | Measurement |
|---|---|
| Does a tick align with `ahead < total` at the completion before it? | `ringtrace` with a signed `ahead` field, q48, one tick |
| What does PipeWire actually negotiate at q48? | the `spa.alsa` log line: buffer, period, periods, headroom, batch |
| Does the first second contain zero-length OUT packets? | usbmon capture of the first 20 ms after `START` |
| Does the 8192 wrap cause a discontinuity at 96/192 kHz? | 2x/4x ISO capture with isolated add/drop events away from the wrap |
| Does `api.alsa.headroom = 128` remove the ticks at q48? | 30-minute q48 run, `out_underruns` and audible check |

## Certain errors — fixed (PR #191, merged)

| Error | Impact if unfixed | Status |
|---|---|---|
| Meter URB coherent buffer allocated `2 * meter_len`, freed `meter_len` in the error path and cleanup | DMA mapping leak per teardown: half the IOVA range stays mapped (IOMMU) or half the bounce slots leak (swiotlb); DMA-API-debug size-mismatch warning. Teardown only, no streaming effect. | done |
| `ucx_if5` static initializer | checkpatch error; style only | done |
| UCX IF5 sent S32 wire with the float `0x16` init value | Format mismatch passes only the sample sign bit: audible distortion on IF5 | done |
| UCX IF5 used the Mac-style connect init | The Windows personality stayed outside its mode: alt 1 silent at the DAC, capture completed zero URBs | done |

## Priority scheme

- **P1** — a failure mode reachable in normal use; fix in the next PR.
- **P2** — degraded behaviour under load or in a specific configuration; scheduled.
- **P3** — diagnostics, performance or cosmetic; cleanup.

## Open driver risks

Scope labels: all = every personality; ISO = UC AP, UCX IF0,
Babyface-classic; status = personalities with a status endpoint.

| Item | Scope | Priority | Impact | Status |
|---|---|---|---|---|
| Duplex period gate deferral counters read/incremented without a shared lock | all | P1 | Established by code analysis: both completion paths increment both directions' counters with non-atomic read-modify-write and no common lock; concurrent completions on different CPUs can release a direction early while its partner is behind (stale-follower wake). | done — `gate_lock` serializes the decision |
| hwdep cleanup runs in disconnect and again in `private_free` | all | P1 | Established by code analysis: disconnect freed the event FIFO without `event_lock` while a reader could be between the disconnected check and the lock (use-after-free window). | done — disconnect quiesces; frees run at card release after the last close |
| `INFO.outputs` reports the playback channel count | Babyface-classic | P2 | The hardware output count differs; a host sizing output state by this field maps meters/levels wrongly. Verify the ABI field semantics before Babyface production use. | needs-verification |
| hwdep read path allocates a buffer per event (`GFP_KERNEL`) | all | P2 | CPU overhead on every event in the drain path; direct-reclaim latency under memory pressure; a failed allocation fails the read. Per-reader preallocation removes it. | optional |
| ISO continuity log counter is module-global | ISO | P3 | The 12-message log throttle is shared across devices and endpoints; continuity diagnostics go silent everywhere after the first 12. Diagnostics only. | open |
| Verbose PCM trigger-stop diagnostics | all | P3 | Unratelimited `dev_info` on every stop: dmesg noise that buries real errors; on a serial console each line stretches the stop path by milliseconds. No effect while streaming. | done — log hygiene PR: per-start/stop/rate/session narration is `dev_dbg`; anomalies are ratelimited warnings; probe identification stays |
| Buffer poison memsets the PCM buffer on every prepare | all | P3 | One whole-buffer memset at stream start (tens of µs at large buffers) plus cache pollution. Already a module parameter (`ffuc_poison`, default on); the remaining decision is the production default. | optional |
| `COUNTERS.status_urbs` reports zero | status | P3 | The host cannot observe status-URB health; the device sheet counter is incomplete. | open |

## Heuristic waits

| Wait | Scope | Priority | Impact | Status |
|---|---|---|---|---|
| Session gaps (250/200/200 ms) | `WIN_SESSION` | P2 | Device-mandated timing, verified in Windows reference-host captures. Removing a gap without a readiness condition leaves the unit silent or wedged (verified failure). A readiness poll needs a capture of a device-visible condition first. | needs-capture |
| Meter/state pump: 4 ms delayed work, 10 ms retry | all | P2 | Up to 4 ms added latency per page plus workqueue jitter under load; caps the page rate near 250/s (the UCX runs ~245/s, at the ceiling); a CPU wakeup every 4 ms. Resubmit from completion; preserve the UC pair-boundary pause. | optional |
| UCX burst-pacing gap (host) | host, UCX | P2 | Throughput ceiling ~63 words per 16 ms; a full scene takes hundreds of ms. Tighter gaps overflow the device receive FIFO (verified at 5 ms). Flow control needs a device-visible drain condition or a bounding capture. | needs-capture |

## Tooling

| Item | Priority | Impact | Status |
|---|---|---|---|
| `sparse` (`make C=1`), lockdep and KCSAN/UBSAN runs | P1 | The evidence base for the two race items above. sparse is installed and reports no warnings on the module; UBSAN is active in the bench kernel and reported nothing during the stress runs. KCSAN, KASAN and lockdep are not built into the Fedora kernel; the races were established by code analysis instead. | done (sparse/UBSAN); KCSAN/lockdep need a debug kernel |
| checkpatch block-comment warnings | P3 | Kernel style compliance for upstream review. | optional |

## Host (TotalControl) communication

Verified current state:

- The service loop is one thread: it applies commands, drains events,
  reads meters and publishes a 50 ms snapshot
  (`totalcontrol-gui/src/device_service.rs`).
- UCX live IF1 writes already run on a dedicated writer thread
  (`AsyncWriter`, `totalcontrol-if1-writer`); the burst-pacing sleeps
  happen there, off the service thread.
- `flush_write_batch` blocks the caller on `writer.flush()` whenever a
  vendor request follows queued words. Scene applies and EP0 vendor
  requests run synchronously on the service thread.
- UC control writes are synchronous on the service thread.
- The command queue is unbounded (`mpsc::channel`).
- The single meter reader is host discipline; the driver does not
  enforce it.

### Host issues — priority and impact

Reproduction evidence (UCX, ISO, firmware 46):

- The stall test (`a_slow_command_freezes_meter_publication`) freezes
  meter publication for the whole slow command on the mock service.
- The backlog test (`a_command_backlog_delays_later_commands`) delays a
  volume move by the full backlog duration.
- Bench stress (`bench_stress_event_drops`, 24 scene+drag floods in
  15 s): longest publication freeze 629 ms, zero new `events_dropped`.
- An unbounded firehose (3000 scene applies queued in 15 s) built
  minutes of backlog, defeated `Stop`, and accumulated ~90k event
  drops. The 64 KiB FIFO fills in about 1 s at the UCX meter rate; a
  629 ms freeze stays below that, the firehose far exceeded it.

| Item | Priority | Impact | Status |
|---|---|---|---|
| Event drain shares the thread with blocking control work (synchronous EP0 calls, up to the 1 s kernel timeout) | P1 | A stalled control path delays `poll_events`. Reproduced: 629 ms publication freeze during a scene apply; meters and model go stale. Event drops require a stall beyond the ~1 s FIFO fill time. | done — drain/worker separation |
| Blocking writer flush (`flush_write_batch` waits for the paced writer) | P1 | A vendor request after queued words waits 16 ms per 63-word burst; a scene is many bursts, so the service thread stalls hundreds of ms. Same freeze class during scene applies. | done — ordered scene batches, no flush wait |
| UC control writes run inline on the service thread | P1 | The UC has no writer thread; a control burst blocks the loop directly. Same stall class as the two rows above. One fix cluster: event-drain independence. | done — UC writes queue on the worker |
| Unbounded command queue | P1 | Reproduced: a 15 s firehose built minutes of backlog, applied stale commands late, and defeated `Stop`; ~90k event drops followed. Bounded queue with coalescing/backpressure. | done — bounded coalescing C1 and worker queue |
| Single meter reader is host discipline only | P2 | A second hwdep reader breaks the UC pair phase model: each reader samples pages, pairs misalign, `PHASE_LOST` flags and wrong RMS/peak pairing follow. Driver enforcement or an explicit ABI contract. | open — later |
| Connect policy writes host settings on connect | P2 | Front-panel changes made while the host was away are overwritten. Already flagged for a separate investigation. | open — later |
| Snapshot cadence (50 ms) | P3 | Fast front-panel moves can alias between captures; the UI confirms at 20 Hz. Acceptable today; event-driven capture is the refinement. | optional |

### Log hygiene result

- Per-start/stop, rate, session, hw_params, suspend/resume and q48
  instrumentation narration moved to `dev_dbg` (dynamic debug).
  Ring-event and pointer-mismatch narration moved to
  `dev_dbg_ratelimited`; the status-watch and cadence warnings are
  ratelimited. Probe identification and error paths stay.
- Bench check (UCX, reload + playback): two driver lines in the kernel
  log, both ratelimited cadence warnings during the rate transition;
  the previous build emitted a dozen informational lines for the same
  sequence.
- Optional refinement: a settling grace for the cadence guard after a
  rate commit would suppress the transition warnings too.

### Kernel P1 fix result

- Gate stress (UCX ISO, duplex q48 through PipeWire, plus five USB
  reset cycles): 5516 deferrals, `signal_overdue` 0, `delta_zero` 0,
  `hw_ptr_mismatch` 0, `max_pending` 1.
- Hotplug stress: an active hwdep reader survived five reset cycles
  (3.6 MB of events, clean `-ENODEV` at each disconnect, six opens);
  no oops, UBSAN report or warning in the kernel log; the card
  recovered every cycle.
- `make C=1` (sparse): no warnings. checkpatch: 0 errors, 0 warnings.
  `make -C kernel test`: ok.

### Host fix result (PR #193, merged)

- H3 applies commands as model updates plus queued I/O; H4 (the shared
  control-I/O worker) executes all F2 operations in order and owns the
  pacing. The Babyface-classic backend shares the worker through its
  own sink; the orchestration is no longer duplicated.
- Scene batches carry words and vendor ops in one ordered command; the
  mid-scene flush fragmentation (which stretched one scene to ~600 ms
  of device time) is gone.
- C1 and the worker queue are bounded and coalescing; Stop preempts.
- Bench acceptance (UCX, ISO, 59 scene+drag floods in 15 s): longest
  publication freeze **0 ns**, zero new `events_dropped`. Core suite
  314 passed, GUI suite 447 passed, including the inverted regression
  tests (prompt live writes on UC and UCX, stop preemption, coalescing,
  backpressure).
- Remaining waits, recorded as open items in the driver design: a
  control read during a large scene apply waits for the remaining
  pacing (drain-pumping while a read reply is pending is the
  refinement); sustained overload beyond the device pace applies
  backpressure to the service thread; the burst-pacing gap is still a
  fixed sleep on the worker.

### Recommended order

1. Host P1 cluster: done (PR #193).
2. Kernel P1 races: done (this PR).
3. P2 items as scheduled; P3 as cleanup.

## Wire-format decision (verified)

The wire format is keyed to the transport as a driver policy decision:
interrupt personalities carry S32 (the Windows reference pairing), ISO
personalities carry float32 (the macOS reference pairing). The device
accepts either format when the `0x16` channel-init value matches; the
init value derives from the resolved wire format for every model.
`FFUC_QUIRK_INT32_SAMPLES` is removed.

Bench evidence (UCX firmware 46, IF5, 48 kHz):

- float32 wire on IF5 distorts ("not a sine"): the float bit patterns
  decode as low-level garbage under S32.
- S32 wire with the matching `0x16=0` init plays with zero transport
  errors; capture delivers a full clean stream (silent inputs at
  −84 dBFS peak).

## UCX IF5 mode setup (verified)

The interrupt personality requires the Windows mode-setup block at
connect (E2b replug capture, `win-20261027-ucx-transport` on the NAS):
60-slot `0x16` init, the DDS/clock quad, the gain-ref word, and the
settings group with a measured 100 ms settle. The reference host
streams 1x on alt 1.

With the previous Mac-style 36-slot init, alt 1 was silent at the DAC
and capture completed zero URBs. With the authentic block, playback and
capture both work on alt 1. The UCX rate write is the family word alone
on both transports; the multiplier follows the alternate setting.

Implemented in PR #191 (`fix/driver-meter-buffer`). Verified on the
bench: clean sine on IF5, a clean IF5 music session (q1024, zero health
deltas), and a clean ISO music regression (44.1 kHz, q48). The UC PC
S32 regression pass is pending; it needs the UC on the bench.

## Open protocol items

Listed with scope and deciding evidence in
`docs/spec/DRIVER-DESIGN.md` §12 and
`docs/spec/FIREFFACE-UCX-PROTOCOL-SPEC.md` §12.4. The UCX IF5 items:
2x/4x alternate selection (needs a 96/192 kHz reference capture), DDS
quad rate dependence beyond the 44.1↔48 family (needs a 32 kHz-family
connect capture), settings-group bit semantics, and the UC PC 60-slot
init alignment.