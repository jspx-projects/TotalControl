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

## Certain errors — fixed

Both are fixed in the fix PR that accompanies this note; they are not
open items.

| Error | Status |
|---|---|
| Meter URB coherent buffer allocated `2 * meter_len`, freed `meter_len` in the error path and cleanup | done |
| `ucx_if5` static initializer (checkpatch error) | done |

## Open driver risks

Scope labels: all = every personality; ISO = UC AP, UCX IF0,
Babyface-classic; status = personalities with a status endpoint.

| Item | Scope | Status | Notes |
|---|---|---|---|
| Duplex period gate deferral counters read/incremented without a shared lock | all | open | Hypothesis: concurrent URB completions on different CPUs race the counters. The period signal is under the stream lock; the gate decision is not. KCSAN/lockdep runs and a serialized or atomic gate decide it. |
| hwdep cleanup runs in disconnect and again in `private_free` | all | open | A reader past the disconnected check could race the FIFO free. Order cleanup against hwdep unregistration. |
| ISO continuity log counter is module-global | ISO | open | Shared across devices and endpoints; move to per-endpoint state. |
| hwdep read path allocates a buffer per event | all | optional | Per-reader preallocation removes the allocation from the event path. |
| Verbose PCM trigger-stop diagnostics | all | optional | Move to debugfs, ratelimit, or a module parameter. |
| Buffer poison memsets the PCM buffer on every prepare | all | optional | Diagnostic; make it a module parameter. |
| `COUNTERS.status_urbs` reports zero | status | open | Expose the status URB counter. |
| `INFO.outputs` reports the playback channel count | Babyface-classic | needs-verification | The hardware output count differs; verify the ABI field semantics. |

## Heuristic waits

| Wait | Scope | Status | Notes |
|---|---|---|---|
| Session gaps (250/200/200 ms) | `WIN_SESSION` | open | Device-mandated timing, verified in Windows reference-host captures. A readiness poll needs a capture of a device-visible readiness condition first. |
| Meter/state pump: 4 ms delayed work, 10 ms retry | all | optional | Resubmit from completion where backpressure allows; preserve the UC pair-boundary pause. |
| UCX burst-pacing gap (host) | host, UCX | open | Fixed sleep between `WRITE_WORDS` bursts; replace with bounded flow control or keep as a named quirk. |

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

### Immediate — next PR

| Item | Reason |
|---|---|
| Keep the event drain independent of blocking control work: bound or move the synchronous EP0 calls, flush waits and UC writes | A stalled control path delays event draining and drops events; the UC meter pair phase depends on prompt draining. |
| Bound the command queue with backpressure or coalescing | A drag can queue writes faster than the device accepts them. |
| Make the writer flush non-blocking for the event drain, or drain events while a flush is pending | The flush wait is the remaining path where burst pacing stalls the service thread. |

### Later

| Item | Reason |
|---|---|
| Exclusive meter reader: driver enforcement or explicit ABI contract | A second reader breaks the UC pair phase model. |
| Snapshot cadence for fast front-panel moves | The 50 ms capture can alias fast moves; acceptable today. |
| Connect-policy investigation | Host settings written on connect can overwrite front-panel changes; already flagged separately. |
| Replace UCX burst pacing with bounded flow control | Needs a device-visible drain condition or a capture that bounds the gap. |

## Open protocol items

Listed with scope and deciding evidence in
`docs/spec/DRIVER-DESIGN.md` §12; the owning protocol specifications
carry the detail. Includes the UCX IF5 wire-format question: IF5 keeps
S32 per the documented Windows protocol while every other personality
follows the host-OS rule (float32 on Linux); a float32 loopback test on
IF5 decides whether the exception stays.