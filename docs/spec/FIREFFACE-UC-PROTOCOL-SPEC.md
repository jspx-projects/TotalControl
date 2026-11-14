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
- [11. Device model and USB transport](#11-device-model-and-usb-transport)
- [12. Code map](#12-code-map)

## 1. Device and control paths

| Personality | USB IDs | Transport |
|---|---|---|
| PC | `0424:3fc5`, `2a39:3fc5`, `2a39:3fce` | interrupt IF0 |
| AP | `0424:3fc6`, `2a39:3fc6` | isochronous IF0 |

The PC personalities differ in the interrupt transport:

| PC product | Sample words | Engine start |
|---|---|---|
| `UC_PC`, `UC_PC_RME` | float32 | alt N; alt reselect only on a rate-band change |
| `UC_WIN_RME`, firmware 138 and later | S32 | alt 0, then alt N, on every start |
| `UC_WIN_RME`, firmware below 138 | float32 | alt 0, then alt N, on every start |

The `UC_WIN_RME` descriptor selects S32 from firmware 138.  Below 138 it
carries float32.  An unread firmware follows the descriptor.  Every other
UC descriptor uses the format in the table.

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
U16 words (U16 word N = bytes 2N..2N+2).  The pages strictly alternate in
a **meter heartbeat**: one peak page followed by one RMS page, repeating
at roughly 190 pages per second.  Page identity is positional, not
byte-encoded: the host knows which half of the heartbeat a page is by
its place in the transfer order, not by reading one of its fields.  Each
U32 field is little-endian unless stated otherwise.

| Term | Definition |
|---|---|
| **Meter heartbeat** | Two consecutive pages of different role: one peak page and one RMS page, in that order. |
| **Peak page** | The heartbeat half whose record table carries the peak/OVR meter records. |
| **RMS page** | The heartbeat half carrying the RMS banks, or its idle-state words when the engine publishes nothing. |
| **Beat words** | Bytes 4..8 of every page, read as a U32: one U16 inside the peak page's header and one U16 inside the RMS page's input bank. Their values are engine fields; their format is session-dependent; they carry no identity. |
| **Record table** | Bytes 108..323 (U32 words 54..161): 54 × U32 meter records. Both halves carry this address range; only the peak half's record table is meter content. |
| **Meter record** | One U32 in the record table; layout and law per record class below. |
| **RMS banks** | The power-word banks of the RMS page: input bank, playback bank, output bank. |
| **No-update sentinel** | `0x1fff`. In a meter record: the channel's detector did not refresh. In an RMS bank word: no value published. Never audio level data. |
| **Header** | Bytes 0..107 of every page: seven 16-byte groups at byte offsets 0, 16, 32, 48, 64, 80, 96; the last group is truncated after its second value slot at byte 104. |
| **Value slot / status slot** | The two U32 positions of a header group: +0 and +8 are value slots, +4 and +12 are status slots. On the peak page the header carries output RMS groups and their status. On the RMS page the same range spans the input bank's slots. |
| **Peak word** | The level halfword of a meter record (16-bit analog/playback/output; 24-bit in bits 8..31 for digital). Linear scale, `0x8000`/`0x800000` full scale. |
| **OVR word** | The over halfword (low byte/nibble) of a meter record: the `0xff` event byte plus the device's consecutive full-scale-sample count; carries no level data. |
| **Heartbeat event** | One driver event carrying a complete heartbeat: the RMS page, then the peak page. The event flag `METER_PAIR` says the payload is a pair. |
| **Phase anchor** | The point at which the page order is known: device power-up starts with the RMS half. |

### Heartbeat synchronisation

1. The device emits the peak page and the RMS page strictly alternately.
   Role is positional: a page is never classified by any of its fields.
2. The device advances its page order only when a transfer completes
   successfully. A page that is not polled waits indefinitely; a pause
   between the two halves of up to one second does not restart the pair.
3. The device has no idle timeout and no restart request. Phase advances
   by exactly one per consumed page, so the phase at any moment is the
   parity of consumption since device power-up.
4. Device power-up anchors the order: the first page is the RMS half.
   A USB reset without power loss does not anchor reliably, so it does
   not count as an anchor.
5. The driver consumes pages in pairs and emits one heartbeat event per
   pair: RMS half first, peak half second. Userspace never sees a single
   page on a UC device. The event flag `METER_PAIR` marks the payload.
6. A normal transport stop completes a held half before stopping, so the
   phase survives reader attachment and detachment.
7. If the phase is not anchored for a session (a fresh probe while the
   unit kept running, or a failed stop drain), the driver orders the
   first pair by the record-table shape: the peak half publishes the
   eight AN records, the RMS half does not. If that check cannot decide,
   the driver publishes the pair in arrival order and keeps the
   `PHASE_LOST` flag set. It retries on every following pair; a decidable
   pair fixes the order and clears the flag. The flag marks the roles as
   unverified.  The peak half carries nonzero AN record words even when
   the analog inputs are silent, so the cue decided the first pair in
   every tested session; the fallback is the safety net for a state where
   both halves read zero.
8. Only one meter reader may drain the stream at a time. Concurrent
   readers each receive a sample of pages and break the phase model.

### Driver feed

1. Meter transfer is reader-triggered: with no reader the driver submits
   no meter transfers and the device sends nothing.
2. The pump consumes in pairs. The driver emits one event per complete
   heartbeat and never exposes a single page of a UC heartbeat.
3. The feed is backpressured: the driver submits the first page of the
   next pair only while the event FIFO is below its high-water mark.
   When the FIFO is full it stops at a pair boundary; the device then
   holds its next page.
4. A reader read that drains the FIFO below the low-water mark resumes
   the pump at the next pair boundary.
5. The last reader closing stops the pump at a pair boundary and drops
   unread meter events, so the next reader starts from current data.
6. The high-water mark covers one slow consumer period: sixteen
   heartbeats. The service consumer normally keeps occupancy near one
   pair, so the mark only matters when the consumer stalls.

The heartbeat pairing is a property of the UC model. The UCX and
Babyface-classic meter packets are self-describing and pass through
unchanged; the `METER_PAIR` flag distinguishes the two feeds.

### Page map

Peak page (4-byte records, two 16-bit words per record):

| Words (bytes) | Contents |
|---|---|
| 0..53 (0..107) | header: seven 16-byte output-pair RMS groups at bytes 0, 16, 32, 48, 64, 80 and 96.  Each group is `+0` left RMS power (U32), `+4` left status, `+8` right RMS power (U32), `+12` right status; the last group ends after its right RMS word at byte 104.  The header's value-slot publication is engine-state dependent; see "Output RMS bank" |
| 54..69 (108..139) | AN1..AN8 input records: high halfword = linear 16-bit peak, low halfword = OVR word when over |
| 70..89 (140..179) | S/PDIF L/R and ADAT 1..8 input records: bits 8..31 = linear 24-bit peak, bits 0..7 = full-scale-sample count, `0x00001fff` sentinel before the first sample |
| 90..125 (180..251) | SW playback 1..18 records: OVR word + linear 16-bit peak |
| 126..161 (252..323) | hardware outputs AN1..8, S/PDIF and ADAT 1..8 records: OVR word + linear 16-bit peak |

Unknown bytes on the peak page: 48 of 324.  The 52 status bytes are the
two status words of the six full output-pair groups plus the last group's
left status word; four bytes are engine fields of header group 0 and the
remaining 48 are status/filler.

RMS page:

| Words (bytes) | Contents |
|---|---|
| 0..71 (0..143) | input RMS bank: four 16-bit words per input, power word at slot offset 3 (`3 + 4c` for input `c`); the other three words are slot-internal engine fields, not meters |
| 72..143 (144..287) | SW playback 1..18 RMS bank: four 16-bit words per channel with the power word at slot offset 3 |
| 144..159 (288..319) | hardware output RMS bank: the power words at 147, 151, 155, 159 = AN1, AN2, AN3, AN4; the other twelve words are slot-internal engine fields |
| 160..161 (320..323) | truncated head of output channel 4's own slot: Unknown |

Unknown bytes on the RMS page: the slot-internal engine fields inside the
RMS banks and the truncation tail.  The seven header groups' value slots
carry output levels only when the engine publishes them (see "Output RMS
bank"); the status slots and the RMS page's engine fields never carry
meter data.

### Playback and output Peak/OVR words

Each peak-page stereo pair occupies four 16-bit words:

| Pair words | Contents |
|---|---|
| `w` | first channel OVR |
| `w + 1` | first channel Peak |
| `w + 2` | second channel OVR |
| `w + 3` | second channel Peak |

Playback pair `p` starts at `w = 90 + 4p`.  Hardware-output pair `p` starts
at `w = 126 + 4p`.  Output block `p` occupies channels `2p+1` and `2p+2` in
display order; a left-only signal appears on `2p+1`.  Output pair 3
(Main 7/8) is therefore words 138..141, bytes 276..283.

A peak word is a linear 16-bit value:

```text
Peak dBFS = 20 × log10(word / 32768)
```

The word clamps at `0x7fff` (0 dBFS).  The peak word follows the row's `0x1a`
output fader: lowering the fader lowers the word.  The OVR word is separate.

The OVR word is two bytes:

| Bits | Contents |
|---|---|
| 15..8 | `0xff` while the channel is at or above full scale, the over event |
| 7..0 | consecutive samples at or above full scale, counted by the device |

The count is the device's own consecutive-over-sample count within the
record's update window: it reads 0 on the `0xff00`/`0x0000` no-over
fillers, climbs with the over fraction of the record window (4/8/11/13/15
at sine peaks of +0.05/+0.25/+0.50/+0.75/+1.00 dB), and saturates: the
upper nibble carries the count past 15 until both nibbles read `0xff`
(every window sample over).  Verified against sine and square staircases
in `docs/work/UC-METER-DECODE.md`.  The host LED gate is this count
compared with the global OVR sample preference (default three); the host
runs no sample counting of its own.

### Playback RMS words

The RMS page carries playback RMS for pair `p` at words `75 + 8p` (left)
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

The RMS page carries output RMS for channels 0..3 (AN1..AN4) in the same
per-channel slot shape as the playback bank, starting at word 144: four
words per output channel with the RMS power word at `147 + 4c`.  Word
160/161 is the truncated head of channel 4's own slot.

The peak page header carries the remaining output channels 4..17
(AN5/6 through ADAT7/8) in seven 16-byte groups.  The left power word is at
group offset 0 and the right power word at group offset 8; the words at
offsets 4 and 12 are status.  The groups map to output pairs in display
order.  This header publication is engine-state dependent, not
page-kind-dependent: in firmware states where the header publishes, its
value slots carried outgoing output levels page-consistent with the same
channels' peak/OVR records; in other states the value slots are zero or
DSP status, and the status slots always only carry DSP status.  Status
slots are never level data.

| Group byte | Output pair |
|---:|---|
| 0 | AN5/6 |
| 16 | AN7/8 |
| 32 | S/PDIF L/R |
| 48 | ADAT 1/2 |
| 64 | ADAT 3/4 |
| 80 | ADAT 5/6 |
| 96 | ADAT 7/8 |

The last group is truncated after its right RMS word; the header ends at
byte 107.  A header word is linear power with `0x80000000` full-scale peak
squared:

```text
RMS amplitude = sqrt(word / 0x80000000)
RMS dBFS      = 10 × log10(word / 0x80000000)
```

A full-scale sine has word `0x40000000` and reads −3.01 dBFS RMS.  The
header and RMS-page output RMS words follow the row's `0x1a` output fader.
TotalControl writes the fader to the device and does not apply it again.
The over band comes from the output record's OVR word.  A header RMS word
whose amplitude exceeds the matching output peak word is status, not a
level; the host ignores it and keeps the previous bar.

### OVR trigger

The device supplies the over event and its own full-scale-sample count.
Analog inputs, playback and hardware outputs carry the count in the OVR
word's low byte with a `0xff` event byte; digital inputs carry the count
in the low nibble when the 24-bit level saturates.  The host gates
the OVR LED on that count against the **Full scale samples for OVR**
sample preference (`FSCntOVR`), default three, range 1..10 on the UC USB
settings.  No USB control write carries this setting; it is a host
preference.

### Analog input records

The input peak law is linear in both personalities.  The analog inputs
carry a linear 16-bit peak in the upper halfword of the peak-page record;
the digital inputs carry a linear 24-bit peak in bits 8..31.  Playback and
hardware-output records use the linear 16-bit peak word as well.

For an analog input the lower halfword is the OVR word when the input is
over: high byte `0xff`, low byte the device's consecutive-over count, with
`0xff00` the no-over filler:

```text
u == 0                  -> below range
dBFS = 20 × log10(u / 32768),  u = record >> 16
```

The word is the same linear scale as the playback and output peak words.
The analog detector settles slowly, so a reading is its hold value, not an
instantaneous level.  The input reference and gain are applied before the
meter.

### Digital input records

The digital input carries a linear 24-bit peak in bits 8..31; the low byte
is a full-scale-sample count:

```text
level = record >> 8 & 0xffffff
level == 0              -> below range
dBFS  = 20 × log10(level / 0x800000)
over  = record & 0x0f
```

The low nibble is a full-scale-sample count (0..15).  A controlled
S/PDIF loop with 1, 2, 4, 8 and 12 full-scale samples per 48-sample
period read `0x01`, `0x02`, `0x04`, `0x08` and `0x0c`; repeating the
widths at a 96-sample period gave the same low nibbles.  The low nibble
saturates at 15.  The upper nibble is a status/overflow field; its packing
is not yet clear.  The 24-bit level saturates at `0x7fffff`, and the over
LED uses the low-nibble count against the global **Full scale samples for
OVR** preference.  The `0x00001fff` no-update sentinel has a level below
the meter's floor and is no data.

### Input RMS words

The RMS page carries an input RMS power word at `3 + 4c` for input `c`
(`0..17`), on the same scale as the playback and output RMS banks:

```text
RMS amplitude = sqrt(word / 32768)
RMS dBFS      = 10 × log10(word / 32768)
```

The other three words in each input slot are slot-internal engine fields
and are not meters.  Their values drift within a session and do not track
the input signal (first-difference correlation against a known input peak
is zero); their semantics are an open item.

The data-page peak and flag-page RMS input words are taken after the
channel gain.  TotalControl uses them directly and does not add the gain
again.  The RMS word is one power LSB, so a very low signal can read
zero; the bar then uses the peak word's sine equivalent
(`peak / sqrt(2)`).

### TotalControl behaviour

- Playback lower VU bar: the RMS-page playback bank (all 18 playbacks).
- Hardware output lower VU bar: the RMS-page output bank for AN1..AN4 and
  the peak-page header for AN5..ADAT8, where the engine publishes it.
  The device words are post-fader; TotalControl writes the row's `0x1a`
  output fader and does not apply it again.  It applies the row mute for
  the display.
- Playback and output peak line: the peak-page peak word.
- Playback and output OVR LED: the device's consecutive-over count in the
  record's OVR word, compared with the global OVR sample preference
  (default three).  The peak-page output records carry OVR for all 18
  outputs.
- Playback and output red band (0..+6 dB): the record's OVR count inverse
  inside the device's resolvable range; beyond it the hardware output
  rows continue on the row's written `0x1a` fader and routed matrix
  state.
- Input lower VU bar: the RMS-page input bank (`3 + 4c`).  When the power
  word is zero for a very low signal, the bar uses the peak word's sine
  equivalent (`peak / sqrt(2)`).
- Input peak line: the peak-page input peak word (linear 16-bit analog /
  24-bit digital).
- Input OVR LED: the analog low-word over event, gated by the global OVR
  sample preference; the digital full-scale-sample count, gated by the
  same preference.
- Ballistics are host side: a 10 dB/s release and a 2 s peak hold.
Input meters measure the physical input before loopback.  A
loopback-enabled output replaces the input's record signal, but does not
change the input meter or the input bank in the `0x85` page.

The input, playback and output RMS banks publish per channel when the
channel carries signal.  A zero power word is a level below one power LSB,
or silence.  The input path uses the peak/√2 fallback at that floor.  The
header value slots publish output RMS when the output carries signal; a
word above the matching output peak is status and is ignored.

### Open metering items

1. The beat words' (bytes 4..8) semantics.  Observed formats include a
   steady `0x00001fff`/zero pair, two independent counter/timestamp
   pairs, and an all-zero state; one session changed its format
   mid-stream.  No session-level value carries page identity, and the
   words have no role in synchronisation.
2. Pair ordering under the UC's Mac (ISO) personality.  Alternation is
   verified in that mode; the RMS-then-peak pair order is verified live
   in the interrupt personality and is carried by the same DSP firmware.
3. Full inventory of the header's status-slot values and of the RMS
   banks' slot-internal engine fields.  These fields drift within a
   session and do not track the input signal under first-difference
   correlation, but no law has been published for them.
4. `0x1fff` appears in three roles: as the no-update sentinel in meter
   records and RMS bank power words, and as a beat-word value in some
   sessions.  Whether the DSP uses one code for all three roles on
   purpose is unknown; the roles are distinct field positions.

## 11. Device model and USB transport

This section defines the device model and the USB transport that the earlier
sections use.

### 11.1 Terms

| Term | Address or shape | Meaning |
|---|---|---|
| Personality | — | The USB variant: PC with interrupt audio, or AP with isochronous audio. |
| Interface | number | A numbered USB interface in the active configuration. |
| Alternate setting | number | A numbered setting of one interface; it selects an audio packet size. |
| Endpoint | address and direction | A USB endpoint address and direction. |
| Packet | — | One USB transfer unit on an endpoint. |
| Sample frame | — | One sample for every active channel at one instant. |
| Sample word | 4 bytes | One 32-bit little-endian audio value. |
| URB | — | A USB request block; one host transfer. |
| Meter page | 324 bytes | One meter packet. |
| Meter heartbeat | two pages | The RMS page followed by the peak page. |
| Status frame | 4 bytes | The frame on the status endpoint. |
| Control request | EP0 | A vendor request. |
| Profile | — | The `uc_pc` or `uc_ap` entry in the profile table. |
| Playback endpoint 0 | OUT `0x01` | Carries software playback sample frames at every rate class. |
| Playback endpoint 1 | OUT `0x03`, IF0 alt 3 | Carries the second half of the playback words at 4x on the PC personality. |
| Capture endpoint 0 | IN `0x82` | Carries input sample frames at every rate class. |
| Capture endpoint 1 | IN `0x84`, IF0 alt 3 | Carries the second half of the input words at 4x on the PC personality. |
| Meter endpoint | IF1 IN `0x85` | Carries meter pages. |
| Status endpoint | IF5 IN `0x8b` on PC; IF4 IN `0x8b` on AP | Carries status frames. |

### 11.2 Device model

| Property | Value |
|---|---|
| Model | `uc` |
| Profiles | `uc_pc` (interrupt), `uc_ap` (isochronous) |
| Logical inputs | 18 |
| Logical outputs | 18 |
| Software playback channels | 18 |
| 1x rates | 32, 44.1, 48 kHz |
| 2x rates | 64, 88.2, 96 kHz |
| 4x rates | 128, 176.4, 192 kHz |
| ALSA PCM channels | 18 at every rate |
| Maximum rate | 192 kHz |
| Meter page | 324 bytes |
| Connection mode | vendor only |

The profile table in
[`TOTALCONTROL-DEVICE-PROFILE-SPEC.md`](TOTALCONTROL-DEVICE-PROFILE-SPEC.md)
selects one profile for each connection.  This document owns the wire formats,
registers and transport sequences.  The profile owns channel names, order,
ranges and operations.  The front panel is in "Front-panel events" and
"Front-panel display".

| Profile | Transport | Audio interface | Meter endpoint |
|---|---|---|---|
| `uc_pc` | interrupt | IF0 | IF1 IN `0x85` |
| `uc_ap` | isochronous | IF0 | IF1 IN `0x85` |

### 11.3 PC configuration

The PC personality enumerates as one vendor-specific configuration with six
interfaces.

| Property | Value |
|---|---|
| Device class | vendor-specific |
| Interface class | vendor-specific |
| Configurations | 1 |
| Interfaces | 6 |
| EP0 maximum packet | 64 bytes |
| Power | self-powered, 100 mA |

The interface map is:

| Interface | Alt | Purpose | Endpoints |
|---|---|---|---|
| 0 | 0 | idle | `0x01` OUT interrupt, 64; `0x82` IN interrupt, 64; `0x03` OUT bulk, 512; `0x84` IN bulk, 512 |
| 0 | 1 | 1x audio | `0x01` OUT interrupt, 576; `0x82` IN interrupt, 576; `0x03` OUT bulk, 512; `0x84` IN bulk, 512 |
| 0 | 2 | 2x audio | `0x01` OUT interrupt, 896; `0x82` IN interrupt, 896; `0x03` OUT bulk, 512; `0x84` IN bulk, 512 |
| 0 | 3 | 4x audio | `0x01` OUT interrupt, 768; `0x82` IN interrupt, 768; `0x03` OUT interrupt, 768; `0x84` IN interrupt, 768 |
| 1 | 0 | meter | `0x85` IN bulk, 512 |
| 2 | 0 | not claimed | `0x86` IN bulk, 512; `0x07` OUT bulk, 512 |
| 3 | 0 | not claimed | `0x88` IN bulk, 512; `0x09` OUT bulk, 512 |
| 4 | 0 | not claimed | `0x8a` IN bulk, 512 |
| 5 | 0 | status | `0x8b` IN interrupt, 4 |

The audio engine uses the playback and capture endpoints.  At 1x and 2x that
is playback endpoint 0 and capture endpoint 0.  At 4x it adds playback
endpoint 1 and capture endpoint 1.  The interrupt endpoints use a
one-microframe interval.

### 11.4 AP personality

The AP personality uses isochronous interface 0 and the same meter
interface.  It uses playback endpoint 0 and capture endpoint 0 at every rate
class.  These rows are the mapping the driver uses.

| Interface | Alt | Purpose | Endpoints |
|---|---|---|---|
| 0 | 1 | 1x audio | `0x01` OUT isochronous; `0x82` IN isochronous |
| 0 | 2 | 2x audio | `0x01` OUT isochronous; `0x82` IN isochronous |
| 0 | 3 | 4x audio | `0x01` OUT isochronous; `0x82` IN isochronous |
| 1 | 0 | meter | `0x85` IN bulk |
| 4 | 0 | status | `0x8b` IN bulk |

### 11.5 Control requests

Mixer and setting writes use EP0 control requests.  The request number selects
the target register or bank.

| Direction | `bmRequestType` | Data stage |
|---|---|---|
| Host to device | `0x40` | none |
| Device to host | `0xC0` | 2 or 4 bytes |

The request numbers and values are in "EP0 request map".  The host reads
`0x11` and `0x1c` as four bytes and `0x17` as two bytes.  All other requests
it sends are writes.

### 11.6 PC audio packets

| Alt | Rate class | Sample words per endpoint | Endpoints | Maximum frames per packet | Maximum packet bytes |
|---|---|---|---|---|---|
| 1 | 1x | 18 | 1 | 8 | 576 |
| 2 | 2x | 14 | 1 | 16 | 896 |
| 3 | 4x | 6 | 2 | 32 | 768 |

The IN packet and the OUT packet have the same size.  A sample frame carries
one sample word per active channel.  The PCM stream has 18 logical inputs and
18 software playback channels.  A packet carries the first 18, 14 or 12 words
of that stream.

At 1x, sample words `0..17` carry logical channels `1..18`.  At 2x, sample
words `0..13` carry channels `1..14`.  At 4x, playback endpoint 0 and capture
endpoint 0 carry words `0..5` for channels `1..6`.  Playback endpoint 1 and
capture endpoint 1 carry words `0..5` for channels `7..12`.

The sample word is float32 on the PC and PC RME variants, and on
`UC_WIN_RME` below firmware 138.  It is S32 on `UC_WIN_RME` from firmware
138.  Sample words are little-endian.

One URB carries 48 sample frames at 1x, 96 at 2x and 192 at 4x.  That is six
maximum-size packets.  Two URBs are in flight per endpoint.  The IN URBs are
submitted at stream start.  The OUT URBs are filled from the PCM ring and
resubmitted at completion.  An interrupt packet starts with the first sample
word.  The four-byte header and the lead word belong to the AP packets.

### 11.7 AP audio packets

| Alt | Rate class | Sample words per frame | Maximum frames per packet | IN maximum packet | OUT maximum packet |
|---|---|---|---|---|---|
| 1 | 1x | 18 | 7 | 508 | 532 |
| 2 | 2x | 14 | 13 | 732 | 780 |
| 3 | 4x | 12 | 25 | 1208 | 1304 |

The IN packet is a four-byte header followed by `frames × sample words × 4`
bytes.  The OUT packet is `frames × (1 + sample words) × 4` bytes.  The
maximum packet columns are the descriptor allocation sizes.

The IN header is a four-byte little-endian value.  The driver reads the
position from bits 5..17 and the phase from bits 18..31.  It derives the frame
count from the packet length.

Every OUT sample frame starts with a lead word.  The UC uses the first lead
word in a packet; the remaining lead words are padding.  The first lead word
is the stream position modulo 8192.

One ISO URB carries up to eight packets, one per USB microframe.  Two URBs are
in flight.  The AP sample word is float32.

### 11.8 ALSA PCM

The driver exposes one duplex PCM.

| Property | Value |
|---|---|
| Channels | 18 playback and 18 capture |
| Formats | `FLOAT_LE` and `S32_LE` |
| Rates | 32 kHz to 192 kHz |
| Buffer | managed vmalloc buffer |

Capture sample word `n` maps to ALSA capture channel `n`.  Playback sample
word `n` maps from ALSA playback channel `n`.  The profile maps each ALSA
channel to a named input or playback row.

The driver converts between the device sample word and the ALSA format.

| Device word | `FLOAT_LE` | `S32_LE` |
|---|---|---|
| float32 | unchanged | converted |
| S32 | converted | unchanged |

At 2x and 4x, channels outside the carried sample words are silent on
capture and ignored on playback.

### 11.9 Meter and status

The meter endpoint is IF1 IN `0x85` bulk.  The driver requests one 324-byte
page per URB.  A request for two pages returns one page, so the driver pairs
pages in the kernel.  Meter URBs run while a hwdep reader has the node open.
The page and heartbeat rules are in "Metering".

The status endpoint is IF5 IN `0x8b` interrupt, four bytes on the PC
personality.  It is IF4 IN `0x8b` bulk on the AP personality.  The frame
layout is in "Front-panel events".  The driver keeps the status URB armed
while the device is connected.

### 11.10 Alternate settings

| Rate class | Alt |
|---|---|
| 1x | 1 |
| 2x | 2 |
| 4x | 3 |

The PC personality selects the alt for the rate class.  The Win RME variant
selects alt 0 and then alt N on every stream start.  The other PC variants
select alt N on a rate-class change.  The AP personality selects alt N for the
rate class.

### 11.11 Open transport items

1. The AP descriptor.  The AP interface and packet rows are the implemented
   mapping; the descriptor has not been captured.
2. Interfaces 2, 3 and 4 on the PC personality.  The endpoints are present;
   the driver does not claim them and their roles are not decoded.
3. MIDI.  Interface 2 has a bulk IN/OUT pair on the PC descriptor; the
   driver creates no rawmidi port for the UC and the protocol is not decoded.
4. The read form of `0x10`.  "EP0 request map" lists it as write or read;
   the implemented host only writes it.
5. The PC interrupt URB size.  The 48/96/192 sample-frame size is the host's
   choice; the device contract is the packet size and the rate class.
6. The AP header phase field.  The driver decodes it; it has not been checked
   against a capture.
7. Status bytes 0 and 1.  "Front-panel events" uses bytes 2 and 3; the roles
   of bytes 0 and 1 are not decoded.
8. The bulk endpoints on interface 0 at 1x and 2x.  `0x03` and `0x84` are
   bulk there and interrupt at 4x; the bulk role is not decoded.

## 12. Code map

| Concern | Source |
|---|---|
| EP0 requests and device sequences | `kernel/ffuc.c` |
| PCM, URBs and sample conversion | `kernel/ffuc.c` |
| Meter pairing, status and hwdep events | `kernel/ffuc-hwdep.c` |
| hwdep ABI | `kernel/ffuc-uapi.h` |
| UC control and meter decode | `ui/totalcontrol/totalcontrol-core/src/fireface/device.rs` |
| UC protocol constants and parsers | `ui/totalcontrol/totalcontrol-core/src/fireface/protocol.rs` |
| UC profile data | `ui/totalcontrol/totalcontrol-core/src/profiles/uc.rs` |
| Profile selection and ownership | `docs/spec/TOTALCONTROL-DEVICE-PROFILE-SPEC.md` |
