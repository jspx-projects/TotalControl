# Period lock and re-prepare

Status: the period pin, the two-direction re-prepare confirmation and the gate
split are desk-verified on the UCX with PipeWire 1.6.11 and the checkout
driver. The cushion stays at H=2P (user decision, 2026-11-26). The measured
limit is recorded below: a production supply stall above 46.7 ms at P=1024 can
still underrun at the minimum fill; that host stall is the open B5 item. The
specification changes are in `TOTALCONTROL-GUI-SPEC.md`,
`TOTALCONTROL-STATE-SPEC.md`, `TOTALCONTROL-ARCHITECTURE.md` and
`DRIVER-DESIGN.md`.

## Model

- `clock.quantum` is the graph **default**. A suspended graph keeps its current
  quantum; a new default applies at a re-prepare.
- The SPA ALSA node keeps its last negotiated period as the writeable
  `api.alsa.period-size` node property. The property is used as
  `default_period_size` on the next open and overrides the graph quantum.
- TotalControl applies the scheduling policy by activating a brief capture
  stream and forcing a quantum bump (`Q-1`) followed by a restore. The bump
  changes the sticky period. A restore that does not re-prepare leaves the
  bumped period (and sometimes only one direction) behind.
- The duplex gate can align two directions only at a common period boundary.
  Unequal periods have none.
- The ALSA ring fill cycles between `H` and `H + P`; the worst-case margin is
  `H`, not `H + P`. `snd_pcm_delay` includes the driver's in-flight queue and
  must not be used as the ring margin.
- While a host stream runs, `P` is the graph's running quantum, not the stored
  target.

## Verified evidence

| Test | Result |
|---|---|
| Direct `force-quantum 1023`, then `0`, with a running 1024 stream | 8 of 8 complete iterations left playback and capture periods and `api.alsa.period-size` at 1023; `clock.quantum` stayed 1024, force 0 |
| Direct ALSA request, 1024/32768 and 128/4096 | granted exactly; the driver does not round |
| Equal periods 128/128, `headroom=256`, plain 25 ms client, 58 s | no new underruns, `ring_min 129`, `gate overdue 0` |
| Unequal periods 127/128, same conditions | about 5.5 underruns/s, `ring_min 1`, `gate overdue` tracking `ring_starved` |
| Direct duplex 1024/1024, 40 ms write stall, margin 1280 (`H=256`) | `out_underruns +1`, `ring_starved 2`, `ring_min 1` |
| Same stall, margin 3072 (`H=2048`) | clean, `ring_min 1264` |
| Direct duplex 127/128, two-period fill, 30 s, before the gate split | `gate deferred 10434`, `overdue 343`, `max_pending 2`; no app XRUN |
| Same cell after the gate split | `gate deferred 0`, `overdue 0`, `max_pending 0`; `ring_min` equal to the 128/128 cell |
| Desk re-prepare 1024, 128, 512, 1024 | playback and capture periods equal the target on every step |

## Falsifiable predictions

- If the bump were transient, the period would return to 1024 after the
  restore. It did not; the residue model holds.
- If 128 were intrinsically too small, equal 128/128 would underrun like
  127/128. It ran 58 s clean; the mismatch model holds.
- If `H=2048` did not add margin, the 40 ms stall would underrun at both
  settings. It underran only at 1280 frames; the margin model holds.
- After the gate split, unequal periods must not defer/overdue. They do not.

## Fix

- The scheduling policy carries `period_size = P` and writes
  `api.alsa.period-size` on both Pro Audio nodes.
- The idle re-prepare pins the target period before and after the bump and
  confirms playback and capture independently.
- `cushion_period` uses the running graph quantum (then the running PCM period)
  while a host stream is active; the stored target is the idle value. The
  cushion multiplier remains H=2P.
- A global quantum force is not issued while a host stream or conductor owns
  the graph; the policy props land at the next natural re-prepare.
- The driver signals each direction at its own boundary when the two ALSA
  periods differ.

## Cases where it can still fail

- The margin is finite. At 1024 the H=2P worst-case margin is 46.7 ms; a host
  stall longer than that at the minimum fill still underruns. The production
  run recorded 54 ms and a 65 ms startup stall. At 128 the worst case is 5.8 ms
  (11.6 ms with H=4P).
- A host that changes its quantum without a re-prepare can leave the PCM at the
  previous period until the next transition; the policy cannot force it without
  overriding the host.
- A client force/lock can outrank the pinned period; the pin must follow the
  host's value, not the stored one.
- The graph quantum probe can be unavailable while PipeWire stalls; the PCM
  period is then the honest fallback, even if it is stale.
- The gate split removes the cross-direction delay but cannot fix a host that
  opens mismatched periods; it only stops the driver from adding a second
  problem.
- A period change at a different rate must be re-derived from that rate's floor;
  a pin carried across a rate change can be below the new floor.
- Not established: the phase and distribution of the production supply stall
  under each cushion; the long-run underrun rate under the new policy; whether
  TotalMix or another control application adds stalls.

## Why playback became 127 while capture stayed 128

The two Pro Audio directions are separate ALSA PCMs, `pcm0p` and `pcm0c`,
opened by separate PipeWire nodes. The graph quantum is common, but the ALSA
period is negotiated per node and cached as `api.alsa.period-size`.

The idle re-prepare forced `Q-1 = 127` and then restored the force to `0`.
Playback re-prepared at the bump and kept 127. Capture did not take the bump:
it either did not re-prepare before the restore or reopened at 128. The two
nodes' cached periods then disagreed.

The driver's period gate needs a common boundary. The playback boundaries are
multiples of 127, the capture boundaries multiples of 128; the first common
one is 16256 frames, about 369 ms away. The gate withholds the playback wake
until the capture boundary, and the two-deferral release fires long before the
common boundary. The wake is therefore late on most cycles, and the PipeWire
ALSA node writes one period only when that wake arrives.

The fix:

- The policy pins `api.alsa.period-size` to the running graph quantum on both
  nodes, so both negotiate the same period.
- The idle re-prepare pins the target before and after the bump and confirms
  playback and capture independently. A failed restore leaves the intended
  period, not the bump.
- TotalControl does not force a global quantum under a host stream, so the bump
  cannot run while a stream owns the graph.
- If a host still opens unequal periods, the driver signals each direction at
  its own boundary, so the gate cannot add a second delay.

Pointers: `DRIVER-DESIGN.md` §4.2 (Period signalling gate) and §14.1.6 (the
invariant list); `TOTALCONTROL-GUI-SPEC.md` (quantum confirmation);
`TOTALCONTROL-ARCHITECTURE.md` §10 (host authority); 
`C1-DYNAMIC-CUSHION-DESIGN.md` (cushion mechanism); this note.


## Cushion matrix (2026-11-26)

Direct duplex, 44.1 kHz, period 1024, equal periods. `target fill` is the
controlled ring fill; the fill cycles between `target - P` (worst case) and
`target` (after a write). Each cell is two trials in each phase; "fail" means
the driver counted an underrun and starved URBs.

| Target fill | H | Worst fill | 50 ms | 60 ms | 70 ms | 80 ms |
|---|---:|---:|---|---|---|---|
| 1280 | 256 | 266 (6.0 ms) | fail | fail | fail | fail |
| 3072 | 2048 | 2058 (46.7 ms) | fail | fail | fail | fail |
| 4096 | 3072 | 3082 (69.9 ms) | pass | pass | pass | fail at the minimum phase |
| 5120 | 4096 | 4105 (93.1 ms) | pass | pass | pass | pass |

The maximum-fill phase of 3072 passes 50 ms and fails 60 ms; the post-stall
wake latency is part of the supply gap. The worst fill is the margin that
matters. H=2P's worst fill (2058 frames, 46.7 ms) failed at 50 ms. H=3P's worst
fill (3082 frames, 69.9 ms) passed 50, 60 and 70 ms and failed 80 ms at the
minimum phase. H=4P's worst fill (4105 frames, 93.1 ms) passed 50-80 ms.

The production run recorded a 54 ms stall earlier and a 65 ms startup stall on
the current build. H=3P (69.9 ms worst) covers both only if they start at or
above the minimum fill; the phase is not established. The 102 ms
`ring_max_stall_ns` in the driver after the matrix is the 80 ms injected stall
plus the probe's post-stall wake, not a host stall.

At 1024, the worst-case margins are 46.7 ms for 2P, 69.9 ms for 3P and 93.1 ms
for 4P. The shipped policy is H=2P; H=3P and H=4P are recorded alternatives,
not shipped. The remaining production supply stall is the B5 item.

## Observed fallback after a transient direct-ALSA owner (open)

A direct probe briefly owned the PCM during the matrix. After the probe, the
running TotalControl stayed on the convenience policy instead of restoring the
IRQ/H=2P policy: period 256, period-num 16, headroom 592, graph 512. That path
ran while the player was resumed and underran (+20 `out_underruns`,
`ring_starved 6`, `gate overdue 91`). A GUI restart restored
period 1024 / period-num 32 / headroom 2048 / IRQ, after which the ring stayed
at `ring_min 1025`, `ring_starved 0`, `gate overdue 0`.

This is the policy re-establishment defect tracked as C13 residual (b), not the
period/rule defect. It is not fixed here. It is recorded because the fallback
is a separate path that can underrun even when the H=2P path is clean.
