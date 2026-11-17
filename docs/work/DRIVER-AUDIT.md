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
- `make -C kernel`, `make -C kernel test`, checkpatch. `sparse` is not
  installed yet; `make C=1` is pending.
- Live context: the UC test unit (firmware 127) passed PC and AP
  streaming in the recent channel-controls and buffer-size PRs. This
  audit added no new hardware captures.

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
| Verbose PCM trigger-stop diagnostics | all | P3 | Unratelimited `dev_info` on every stop: dmesg noise that buries real errors; on a serial console each line stretches the stop path by milliseconds. No effect while streaming. | optional |
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