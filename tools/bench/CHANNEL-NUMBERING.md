# Fireface channel numbering — read before routing a measurement

The loopback tools route a test signal to a physical output. Three different
numbering schemes are in play, and mixing them silently routes the signal to
the wrong port (the symptom is "NO SIGNAL" on the expected input).

## 1. Sends are PAIR-indexed; output mute/volume is MONO-indexed

`RmeDevice::set_volume(ChannelId::Input(i) | Playback(i), output, level)` takes
`output` as an **output-pair index** (`0..PAIRS`, `PAIRS = 9`). It is validated
against `PAIRS`, not the 18 mono channels.

`set_volume(ChannelId::Output(idx), ..)` / `mute(Output(idx))` take a **mono
index** into `outputs()` (`0..18`).

So a route that resolves a mono output index must divide by two before passing
it to an input/playback send:

```text
pair = mono_index / 2
set_volume(Playback(i), pair, level)   // pair index
set_mute(Output(mono_index), false)    // mono index
```

Passing the mono index straight to a send routes to the wrong pair. Example:
AN5 is mono index 4 on the UC; passing `4` to a send targets **pair 4 = SPDIF**,
not pair 2 = AN5/6.

## 2. Internal order vs device DSP order (Phones-first inheritance)

TotalControl's channel model uses a **Phones-first** ordering. The device DSP
is **AN-first**. The bridge is `Fireface::matrix_blocks_for_pair(pair)`:

- **UCX:** maps the internal (Phones-first) pair through `ucx::output_pair_dsp`
  to the AN-first DSP blocks.
- **UC:** uses `pair * 2` directly (its `outputs()` row is AN-first:
  AN1=0, AN2=1, …, AN5=4, AN6=5, Phones=6/7, …).

`outputs()` names are the stable topology key; indices are local to the current
profile order. Resolve a port **by name** (e.g. `"AN5"`), never by a hardcoded
index copied from another device.

## Practical rule for the measurement

The cable is on a physical port pair (e.g. out 5 → in 5 = AN5). Anchor on the
physical port label, not a display position. Route with
`fireface-scene-ctl route "AN5" <level>`: it resolves the name to the mono index,
derives the pair for the sends, and unmutes/raises both sides of the pair. The
capture channel is the PCM index of the matching input (AN5 = channel 4 on UC
and UCX). Verify signal on the expected input before trusting a latency number.