# Fireface UC control specification

This specification defines the control protocol and persistent mixer state of
the Fireface UC in normal RME mode.  It is the authoritative reference for
the UC control surface implemented by TotalControl.

## Contents

- [1. Device and control paths](#1-device-and-control-paths)
- [2. EP0 request map](#2-ep0-request-map)
- [3. Matrix and channel words](#3-matrix-and-channel-words)
- [4. Output level and balance](#4-output-level-and-balance)
- [5. Switches and references](#5-switches-and-references)
- [6. Routing controls](#6-routing-controls)
- [7. Sample rate and clock](#7-sample-rate-and-clock)
- [8. State ownership](#8-state-ownership)
- [9. Host-side mixer operations](#9-host-side-mixer-operations)

## 1. Device and control paths

| Personality | USB IDs | Transport |
|---|---|---|
| PC | `0424:3fc5`, `2a39:3fc5`, `2a39:3fce` | interrupt IF0 |
| AP | `0424:3fc6`, `2a39:3fc6` | isochronous IF0 |

The PC personalities differ in the interrupt transport:

| PC product | Sample words | Engine start |
|---|---|---|
| `UC_PC`, `UC_PC_RME` | float32 | alt N; alt reselect only on a rate-band change |
| `UC_WIN_RME` | S32 | alt 0, then alt N, on every start |

On `UC_WIN_RME` the output latch clears only on the alt 0 → alt N reselect.
Every UC engine start follows the previous engine stop by at least
`FFUC_RESTART_SETTLE_MS` (250 ms); an immediate restart leaves `UC_WIN_RME`
silent in both directions.

The UC uses EP0 vendor requests for control. Two additional endpoints carry
live information:

| Endpoint | Direction | Contents |
|---|---|---|
| `0x8b` | interrupt IN | Front-panel encoder events and output-latch state |
| `0x85` | bulk IN | 324-byte transport and link state |

Mixer state is write-only.  The host model is the authoritative copy and is
written to the device on connect and on each change.

## 2. EP0 request map

| Request | Direction | Function |
|---|---|---|
| `0x10` | OUT/IN | Options word, clock source, rate family and rate commit |
| `0x11` | IN | Device state; top byte carries the rate index |
| `0x12` | OUT | Mixer crosspoints, gain/phase values and output masters |
| `0x13` | OUT | Global mute |
| `0x14` | OUT | Global unmute |
| `0x15` | OUT | Loopback bank |
| `0x16` | OUT | Channel initialization |
| `0x17` | OUT/IN | Reference levels, 48 V, PAD, Inst |
| `0x18` | OUT | Input phase |
| `0x1a` | OUT | Input gain and output-channel level |
| `0x1b` | OUT | Rate-generator/pitch coefficients |
| `0x1c` | IN | Firmware revision |
| `0x20` | OUT | Global enable |

### 2.1 Channel initialization

The UC connect sequence writes request `0x16` with `wValue 0x0000` for slots
`0..29` and `32..61` (60 writes).  Slots 30 and 31 are skipped.  The slot
ranges and values are per-device data; they are not derived from the channel
count.

## 3. Matrix and channel words

A mixer cell is addressed by:

```text
wIndex = block * 36 + source
source = 0..17   hardware inputs
source = 18..35  software playback channels
block  = 0..17   hardware output blocks
```

The value is a signed 16-bit level.  A level of `0x2000` is 0 dB.  A non-zero
send sets `wIndex` bit 14; a zero send writes the same cell with bit 14 clear.
Bit 14 is a send-active flag, not a submix selector.

For a stereo pair, source `N` writes the left cell in block `2p` and source
`N + 1` writes the right cell in block `2p + 1`; `p` is the output pair.

The output masters use `wIndex = 992 + block`.  The four submix banks occupy
`wIndex` bits 14:15; an output-balance change writes the left and right
masters in each bank.

Input phase uses request `0x18`:

| State | `wValue` | `wIndex` |
|---|---:|---|
| Normal | `0x0156` | `0x4000 + code` |
| Inverted | `0xfea9` | `0xc000 + code` |

The low code is `0x00` for input 1 and `0x24 + (input - 1)` for inputs 2–4,
giving `0x25` for input 2 and `0x27` for input 4.  The value is a signed
coefficient; phase is not a `0x17` bit on the UC.

## 4. Output level and balance

Hardware output level uses request `0x1a` with the 1 dB code:

```text
code = 6 - dB
0 = +6 dB, 6 = 0 dB, 63 = -57 dB
```

Output channel indices are:

| Output pair | Channels |
|---|---|
| Phones 7/8 | 10, 11 |
| AN1/2 | 4, 5 |
| AN3/4 | 6, 7 |
| AN5/6 | 8, 9 |
| SPDIF | 12, 13 |
| ADAT 1/2 … 7/8 | 14 … 21 |

The `0x12` master for channel `n` is `992 + n`; the four submix banks are
selected by `wIndex` bits 14:15.

An output pair carries balance as two distinct channel levels.  The linked
fader is the pair level: moving it applies one common dB offset to both
channels.  The two channel levels are the persistent state; the balance
position follows from their difference.

A balance change writes:

- the two `0x1a` output codes, and
- the corresponding left/right `0x12` output masters in the four banks.

The mixer represents input and playback pan/balance with the two source
levels for the selected output pair.

## 5. Switches and references

Request `0x17` is a 16-bit register:

| Bits | Function |
|---|---|
| 0, 1 | 48 V on inputs 1, 2 |
| 2, 3 | PAD on inputs 4, 3 |
| 4, 5 | Inst on inputs 4, 3 |
| 6–7 | Line output reference |
| 8–9 | Line input reference |
| 10–11 | Phones reference |

A switch change reads the complete register, changes the target field and
writes the complete 16-bit value with `wIndex 0xf03f`.  A reference-level
change uses `wIndex 0x0fc0` and the same read-modify-write form.

Reference codes:

| Field | Bits | Codes |
|---|---|---|
| Line output | 6–7 | `1` −10 dBV, `2` Hi Gain, `3` +4 dBu |
| Line input | 8–9 | `0` Lo Gain, `1` +4 dBu, `3` −10 dBV |
| Phones | 10–11 | `1` Hi Gain, `2` −10 dBV, `3` +4 dBu |

The reference register has base `0x3000`.  `SPDIF In TMS` is a host-side
option on the UC and does not change this register.

## 6. Routing controls

Loopback uses request `0x15`.  An index in `0..29` addresses one output
channel; value `1` feeds that output into the same-numbered input channel.
The loopback bank is live and changes the running stream.

The matrix is latched when the audio engine starts.  TotalControl writes the
complete matrix — 36 sources × 18 blocks plus 18 masters — before opening
PCM.  Subsequent cell writes update the device's stored state and apply at
the next engine start.  Loopback remains live.

Input gain uses `0x1a` with 1 dB steps for inputs 1/2 and 0.5 dB steps
(codes 0–36) for inputs 3/4.

## 7. Sample rate and clock

The host stream owns the UC sample rate.  TotalControl restores its remembered
engine rate at startup and on reconnect; `Auto` releases the override and
follows the device/graph rate.

Rate selection uses `0x10`, `wIndex 48`:

| Rate | Word |
|---|---|
| 32.0 kHz | `0x0000` |
| 44.1 kHz | `0x0010` |
| 48.0 kHz | `0x0020` |
| 64.0 kHz | `0x0000` |
| 88.2 kHz | `0x0010` |
| 96.0 kHz | `0x0020` |
| 128.0 kHz | `0x0000` |
| 176.4 kHz | `0x0010` |
| 192.0 kHz | `0x0020` |

The word selects the 32 kHz, 44.1 kHz or 48 kHz family; the stream multiplier
follows the active stream.  A selection is followed by
`0x10 / wIndex 1487 = 0x0001`.

The options word at `wIndex 1487` has base `0x0001` and carries clock source
(bits 1–3), word-clock single-speed (bit 6), SPDIF professional (bit 7) and
optical output format (bit 10; set for SPDIF, clear for ADAT).  `SPDIF In TMS`
is host-side and has no bit in this word.

Pitch/varispeed uses request `0x1b` with four DDS words.  Each DDS word is
split as `wValue` = bits 31..16 and the `wIndex` high byte = bits 15..8; the
`wIndex` low byte is the fixed channel/family `0..3`.  At 0% pitch the 32 kHz,
44.1 kHz and 48 kHz words are:

| Base | `wValue` / `wIndex` |
|---|---|
| 32 kHz | `0xc350` / `0x0000` |
| 44.1 kHz | `0x8db9` / `0x2d01` |
| 48 kHz | `0x8235` / `0x5502` |

Pitch scales the DDS value as `base / (1 + pitch / 100)`.  The fourth word
has a fixed low byte `3`; its clock role is not part of the rate selection.

A host rate change closes the current session with
`0x13 / wValue 0x000F / wIndex 0xC000`, writes the new rate word when the
family changes, waits 200 ms, then opens the new session.  The UC keeps its
rate family across a USB reconnect.  A USB reset restores the 1× multiplier.

## 8. State ownership

The device group is read from the unit at connect: input gain, 48 V, PAD,
Inst, reference levels, clock source and optical format.

The mixer group is written from the TotalControl scene: matrix, pans, faders,
mutes, loopback, output levels and output balance.  The connect sequence
writes 60 `0x16` channel slots, four `0x1b` DDS words, 11 `0x10` settings
words, 30 loopback entries, one `0x17` switch reset, four `0x1a` preamps,
18 `0x1a` output levels and 666 `0x12` matrix/master words.

Front-panel gain and output-level changes arrive on `0x8b` and update the
live model.  The host scene remains the source for the next connect.

## 9. Host-side mixer operations

Trim Gains is an editing mode over a channel's send levels.  A fader move in
trim mode writes the channel's `0x12` send cells directly; the send levels
are the persistent state.

Width and MS Proc are host-computed matrix operations.  The host writes the
resulting cross-feed coefficients as `0x12` cells.

Talkback is a host-side operation.  The UC control stream carries the normal
matrix and output state produced by the host mixer.
