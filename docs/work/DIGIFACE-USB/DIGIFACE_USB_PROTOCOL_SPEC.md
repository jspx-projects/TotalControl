# Digiface USB protocol

Working note, not part of the specification. Sources are listed in
[`SOURCES.md`](SOURCES.md). Claims are marked verified or prior art. Prior art
is a public implementation, not a capture made in this repository.

## 1. Terms

| Term | Address or shape | Definition |
|---|---|---|
| normal mode | vendor-specific USB declaration | The mode that contains the TotalMix mixer. |
| CC mode | class-compliant declaration | The mode that gives audio and clock control through the class driver. |
| mixer interface | USB interface 1 | The vendor interface that carries mixer and meter traffic. |
| control register 1 | request 16, 16-bit bits and mask | Clock, rate and mixer-enable bits. |
| control register 2 | request 18, 16-bit bits and mask | Output format and further configuration bits. |
| status block | request 17, 16 bytes | Four little-endian 32-bit words of device state. |
| node | 0–2047 | One crosspoint in the mixer matrix. |
| matrix | source × output | The set of active nodes. |
| gain word | 16 bits | Encoded gain for a node or an output fader. |
| meter frame | 1024 bytes | One frame of RMS and peak values for one bus. |
| speed mode | 2, 3 or 4 | 1x, 2x or 4x sample-rate family. |

## 2. Modes

Verified from the vendor manual:

- The unit is switched between normal mode and CC mode with the firmware
  update tool. The option is `CC Mode Active`.
- CC mode defaults to internal clock and ADAT output format.
- In CC mode the manual's Linux answer sets the clock source through the ALSA
  mixer: Internal or Optical 1–4.
- TotalMix FX is the mixer in normal mode. It offers routing, faders, mutes,
  pans, groups, snapshots and loopback.
- The model has no FX. The manual marks reverb and echo as not available.

RME's iPad application reaches the mixer over USB. That shows the mixer runs
and answers in CC mode. It says nothing about the control interface being
available to a third-party host.

Two control models differ:

| Model | Who owns the protocol | Third-party host |
|---|---|---|
| Native: a class-USB control interface | the USB interface, described by the device's own descriptors | can capture it and drive it, as this document does for normal mode |
| Vendor: a private application protocol over a class transport, here USB MIDI | RME | captures the application traffic to learn it |

The vendor model is the one RME uses for the iPad. An interface may therefore be
present in CC mode and still carry no control interface a host can simply use.

CC mode has no documented mixer-control interface. Before the backend is
committed to normal mode, one capture decides whether a control interface
exists there: the CC-mode descriptor tree of §12. Until that capture, normal
mode is the mode with a known protocol.

RME states for the Digiface USB that TotalMix FX for iPad support is planned.
The same statement holds for the AES, whose manual describes the CC-mode routing
as *very similar to normal operation with TotalMix FX*, with the mixer state
loaded from the unit's setups.

## 3. Mainline host behaviour

Verified from `sound/usb/quirks-table.h` and `sound/usb/quirks.c`:

- Two product IDs are matched. The values live in the mainline quirk table and
  are deliberately not repeated here.
- The match names interface class vendor-specific and interface number 0, so
  the class driver claims only the audio interface.
- Three audio format sets are declared, one per speed family.
- A boot quirk runs at probe and writes a fixed state: mixer disabled,
  internal clock, all outputs ADAT, 48 kHz, unity output gain, loopback off.
- The quirk exposes audio only, and leaves the mixer interface to a vendor
  driver.

The boot sequence, as request, value and index:

| Request | Value | Index | Effect |
|---|---|---|---|
| 16 | `0x2410` | `0x7fff` | Internal clock, 48 kHz, 1x, mixer off |
| 18 | `0x0104` | `0xffff` | Mixer off plus the unknown bit 2 |
| 22 | `0x0400` | channel | Loopback off, per the kernel comment |
| 21 | `0x9000` | `0x100 + channel` | Unity gain on every output |

The value `0x2410` decodes on the prior-art register model as rate index 2
(48 kHz), speed mode 2 (1x) and the mixer-off bit. That agreement supports the
register layout. It is not a capture.

## 4. Mixer interface

From public prior art. Not verified on a unit in this repository.

| Item | Value |
|---|---|
| USB interface | 1 |
| Bulk OUT, mixer commands | `0x0b` |
| Bulk IN, meters | `0x84` |
| Interrupt IN | `0x83`, 8-byte packets, purpose unknown |
| Vendor control requests | 16, 17, 18, 19, 20, 21, 22 |
| Command word | 32-bit little-endian |
| Meter frame | 1024 bytes |
| Maximum nodes | 2048 |

The audio interface stays with `snd-usb-audio`. The mixer module claims only
interface 1.

The endpoint packet size is open (§12). One implementation sends matrix word
lists in 4 KiB chunks and reads one meter frame per transfer.

## 5. Control transfers

All requests are vendor type, recipient device. Request 17 is a read; the
others are writes.

### 5.1 Control register 1, request 16

`wValue` carries the bits to set, `wIndex` the mask of bits to change.

| Bits | Mask | Meaning |
|---|---|---|
| 0–2 | `0x0007` | Clock source: 0 internal, 1–4 Optical 1–4 |
| 3–6 | `0x0078` | Configured sample-rate index |
| 7–11 | `0x0f80` | Unknown; prior art clears these at init |
| 12–14 | `0x7000` | Speed mode: 2 = 1x, 3 = 2x, 4 = 4x |
| 10 | `0x0400` | Mixer off |

The prior-art initialisation writes value 0 with mask `0x1f87`. That clears
the clock-source bits, bits 7–12 and the low speed bit, and leaves the
rate-index bits 3–6 untouched.

### 5.2 Control register 2, request 18

`wValue` carries the bits, `wIndex` the mask.

| Bit | Mask | Meaning |
|---|---|---|
| 0 | `0x0001` | Optical output 1: 1 = S/PDIF, 0 = ADAT |
| 1 | `0x0002` | Optical output 2 |
| 2 | `0x0004` | Written by the mainline boot quirk; meaning unknown |
| 3 | `0x0008` | Optical output 3 |
| 4 | `0x0010` | Optical output 4 |
| 6 | `0x0040` | Named TMS by prior art; exact meaning unknown |
| 8 | `0x0100` | Mixer off |

The output-format bits are confirmed by the manual's statement that the
setting defines the optical output format. The mainline boot comment says
"all outputs ADAT" and writes zero to bits 0, 1, 3 and 4.

Bit 2 is written as one by the mainline boot quirk. Prior art calls bit 6 TMS,
while the mainline comment also claims TMS off and writes zero to bit 6. The
two statements conflict. Neither bit is implemented before a capture.

### 5.3 Status, request 17

A 16-byte read returns four little-endian 32-bit words.

| Word | Bits | Meaning |
|---|---|---|
| 0 | 0–3 | Input 1–4 lock |
| 0 | 5–8 | Input 1–4 sync |
| 0 | 10–12 | Current clock source; 7 means internal |
| 0 | 13–16 | Input 1–4 format: 1 = S/PDIF |
| 1 | 4 × i | Input i sample-rate index, i = 0–3 |
| 1 | 20–23 | Current sample-rate index |
| 2 | — | Unknown |
| 3 | 0–2 | Selected clock source; 0 means internal |
| 3 | 3–6 | Configured sample-rate index |
| 3 | 12–14 | Speed mode value, 2–4 |
| 3 | 16, 17, 19, 20 | Output 1–4 format: 1 = S/PDIF |
| 3 | 22 | TMS state, inverted |
| 3 | 24 | Mixer off |

Sample-rate index:

| Index | Rate |
|---|---|
| 0 | 32000 |
| 1 | 44100 |
| 2 | 48000 |
| 3 | invalid |
| 4 | 64000 |
| 5 | 88200 |
| 6 | 96000 |
| 7 | invalid |
| 8 | 128000 |
| 9 | 176400 |
| 10 | 192000 |
| 11 | invalid |

The speed mode follows the index: `(index >> 2) + 2`.

### 5.4 Output fader, request 21

- `wValue` is the encoded gain.
- `wIndex` is `0x100 + output channel`.
- Output channels are 0–33 at 1x.

This is the output-bus fader, the third row of TotalMix. It is separate from
the matrix node gains.

### 5.5 Loopback, request 22

The sources disagree.

| Source | Value | Index | Meaning |
|---|---|---|---|
| Mainline boot quirk | `0x0400` | input channel | Disable loopback |
| Prior art | 0 or 1 | `0x100 + input channel` | Loopback off or on |

These cannot describe the same field. One source is wrong, or they address
different registers. Loopback stays unimplemented until a capture shows the
exact write.

### 5.6 Per-node control requests

Prior art also exposes the matrix through control transfers:

| Request | Value | Index | Meaning |
|---|---|---|---|
| 19 | gain word | node | Set node gain |
| 20 | `(dst << 9) \| src` | node | Route node |

The bulk command words are the batched form of the same operations. The
control path suits a single change; the bulk path suits a full sync.

## 6. Mixer matrix

### 6.1 Command words

Bulk OUT `0x0b` accepts a list of 32-bit little-endian words.

| Operation | Word |
|---|---|
| Set node gain | `(node << 16) \| gain` |
| Route node | `0x40000000 \| (node << 16) \| (dst << 9) \| src` |
| Reset node | `0xc000ffff \| (node << 16)` |
| Silence node | `(node << 16)` |

Field ranges:

| Field | Range |
|---|---|
| node | 0–2047 |
| source, input | 0–31 |
| source, playback | `0x100 + 0–33` |
| destination | 0–33 |

A reset node has gain zero. That makes a route safe to send before its gain:
the node stays silent until the gain word arrives.

### 6.2 Gain law

The input is a linear gain, where 1.0 is unity.

```text
value = round(gain * 32768)
value = min(value, 65536)
if value >= 0x4000:
    value = (value >> 3) | 0x8000
```

| Linear gain | Intermediate | Encoded |
|---|---|---|
| 0 | 0 | `0x0000` |
| 0.25 | `0x2000` | `0x2000` |
| 0.5 | `0x4000` | `0x8800` |
| 1.0 | `0x8000` | `0x9000` |
| 2.0 | `0x10000` | `0xa000` |

The maximum is +6 dB. Unity is `0x9000`. The mainline unity write uses
`0x9000`, which confirms the encoding at unity.

The step is coarser above 0.5 linear because of the three-bit shift. That is a
property of the device encoding, not a rounding choice.

### 6.3 Building a matrix

Prior art uses this order:

1. reset every node with `0xc000ffff | (node << 16)` for all 2048 nodes;
2. route a free node, then set its gain;
3. free a node by setting its gain to zero, then reuse it.

A full sync resets all nodes and rebuilds the active crosspoints. An
incremental sync sends only changed gains and routes.

One node is one source-to-one-output connection. A stereo source into a stereo
output pair uses two nodes, one per side. Pan is realised by the pair of node
gains.

### 6.4 Enabling the mixer

The mainline quirk disables the mixer at boot. The mixer module enables it
after claiming interface 1:

1. request 18, value 0, index `0x0100`;
2. request 16, value 0, index `0x0400`.

To disable, write the same masks with the bits set. Do not disable the mixer
on unplug; the device keeps its state.

## 7. Meters

Bulk IN `0x84` returns one 1024-byte frame per transfer. A frame is 128
little-endian 64-bit words.

| Words | Content |
|---|---|
| 0–33 | RMS per channel, mean square × 2^55 |
| 64–80 | Peaks, two channels per word |
| 100–127 | Frame marker, repeated |

Peak extraction: even channel in the low 32 bits, odd channel in the high
32 bits. Full scale is 2^27.

RMS extraction: `sqrt(word / 2^55)`.

Frame marker: both halves equal `0x0ffffffn`, where `n` is the frame type, so
bits 4–27 are all ones.

| Type | Bus |
|---|---|
| 0 | Inputs |
| 1 | Playback |
| 2 | Outputs |

The types cycle. Prior art reads up to six frames to collect all three. The
frame rate is not documented. A capture must record the rate and the idle
behaviour.

The prior-art check requires word 127 and word 100 to be equal and the upper
and lower halves to match.

## 8. Channel topology

Verified channel counts from the mainline quirk table:

| Speed | Inputs | Playback | Outputs |
|---|---|---|---|
| 1x, 32–48 kHz | 32 | 34 | 34 |
| 2x, 64–96 kHz | 16 | 18 | 18 |
| 4x, 128–192 kHz | 8 | 10 | 10 |

At 1x the names follow the four optical ports:

| Channels | Name |
|---|---|
| 0–7 | ADAT 1–8 |
| 8–15 | ADAT 2 |
| 16–23 | ADAT 3 |
| 24–31 | ADAT 4 |
| 32–33 | Phones L/R |

Each optical port can carry S/PDIF instead of ADAT. The S/PDIF pair aliases
the first two channels of that port:

| Port | S/PDIF channels |
|---|---|
| 1 | 1/2 |
| 2 | 9/10 |
| 3 | 17/18 |
| 4 | 25/26 |

Playback channels are named Playback 1–34.

The 2x and 4x channel maps are a hypothesis. ADAT S/MUX reduces each port to
four or two channels, and the Phones pair stays last. The exact numbering at
those rates needs a capture.

## 9. Clock, rate and format

- Clock source is control register 1 bits 0–2.
- Configured rate is bits 3–6.
- Speed mode is bits 12–14.
- Output format is control register 2, one bit per optical output.
- Input format is reported in the status block. Inputs appear to auto-detect
  ADAT or S/PDIF; no input-format write is known.
- The status block reports lock, sync, current clock and current rate per
  input.

The rate can be written through register 1, but the audio stream also sets
it. Whether a mixer write may change the rate while audio streams is an open
question. Do not write the rate until a capture or a hardware test answers it.

## 10. Snapshots and stored state

Unknown. Prior art does not implement snapshots. The mainline driver does not
touch them. The device may store TotalMix snapshots in a vendor request, or
the snapshot may be nothing more than a matrix state.

TotalControl's snapshots are host-side mixer slots and work without device
storage. Device-stored snapshots need a capture of TotalMix storing and
recalling one.

## 11. Evidence summary

| Claim | Evidence |
|---|---|
| Normal mode is vendor-specific | Vendor manual and mainline quirk |
| The mixer answers in CC mode | RME forum: the iPad application reaches it over USB |
| No CC-mode control interface is documented for a third-party host | Vendor documentation and RME forum |
| Mainline claims only interface 0 | Mainline quirk table |
| Boot state is fixed | Mainline boot quirk |
| Mixer interface, endpoints, requests | Prior art |
| Register bit layout | Prior art, partly confirmed by the mainline values |
| Gain law | Prior art, unity confirmed by mainline |
| Meter layout | Prior art |
| Channel counts per speed | Mainline quirk table |
| Loopback write | Conflict; needs a capture |
| Snapshot storage | Unknown |

## 12. Open items

| Status | Item |
|---|---|
| needs-capture | CC mode: the interface classes, the endpoints and whether a control interface exists |
| needs-capture | CC mode: if a MIDI port appears, its messages while the iPad application changes the mixer |
| needs-capture | Interface 1 descriptors and endpoint sizes |
| needs-capture | One write per control, with a known before and after state |
| needs-capture | Loopback on and off |
| needs-capture | Output format and input format changes |
| needs-capture | Clock source and rate change while streaming |
| needs-capture | Snapshot recall and store |
| needs-capture | Meter rate, idle behaviour and frame types |
| needs-capture | Feedback endpoint `0x83` |
| open | Meaning of register 1 bits 7–11 and register 2 bit 2 |
| open | Whether the mixer can be enabled while audio streams |
| open | 2x and 4x channel maps |
