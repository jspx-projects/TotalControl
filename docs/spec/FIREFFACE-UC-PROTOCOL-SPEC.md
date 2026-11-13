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
- [10. Metering](#10-metering)

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
| `0x8b` | interrupt IN on PC, bulk IN on interface 4 for AP | Front-panel encoder events and status-ticker frames |
| `0x85` | bulk IN | 324-byte hardware Peak/RMS meter pages |

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
| `0x1d` | OUT | Stream-open word |
| `0x20` | OUT | Global enable |

### 2.1 Channel initialization

The UC probe writes request `0x16` with `wValue 0x0001` for slots `0..17` and
`32..49` (36 writes).  Slots `18..31` and `50..63` are not written.  The slot
ranges and values are per-device data; they are not derived from the channel
count.

### 2.2 Probe and stream open

The implemented probe sequence is:

| Order | Request | Value |
|---|---|---|
| 1 | `0x16` | `0x0001`, slots `0..17` and `32..49` |
| 2 | `0x10` | `0x0800 / 0xb800` |
| 3 | `0x20` | `0x0001 / 0x0000` |

The initial 48 kHz stream opens directly after this sequence.  Both UC
personalities, PC/interrupt and AP/ISO, use this same known-good sequence.

A rate change closes the stream with `0x13 0x000f/0xc000`, writes the new
`0x10` rate family and the `0x10 / 1487 = 0x0001` commit, waits 200 ms, then
opens with:

| Request | Value / index |
|---|---|
| `0x10` | `0x0000 / 0x8000` |
| `0x1d` | `0x0000 / 0x0000` |
| `0x14` | `0x000f / 0xc000` |

The Windows PC driver probe instead writes 60 `0x16` values of `0x0000` for
slots `0..29` and `32..61`, four `0x1b` DDS words and eleven `0x10` settings
words.  Its WDM stream open adds `0x10 0x8000/0x8000`, `0x1d` and `0x14`.
No nonzero `0x16` restore follows the Windows clear.  Replaying the Windows
clear without the Windows later state produces the wrong rate and distorted
audio; the Linux initial path does not use it, and that later state is not
decoded.  The shipped UC init is the sequence above.

## 3. Matrix and channel words

A mixer cell is addressed by:

```text
wIndex = block * 36 + source
source = 0..17   hardware inputs
source = 18..35  software playback channels
block  = 0..17   hardware output blocks
```

The effective level is the 12-bit `wValue`; `0x2000` is 0 dB.  The two level
LSBs that would occupy `wIndex` bits 14:15 are not used for matrix cells.
Those bits select submix banks for output masters, so a crosspoint write keeps
them clear and writes the plain cell address.

For a stereo pair, source `N` writes the left cell in block `2p` and source
`N + 1` writes the right cell in block `2p + 1`; `p` is the output pair.

The output masters use `wIndex = 992 + block`.  The block order is the
reference mixer's display order: AN1/2 is `0`, and Phones 7/8 is `6`.  The four submix
banks occupy `wIndex` bits 14:15; an output-balance change writes the left and
right masters in each bank.

Input phase uses request `0x18`:

| State | `wValue` | `wIndex` |
|---|---:|---|
| Normal | `0x0156` | `0x4000 + code` |
| Inverted | `0xfea9` | `0xc000 + code` |

The low code is `0x00` for input 1 and `0x24 + (input - 1)` for inputs 2–4,
giving `0x25` for input 2 and `0x27` for input 4.  The value is a signed
coefficient; phase is not a `0x17` bit on the UC.

Hardware-output phase is carried in the `0x12` cells, not in `0x18`.  An
inverted cell writes the bitwise complement of the normal `wValue`; the
`wIndex` address is unchanged.  Toggling output phase rewrites every cell
routed to that output block.  For example, normal `0x16a0` becomes inverted
`0xe95f`, and normal `0x3fd9` becomes inverted `0xc026`.

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

The `0x12` master for block `n` is `992 + n`; the four submix banks are
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

### 4.1 Front-panel events

A `0x8b` interrupt frame is four bytes:

| Bytes | Meaning |
|---|---|
| 2, bits 0–6 | level code (`0..63` for outputs, raw gain for inputs) |
| 2, bit 7 | stereo pair command: the level applies to both sides |
| 3, bits 0–4 | encoder channel `0..21` |
| 3, bit 7 | set on captured front-panel frames |

Byte 3 bit 7 does not make the frame invalid; decode the channel from bits
0–4.  It is set on captured output frames and on captured input frames.  A
status-ticker frame uses channel `31` and carries no encoder value; it is
ignored.

Input gains use channels `0..3`; captured i1 is `9f b2 15 80`, channel `0`,
value `21`, byte 2 bit 7 clear.  This is a mono command and affects only i1.
When byte 2 bit 7 is set the frame is a stereo pair command and the level
applies to both channels of the pair.  The front panel has gain for inputs
1–4 only; inputs 5–18 produce no gain events.  Output levels use channels
`4..21`; captured Phones is `62 e3 2b 8b`, channel `11`, value `43`.

The AP personality reports the same four-byte frames on interface 4, bulk IN
endpoint `0x8b`. The frame layout and channel numbers are unchanged.

### 4.2 Front-panel display

The encoder cycles through these entries:

```text
i.1, i.2, i.3, i.4,
L.1, L.2, L.3, L.4, L.5, L.6,
PH, SP,
A.1, A.2, A.3, A.4, A.5, A.6, A.7, A.8
```

| Entry | Value | Range | Step |
|---|---|---|---|
| `i.1`, `i.2` | input gain | 0 (mute), then 10–65 dB | 1 dB |
| `i.3`, `i.4` | input gain | 0–18 dB | 0.5 dB, dot for half |
| `L.1`–`L.6`, `PH`, `SP`, `A.1`–`A.8` | output level | +6 … −58 dB, then mute | 1 dB |

A long press (more than one second) toggles stereo setup mode; the display
shows `on` or `off`. In stereo mode only the left channel of a pair is shown
(`L.1`, `L.3`, `L.5`, …), and a level change applies to both sides.

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
channel in matrix display order; value `1` feeds that output into the
same-numbered input channel.  The unit ignores a single-index update:
every change writes the complete 30-entry bank, with `0` for every disabled
index.  AN5/6 therefore writes indices `4` and `5`.  The loopback bank is
live and changes the running stream.

The matrix applies live.  A `0x12` cell write takes effect on the running
engine, so a route made while audio plays is audible immediately and a scene
applied while the engine runs takes effect at once.  TotalControl still writes
the complete matrix — 36 sources × 18 blocks plus 18 masters — on connect,
because a fresh device holds no scene.  Loopback is live as well.

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
mutes, loopback, output levels and output balance.  The kernel probe writes
the 36 `0x16` channel slots and the `0x10`/`0x20` words from §2.2.  The host
scene apply then writes 30 loopback entries, one `0x17` switch reset, four
`0x1a` preamps, 18 `0x1a` output levels and 666 `0x12` matrix/master words.

Front-panel gain and output-level changes arrive on `0x8b` and update the
live model.  Byte 3 bit 7 is present on captured output frames and does not
suppress the channel.  The host scene remains the source for the next connect.

## 9. Host-side mixer operations

Trim Gains is an editing mode over a channel's send levels.  A fader move in
trim mode writes the channel's `0x12` send cells directly; the send levels
are the persistent state.

Width and MS Proc are host-computed matrix operations.  The host writes the
resulting cross-feed coefficients as `0x12` cells.

Talkback is a host-side operation.  The UC control stream carries the normal
matrix and output state produced by the host mixer.

## 10. Metering

The `0x85` bulk stream carries 324-byte pages: 162 little-endian 16-bit
words.  The stream alternates a data page and a flag page.

| Page | Marker |
|---|---|
| Data | 32-bit word at byte 4 is `0x00001fff` |
| Flag | 32-bit word at byte 4 is `0` or `1` |

The page kind of one mode carries marker `0x1fff` (see the table above)
on the data page, another carries `0`.  There word 6 (byte 12..13) is the
discriminator: the data page carries a non-zero status word there and the
flag page carries zero.  A reader classifies the page as data when the
marker is `0x1fff` **or** word 6 is non-zero, and as flag otherwise.

The data page carries the Peak and OVR words.  The flag page carries the
software-playback RMS words.  A reader must not apply a flag page to the
Peak bank, and must not apply a data page to the RMS bank: in the modes
that carry marker `0` on both page kinds, reading the data page's record
region at the flag-page word offsets turns input/peak data into huge
phantom RMS values.

### Page map

Data page (either page kind that sets word 6; 4-byte records, two
16-bit words per record):

| Words (bytes) | Contents |
|---|---|
| 0..53 (0..107) | header/status: the page discriminator in word 2 (byte 4) or word 6 (byte 12..13), everything else Unknown.  Word 2 carries the `0x00001fff` marker in the one mode, `0` in the other; where it is `0`, word 6 tells the page kind: non-zero on the data page, zero on the flag page (see the classification rule above) |
| 54..69 (108..139) | AN1..AN8 input records: high halfword = level, low halfword = junk.  AP linear 16-bit RMS; PC 6800 log word |
| 70..89 (140..179) | S/PDIF L/R and ADAT 1..8 input records: bits 8..31 = level, bits 0..7 = overload ratio, `0x00001fff` filler before the first sample.  AP linear 24-bit RMS; PC `0x420000` octave word |
| 90..125 (180..251) | SW playback 1..18 records: OVR word + linear 16-bit Peak |
| 126..161 (252..323) | hardware outputs AN1..8, S/PDIF and ADAT 1..8 records: OVR word + linear 16-bit Peak |

Unknown bytes on the data page: 104 of 324 (the header/status region).

Flag page (the non-data page of the alternation, word 6 reads zero):

| Words (bytes) | Contents |
|---|---|
| 0..71 (0..143) | header/status: Unknown |
| 72..143 (144..287) | SW playback 1..18 RMS bank: four 16-bit words per channel with the power word at slot offset 3 |
| 144..159 (288..319) | hardware output RMS bank: the power words at 147, 151, 155, 159 = AN1, AN2, AN3, AN4; the other twelve words are junk |
| 160..161 (320..323) | truncated head of output channel 4's own slot: Unknown |

Unknown bytes on the flag page: 172 of 324 (the header/status region, the
junk words of the output bank and the truncation tail).

### Playback and output Peak/OVR words

Each data-page stereo pair occupies four 16-bit words:

| Pair words | Contents |
|---|---|
| `w` | first channel OVR |
| `w + 1` | first channel Peak |
| `w + 2` | second channel OVR |
| `w + 3` | second channel Peak |

Playback pair `p` starts at `w = 90 + 4p`.  Hardware-output pair `p` starts
at `w = 126 + 4p`.  Output pair 3 (Main 7/8) is therefore words 138..141,
bytes 276..283.

A Peak word is a linear 16-bit value:

```text
Peak dBFS = 20 × log10(word / 32768)
```

The word clamps at `0x7fff` (0 dBFS): the record is measured upstream of the
row's `0x1a` output fader, so pushing that fader alone into the over band
keeps this word at its ceiling.

The OVR word is two bytes:

| Bits | Contents |
|---|---|
| 15..8 | `0xff` while the channel is at or above full scale, the over event |
| 7..0 | consecutive samples at or above full scale, counted by the device |

The low byte is the device's own consecutive-over-sample count within the
record's update window: it reads 0 on the `0xff00`/`0x0000` no-over
fillers, climbs with the over fraction of the record window (4/8/11/13/15
at sine peaks of +0.05/+0.25/+0.50/+0.75/+1.00 dB), and saturates: the
upper nibble carries the count past 15 until both nibbles read `0xff`
(every window sample over).  Verified against sine and square staircases
in `docs/work/UC-METER-DECODE.md`.  The host LED gate is this count
compared with the global OVR sample preference (default three); the host
runs no sample counting of its own.

### Playback RMS words

The flag page carries playback RMS for pair `p` at words `75 + 8p` (left)
and `79 + 8p` (right).  The word is linear power relative to full-scale
peak squared:

```text
RMS amplitude = sqrt(word / 32768)
RMS dBFS      = 10 × log10(word / 32768)
```

A full-scale sine therefore has word 16384 and reads −3.01 dBFS RMS.
Playback pair 1 uses bytes 150 and 158.  All nine playback pairs were
verified with an 18-channel source: each pair's RMS appears only at
`75 + 8p` and `79 + 8p`.

### Output RMS bank

The flag page carries output RMS in the same per-channel slot shape as the
playback bank, starting at word 144: four words per output channel with the
RMS power word at `147 + 4c`.  Only channels 0..3 fall inside the page
(AN1, AN2, AN3, AN4); word 160/161 is the truncated head of channel 4's own
slot and the page ends before channel 4's level word.

A 120-second solo capture of the `0x85` stream
(`docs/captures/uc/fullstream-monoAN1.raw`, no competing reader) confirms
the bank covers channels 0..3: every non-data page has the same nonzero
word pattern, and output RMS for AN5..ADAT8 appears nowhere in the stream.
While a row is over 0 dBFS the device holds its output RMS word at zero, so
the bank is the row's 0..0 dB meter only; the over band is not a device
RMS feed.  TotalControl's rows beyond AN4 therefore draw the bar from the
documented host-side post-fader estimate, and the over band on every row
comes from the row's own written fader and matrix state when the device
reports the row over.

### OVR trigger

The device supplies the over event and its own consecutive sample count in
the record's OVR word.  The host gates the OVR LED on that count against
the **Full scale samples for OVR** sample preference (`FSCntOVR`), default
three, range 1..10 on the UC USB settings.  No USB control write carries
this setting; it is a host preference.

### Analog input records

The input level law is personality-dependent: the AP personality publishes
linear RMS words, while the PC personality publishes the 6800 (analog) and
`0x420000` (digital) octave words.  Playback and hardware-output records
use the linear 16-bit Peak word in both.

The AP personality carries a linear 16-bit RMS word in the upper halfword
of the data-page record; the lower halfword is packet noise:

```text
u == 0                  -> below range
dBFS = 20 × log10(u / 32768),  u = record >> 16
```

The word is the same linear scale as the playback and output Peak words.
The analog detector settles slowly, so a reading is its hold value, not an
instantaneous level.  The input reference and gain are applied before the
meter.

The PC (interrupt) personality uses the octave scale instead:

```text
u == 0                  -> below range
dBFS = (6800 - u) * 0.12745,  u = record >> 16
```

Records outside the 6740..7550 window are no data in that mode.
The decoder selects the law from the active personality.

### Digital input records

The AP personality carries a linear 24-bit RMS word in bits 8..31; the low
byte is the overload field:

```text
level = record >> 8 & 0xffffff
level == 0              -> below range
dBFS  = 20 × log10(level / 0x800000)
```

The `0x00001fff` filler has a level below the meter's floor and is no data,
not a level.

The PC personality uses the octave word instead:

```text
level = record >> 8 & 0xffffff
dBFS  = -(level - 0x420000) * 6.0206 / 0x8000
```

with `0x7fffff` the saturation record and the over value in the low byte.

### TotalControl behaviour

- Playback lower VU bar: the flag-page RMS bank (all 18 playbacks).
- Hardware output lower VU bar: AN1..AN4 from the flag-page output RMS
  bank; AN5..ADAT8 from the documented host-side post-fader estimate
  (power sum of every routed source's device level times its matrix cell
  gain, scaled by the row's `0x1a` out fader, zero when the row is
  muted).
- Playback and output peak line: the data-page Peak word.
- Playback and output OVR LED: the device's consecutive-over count in the
  record's OVR word, compared with the global OVR sample preference
  (default three).
- Playback and output red band (0..+6 dB): the record's OVR count inverse
  inside the device's resolvable range; beyond it the hardware output
  rows continue on the row's written `0x1a` fader and routed matrix
  state.
- Analog inputs: the data-page input record, decoded on the active
  personality's law (AP linear 16-bit RMS, PC 6800 log).
- Digital inputs: the data-page input record, in record order S/PDIF L/R
  then ADAT 1..8 (AP linear 24-bit RMS, PC `0x420000` octave).
- Ballistics are host side: a 10 dB/s release and a 2 s peak hold.
Input meters measure the physical input before loopback.  A
loopback-enabled output replaces the input's record signal, but does not
change the input meter or the input bank in the `0x85` page.
