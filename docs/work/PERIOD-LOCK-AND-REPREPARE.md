# Period lock and re-prepare

Status: implemented on `fix/period-lock-and-reprepare`; desk-verified on the
UCX with PipeWire 1.6.11 and the checkout driver. The specification changes are
in `TOTALCONTROL-GUI-SPEC.md`, `TOTALCONTROL-STATE-SPEC.md`,
`TOTALCONTROL-ARCHITECTURE.md` and `DRIVER-DESIGN.md`.

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
- The cushion is `H = 2·P` and the IRQ ring margin is `period + headroom`.
  While a host stream runs, `P` is the graph's running quantum, not the stored
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
  while a host stream is active; the stored target is the idle value.
- A global quantum force is not issued while a host stream or conductor owns
  the graph; the policy props land at the next natural re-prepare.
- The driver signals each direction at its own boundary when the two ALSA
  periods differ.

## Cases where it can still fail

- The margin is finite. At 1024 the intended `H=2·P` margin is about 69 ms; a
  host stall longer than that still underruns. At 128 it is about 9 ms.
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
