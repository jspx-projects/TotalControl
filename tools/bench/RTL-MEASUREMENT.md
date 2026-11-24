# RTL measurement method — operating point and pitfalls

Companion to `fireface-rtl-probe.c` and `fireface-rtl-measure.sh`. Read with
`CHANNEL-NUMBERING.md` (routing) and `docs/spec/DRIVER-DESIGN.md` §4.1.2 (the
latency model).

## What the probe measures

Linked-duplex chirp: playback and capture start on the same device clock. The
chirp is written at a known playback ring position `P`; cross-correlation finds
its capture ring position `C`. The **device span** is `D = C − P`, exact across
reps. It covers the in-flight URB queue + the device output buffer + the
converters + the capture return. It excludes the host ring fill.

The round trip is `RTL = ring fill + D`. Since `snd_pcm_delay = ring fill +
in-flight queue`, the live form is `snd_pcm_delay + (D − in-flight at
calibration)`. This is what the GUI displays; `D` is the per-rate calibration
value.

## The buffers, and why the device span is fill-independent

| Buffer | Set by | Varies with the probe fill? |
|---|---|---|
| ALSA playback ring (`buffer_size`) | the probe | yes — this is the fill |
| in-flight URB queue (`nurbs × URB`) | module params | no |
| lead horizon (`out_lead`) | module params | no |
| device output buffer + converters | the device | no |
| ALSA capture ring | the probe | drains continuously |

The device only sees the in-flight URBs and the lead horizon, both fixed by
driver parameters. So **the device span `D` does not depend on the ALSA ring
fill**. The ring fill is upstream of the in-flight queue; it is the host
write-ahead, not part of `D`. This is why `D` is a stable per-rate calibration
value.

## The operating-point pitfall (measured)

Although `D` is physically fill-independent, the *measurement* is perturbed
when the probe's ring runs near capacity. A fill sweep at 48 kHz with the
buffer fixed at 8 periods:

| held fill (periods) | 1 | 2 | 3 | 4 | 5 | 6 |
|---|---|---|---|---|---|---|
| D (frames) | 229 | 229 | 229 | 229 | 229 | **265** |

Fills 1–5 (all whole multiples of the URB) give the identical, correct span.
At fill 6 the held fill (6–7 periods) presses against the 8-period capacity and
the read/write timing near full shifts the result. **This is a probe artifact,
not a device change.** It is not a "prefill must be a multiple of the quantum"
rule — every fill in the sweep was a whole multiple.

An oversized buffer confounds the same way: a 16-period buffer with a 4-period
fill also read 265, while an 8-period buffer with a 4-period fill reads 229.

## The robust operating point

- Held fill ~2 periods, playback buffer 8 periods (fill well below capacity).
- Poll fast enough to avoid underruns (50 µs); do not inflate the buffer to
  cure an xrun — that moves the operating point.
- 2 s pre-roll before the first chirp so the queue/lead settles; use the
  steady reps.
- Verify the span is constant across reps **and across fills** before trusting
  it as a device constant. A span that moves with the prefill is a measurement
  offset, not a device property.

## UCX start-phase offset (measured pitfall)

The UC AP duplex span at 48 kHz is fill-independent (229 for fills 1–5). The
UCX span, by contrast, moves with the prefill (232 / 184 / 172 / 220 / 184 /
220 for fills 1–6), in multiples of 12 frames (2 ISO packets). This is a
**start-phase offset in the `D = C − P` accounting**: the capture/playback
device-position alignment at trigger depends on the prefill. It is not a
fill-dependent device constant.

Until the capture start alignment is accounted for (read both streams' device
position / `hw_ptr` at trigger and subtract the capture pre-trigger offset), a
UCX span measured at one operating point is not a universal per-rate constant.
Verify the span across fills before trusting it. The UC and UCX
buffering/driving themselves are unaffected — this is a latency-display
measurement gap, not an audio-correctness gap.