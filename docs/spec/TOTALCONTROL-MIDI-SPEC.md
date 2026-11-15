# MIDI transport and Mackie Control specification

This document defines the USB-MIDI transport the kernel driver exposes,
the ALSA rawmidi surface built on it, and the Mackie Control host that
TotalControl implements on top. It is generic across devices; the device
protocol specs own the per-device interface and endpoint facts and refer
here for the protocol and the remote model. OSC remote control is a
separate surface with its own document
([`TOTALCONTROL-OSC-SPEC.md`](TOTALCONTROL-OSC-SPEC.md)); both share the
mixer model and the fader curve.

| Part | Status |
|---|---|
| USB-MIDI transport on the Fireface UC | implemented, verified on the wire |
| USB-MIDI transport on the Fireface UCX | implemented, verified |
| USB-MIDI transport on the Babyface-classic | implemented, not verified |
| Two-port routing | implemented |
| ALSA rawmidi surface | implemented |
| Mackie Control host | implemented, tested against the codec and the mixer model; live-surface verification open |

Not supported:

| Capability | Reason |
|---|---|
| UMP and MIDI 2.0 | not implemented |
| Clock generation | the driver carries bytes, it does not pace them |
| Messages larger than 128 bytes | the driver sends one 128-byte chunk per transfer, and a split message is lost |
| HUI and Eucon | TotalMix FX does not implement them either; the controller protocol is Mackie Control |
| More than two MIDI remotes, extender linking | the device ports are the remotes; TotalMix FX's four-remote/extender model is open (§13) |
| Reverb/echo user presets via CC | the mixer model has no FX user presets |

## Contents

- [1. Terms](#1-terms)
- [2. USB transport](#2-usb-transport)
- [3. Device matrix and class compliance](#3-device-matrix-and-class-compliance)
- [4. ALSA rawmidi surface](#4-alsa-rawmidi-surface)
- [5. Outbound messages](#5-outbound-messages)
- [6. Inbound messages](#6-inbound-messages)
- [7. Limits](#7-limits)
- [8. MIDI remote control model](#8-midi-remote-control-model)
- [9. Mackie Control wire protocol](#9-mackie-control-wire-protocol)
- [10. Feedback](#10-feedback)
- [11. TotalControl host](#11-totalcontrol-host)
- [12. Device differences](#12-device-differences)
- [13. Open items](#13-open-items)
- [14. Code map](#14-code-map)

## 1. Terms

| Term | Shape | Meaning |
|---|---|---|
| MIDI interface | IF2 | The USB interface that carries MIDI packets in both directions. |
| MIDI endpoint pair | OUT `0x07`, IN `0x86` | The two bulk endpoints of the MIDI interface. |
| MIDI port | byte 0 bits 7..4 | A logical MIDI port on the endpoint pair; 0 is port 1 and 1 is port 2. |
| MIDI packet | 4 bytes | One port, one code and up to three MIDI bytes. |
| Code index number | byte 0 bits 3..0 | The USB-MIDI 1.0 code for the message class and payload length. |
| Bulk transfer | 1 to 512 bytes | One host write to the OUT endpoint. |
| Rawmidi device | ALSA node | One ALSA rawmidi device per MIDI port. |
| Surface | — | A Mackie Control compatible controller. |
| Host | — | The application a surface controls; TotalControl. |
| Remote | — | One surface connection: an input port, an output port and its settings. |
| Strip | — | One channel column of the surface: fader, VPOT, four buttons. |
| Window | 8 strips | The channels the remote currently addresses. |
| Row | — | One mixer row: hardware inputs, software playback, hardware outputs. |
| Submix | output pair | The output whose mix the faders edit. |

## 2. USB transport

| Property | Value |
|---|---|
| Interface | 2 |
| OUT endpoint | bulk `0x07`, 512-byte maximum packet |
| IN endpoint | bulk `0x86`, 512-byte maximum packet |
| Directions | duplex |
| Packet size | 4 bytes |
| Ports | 1 or 2, per §3 |

A MIDI packet is:

| Byte | Bits | Meaning |
|---|---|---|
| 0 | 7..4 | MIDI port |
| 0 | 3..0 | Code index number |
| 1 | | MIDI byte 1 |
| 2 | | MIDI byte 2, or 0 when unused |
| 3 | | MIDI byte 3, or 0 when unused |

The code index number is a USB-MIDI 1.0 code in both directions:

| Code | MIDI bytes | Message |
|---|---|---|
| `0x2` | 2 | two-byte system common |
| `0x3` | 3 | three-byte system common |
| `0x4` | 3 | SysEx start or continue |
| `0x5` | 1 | SysEx end |
| `0x6` | 2 | SysEx end |
| `0x7` | 3 | SysEx end |
| `0x8` | 3 | note off |
| `0x9` | 3 | note on |
| `0xa` | 3 | poly pressure |
| `0xb` | 3 | control change |
| `0xc` | 2 | program change |
| `0xd` | 2 | channel pressure |
| `0xe` | 3 | pitch bend |
| `0xf` | 1 | single byte |

The high nibble selects the port. On the UC, TotalMix FX sends the packet
`9f 7f 5a` on the selected port periodically while a MIDI remote output is
enabled; it is the loopback-detection note (§8.6).

RME's Windows driver sends the payload length in the low nibble instead:
`0xf` for one byte, `0x2` for two and `0x3` for three. The UC accepts both
forms. This driver sends the code index number.

On the UC, the unit parses each bulk transfer as a separate MIDI stream. A
message must lie within one bulk transfer. A SysEx that starts in one
transfer and continues in the next loses the continuation bytes.

## 3. Device matrix and class compliance

| Model | IF2 class | Ports | Port source | Stand-alone MIDI | CC mode |
|---|---|---|---|---|---|
| Fireface UC, PC personality | vendor-specific `ff/00/00` | 2 | device profile | yes (§12) | no |
| Fireface UC, AP personality | not captured | 2 expected | not captured | yes | no |
| Fireface UCX | Audio, MIDI streaming `01/03` | 2 | class endpoint descriptor | yes (§12) | yes |
| Babyface-classic | not captured | 1 | device profile | no | yes |

The port count comes from the class-specific endpoint descriptor when the
interface declares one: each embedded jack is one port. The UCX declares
two embedded IN jacks and two embedded OUT jacks, `bNumEmbMIDIJack = 2` on
both endpoints. A vendor-specific interface has no such descriptor and
takes the count from the device profile. The driver exposes at most two
ports.

Class compliance has two layers:

| Layer | UC PC | UC AP | UCX | Babyface |
|---|---|---|---|---|
| Wire format | USB-MIDI 1.0, verified | assumed | USB-MIDI 1.0 | USB-MIDI 1.0 |
| USB declaration | vendor-specific | class-compliant per RME documentation | class-compliant, captured | not captured |

RME's product sheet and manuals state that the UC's MIDI ports are class
compliant under Mac OS, i.e. in the AP personality; the UC has no full CC
mode. The UCX manual states the same for the UCX in normal mode. The
Babyface manual states it for the Babyface; its CC mode (firmware 200 and
later, Select+Recall at power-on) is fully class compliant including
SysEx-capable MIDI.

The UCX declaration is verified in **normal mode**, not only in CC mode:
the saved descriptor capture shows the vendor-specific audio interface 0
and the serial-bearing product string of normal mode, while interface 2
already declares `01/03` with full MIDIStreaming descriptors. The UCX and
UCX II manuals repeat "The Fireface's MIDI is class compliant … OS X …
will be using it with the driver included in the operating system" in
their normal-mode Mac FAQ. CC mode re-enumerates the whole unit with
class descriptors; MIDI stays class compliant in both modes. The UC is
the older design — vendor-specific MIDI on the PC personality, class
compliant on AP — and the UCX unified it into one personality. RME's
stated motivation for CC mode itself is the iPad; the class-compliant
MIDI declaration exists independently of it.

Host binding on Linux follows from the declaration:

| Interface declaration | `snd-usb-audio` behaviour |
|---|---|
| MIDI streaming subclass (3) | binds the interface, creates one rawmidi device per embedded jack |
| vendor-specific, subclass 0 (UC PC) | skips the interface; only this driver exposes MIDI |

The driver claims IF2 on every model. It binds the audio interface first
and claims IF2 during probe, before the class driver is tried on it, so
one card carries the audio and the MIDI ports. When `snd-usb-audio`
already holds IF2 (the module loaded after the device enumerated), the
driver logs `no MIDI port` and continues without MIDI; the ports remain
available on the class driver's card. A user can therefore connect
directly to the UCX's MIDI endpoints without this driver; on the UC PC
personality the driver is required.

In CC mode the whole unit re-enumerates with class descriptors and
belongs to `snd-usb-audio`; this driver does not bind class interfaces.
On the UCX and Babyface, RME's TotalMix FX for iPad app then controls the
unit's mixer over SysEx on MIDI port 2 (observed on the UCX II; assumed
for the UCX, §13).

## 4. ALSA rawmidi surface

| Property | Value |
|---|---|
| Devices | one rawmidi device per MIDI port, indices 0 to ports-1 |
| Name | `<model> MIDI <port>`, port counted from 1 |
| Streams | input and output on every device |
| Node | `/dev/snd/midiC<card>D<device>` |
| Input start | when the first input substream opens |
| Input URBs | 2 URBs of 512 bytes |
| Input stop | when the last input substream closes |
| Output start | on the output trigger |
| Output poll | every 1 ms while triggered |
| Output chunk | up to 128 packets, 512 bytes |

The input URBs run only while an input substream is open. A bulk IN URB
that completes with an error is submitted again, so the input stream does
not stop on a transient error.

The output work takes one chunk from the rawmidi buffer per poll and
writes it as one bulk transfer. Data is acknowledged when the transfer
completes. On a failed transfer the driver logs a warning, clears a
stalled endpoint and drops the chunk, so a writer cannot block on data
that the unit refused.

The driver creates rawmidi devices only. The ALSA core also registers the
card as a sequencer client with one port for each MIDI port.

The MIDI settings use the unit's own rawmidi ports: port 1 is rawmidi
device 0, port 2 is device 1, on the unit's ALSA card. A controller
connects through the breakout cable.

## 5. Outbound messages

The driver converts a raw MIDI byte stream into packets:

| Input bytes | Packet code | Message |
|---|---|---|
| `8n kk vv` | `0x8` | note off |
| `9n kk vv` | `0x9` | note on |
| `an kk vv` | `0xa` | poly pressure |
| `bn cc vv` | `0xb` | control change |
| `cn pp` | `0xc` | program change |
| `dn pp` | `0xd` | channel pressure |
| `en ll mm` | `0xe` | pitch bend, 14-bit little-endian |
| `f1 xx`, `f3 xx` | `0x2` | two-byte system common |
| `f2 ll mm` | `0x3` | song position |
| `f6` | `0xf` | tune request |
| `f8` to `ff` | `0xf` | realtime, may interleave between other packets |
| `f0 ...` | `0x4` | SysEx start or 3-byte chunk |
| `... f7` | `0x5`, `0x6`, `0x7` | SysEx end with 1, 2 or 3 bytes |

Running status is expanded: every packet carries its status byte. A data
byte after a completed channel message reuses the previous status. `f4`
and `f5` are undefined and are dropped.

A partial message that has produced no packet is held in the port state
and is sent when it completes. Bytes that were already written in an
earlier transfer cannot be continued; see §7.

## 6. Inbound messages

The driver decodes every packet by its code index number and passes the
payload bytes to the rawmidi device for the port in the high nibble. A
packet for a port that does not exist is dropped. A code with no payload
length is dropped.

SysEx is delivered as a byte stream. The client sees the `f0` and `f7`
bytes and decides where a message begins and ends.

## 7. Limits

| Limit | Value |
|---|---|
| Message per transfer | the unit loses a continuation that arrives in a later transfer |
| Output chunk | 128 packets, 512 bytes |
| Input URB | 512 bytes |
| Output poll | 1 ms |

A caller must write a message in one piece that fits one output chunk. A
message written in several pieces, or larger than the chunk, is split
across transfers and the unit drops the continuation. A message larger
than 128 bytes cannot be sent whole. The host's feedback messages (LCD
SysEx, handshake) are all below this limit.

## 8. MIDI remote control model

This is the TotalMix FX remote model, implemented by TotalControl. The
note and CC values are the Mackie Control Universal map (§9); the
meanings are RME's.

### 8.1 Enable and view

The remote is off until enabled in the settings. MIDI remote control
always operates in Submix view: the faders edit the mix of the selected
submix, even when the GUI is in Free mode. The remote's submix follows
the submix selected in the GUI (submix-linked), and selecting a submix on
the remote selects it in the GUI.

### 8.2 Surface mapping

| Surface control | TotalControl action |
|---|---|
| Strip faders 1–8 | volume of the window channel, through the fader curve |
| Master fader | Main Out pair volume |
| VPOT turn | pan of the window channel (balance on outputs) |
| VPOT push | pan = center |
| MUTE 1–8 | channel mute toggle |
| SOLO 1–8 | channel solo toggle |
| SELECT 1–8 | ad-hoc fader-group selection |
| REC 1–8 | select the submix under the strip (output row, or any row with the option) |
| BANK LEFT / RIGHT, ARROW LEFT / RIGHT | move the window eight strips |
| CHANNEL LEFT / RIGHT, REWIND / FAST FORWARD | move the window one strip |
| ARROW UP / DOWN, Assignable 1 / 2 (PAGE+/-) | move one row |
| EQ | Master Mute (Main Out pair) |
| PLUGINS / INSERT | Master Solo (Main Out pair) |
| STOP | Dim |
| PLAY | Talkback |
| PAN | Mono |
| FLIP | Speaker B |
| DYN / INSTRUMENT | Trim Gains mode toggle |
| SEL + DYN | Trim Gains mode (the model trim is per channel) |
| RECORD | Recall |
| F1–F8 | load Snapshot 1–8 |
| F9 | select Main Out and cue it |
| F10–F12 | cue Phones 1–3 (device-dependent, §12) |
| Fader touch | carried, no action |
| Transport, zoom, scrub, save, undo, shift, assignment buttons | no TotalMix meaning; ignored |

In Trim Gains mode the strip faders write the channel trim in dB instead
of the send volume; outputs keep their volume. A linked stereo pair is
one strip and moves together; with "Map Stereo to 2 Mono Channels" each
channel is its own strip.

### 8.3 Simple CC and note commands

These work without a Mackie surface, from any MIDI controller. The note
commands are the same notes as the surface buttons (§9.2), on MIDI
channel 1; they are part of the Mackie protocol and are disabled with it.

| Command | Action |
|---|---|
| CC 7 (Volume), channel 1 | Main Out volume |
| CC 9, channels 1–16 | input gain, value = the gain itself; controllable inputs are device-dependent (§12) |
| CC 102–117, channels 1–4 | row 1 faders, 16 per channel block |
| CC 102–117, channels 5–8 | row 2 faders |
| CC 102–117, channels 9–12 | row 3 faders |
| CC 102, channel 13 | Trim Gains on |
| CC 103, channel 13 | Trim Gains off |
| CC 104+2n / 105+2n, channel 13 | select submix n (pair) |
| Note 0x5D | Dim |
| Note 0x2A | Mono |
| Note 0x5E | Talkback |
| Note 0x5F | Recall |
| Note 0x32 | Speaker B |
| Note 0x3E | Cue Main Out |
| Notes 0x3F–0x42 | Cue Phones 1–4 |
| Notes 0x36–0x3D | Snapshot 1–8 |
| Note 0x2D | Trim Gains |
| Note 0x2C | Master Mute |
| Note 0x2B | Master Solo |

The CC fader value and the pitch-bend value map to the same normalized
fader position: `p = v/127` for 7-bit CC, `p = v/16383` for 14-bit pitch
bend. The position goes through the fader curve of the OSC spec
(`fader_db`): 104/127 ≈ 0 dB, 127/127 = +6 dB, 0 = −∞. This matches the
RME manual's CC examples.

### 8.4 Options

| Option | Effect | Default |
|---|---|---|
| Enable Mackie protocol | off: only the simple CC commands of §8.3 are accepted | on |
| Full LCD support | eight names and eight values; off: brief info about the first fader | on |
| Send user defined names | user labels replace the channel names on the LCD | on |
| Send level messages | meter feedback (§10) | off |
| Peak hold | meters follow the held peak instead of the instant level | off |
| Map Stereo to 2 Mono Channels | one strip per channel instead of per linked pair | off |
| Submix Selection in Input/Playback Row | REC selects the submix from any row | off |
| Disable MIDI in background | suspend the remote while the window is unfocused or hidden | off |
| Submix linked | the remote follows the GUI's submix selection | on |

The settings page exposes the first five plus the port choice and the
enable switch; the remaining options keep their defaults in this version
(§13). MIDI Out may be None: control works without feedback.

### 8.5 Window marking

The channels in the remote's window show their name field in the remote
tint (brown), like TotalMix. Channels hidden by the channel layout are
skipped in the window.

### 8.6 Loopback detection

While the remote is online with an output port, the host sends the note
`9f 7f 5a` (note 127, velocity 90, channel 16) every 500 ms. Hearing that
exact note on the input means the output is wired back into the input;
the host disables the remote until the user re-enables it. A surface
never sends this note.

## 9. Mackie Control wire protocol

Messages use MIDI channel 1 unless a row states otherwise. A button is a
note on with velocity 127 when pressed and a note off or velocity 0 when
released. An LED is the same note with velocity 127 for on, 1 for
flashing and 0 for off.

### 9.1 Connection handshake

SysEx header: `f0 00 00 66 14` (Mackie manufacturer and device ID),
closed by `f7`.

| Code | Direction | Payload | Meaning |
|---|---|---|---|
| `0x00` | host → surface | — | device query |
| `0x01` | surface → host | serial[7], challenge[4] | host connection query (also at power-on) |
| `0x02` | host → surface | serial[7], response[4] | host connection reply |
| `0x03` | surface → host | serial[7] | host connection confirmation |
| `0x04` | surface → host | serial[7] | host connection error |
| `0x0f` | host → surface | `0x7f` | go offline |
| `0x13` | host → surface | `0x00` | firmware version request |
| `0x14` | surface → host | version[5] | version reply |

The response code derives from the challenge `c[0..4]`:

```text
r[0] = 0x7f & (c[0] + (c[1] ^ 0x0a) - c[3])
r[1] = 0x7f & ((c[2] >> 4) ^ (c[0] + c[3]))
r[2] = 0x7f & ((c[3] - (c[2] << 2)) ^ (c[0] | c[1]))
r[3] = 0x7f & (c[1] - c[2] + (0xf0 ^ (c[3] << 4)))
```

The host sends the device query when the port opens, answers a connection
query with the reply, and marks the remote online on the confirmation. A
surface that skips the handshake goes online on its first control
message. On close the host sends go-offline.

### 9.2 Note map

| Control | Notes |
|---|---|
| REC 1–8 | `0x00`–`0x07` |
| SOLO 1–8 | `0x08`–`0x0f` |
| MUTE 1–8 | `0x10`–`0x17` |
| SELECT 1–8 | `0x18`–`0x1f` |
| VPOT push 1–8 | `0x20`–`0x27` |
| Assignment: track, send, pan, plug-in, EQ, instrument | `0x28`–`0x2d` |
| Bank left / right | `0x2e`, `0x2f` |
| Channel left / right | `0x30`, `0x31` |
| Flip, global view | `0x32`, `0x33` |
| F1–F8 | `0x36`–`0x3d` |
| MIDI tracks, inputs, audio tracks, audio instruments, aux | `0x3e`–`0x42` |
| Shift, option, control, alt | `0x46`–`0x49` |
| Save, undo, cancel, enter | `0x50`–`0x53` |
| Rewind, forward, stop, play, record | `0x5b`–`0x5f` |
| Cursor up, down, left, right, zoom, scrub | `0x60`–`0x65` |
| User switch 1 / 2 (Assignable 1 / 2) | `0x66`, `0x67` |
| Fader touch 1–8, master | `0x68`–`0x70` |
| SMPTE, BEATS, rude solo LEDs | `0x71`–`0x73` |

### 9.3 Control change map

| Message | Control | Values |
|---|---|---|
| CC `0x10`–`0x17`, ch 1 | VPOT turn 1–8 | 1–63 clockwise, 65–127 counter-clockwise (relative) |
| CC `0x30`–`0x37`, ch 1 | VPOT LED ring 1–8 | bit 6 center LED; bits 5..4 mode; bits 3..0 position |
| CC `0x3c`, ch 1 | jog wheel | relative, same encoding |
| Pitch bend ch 1–8 | strip faders 1–8 | 14-bit, 0–16383, low byte first |
| Pitch bend ch 9 | master fader | 14-bit |

The ring uses mode 0 (single dot) with position 0–11 from the pan; the
center LED lights at pan 0.

### 9.4 LCD SysEx

`f0 00 00 66 14 12 <offset> <text> f7`. The offset is 0–55 for the top
row and 56–111 for the bottom row; text is ASCII, at most 112 bytes per
message. The full layout is eight 7-character names on the top row and
eight 7-character values on the bottom row. The brief layout writes the
row and first channel number at offset 0.

## 10. Feedback

| Feedback | Message |
|---|---|
| Fader position | pitch bend, same channels as §9.3 |
| Button LED | note on with velocity 127 or 0 |
| VPOT ring | CC `0x30`–`0x37` |
| LCD text | SysEx `0x12` (§9.4) |
| Meter | channel pressure, high nibble strip 0–7, low nibble level |

The meter nibble follows the Mackie scale:

| Level | Nibble |
|---|---|
| > 0 dB | `0xd` |
| 0 dB | `0xc` |
| ≥ −2, −4, −6, −8, −10 dB | `0xb`, `0xa`, `0x9`, `0x8`, `0x7` |
| ≥ −14, −20, −30, −40, −50, −60 dB | `0x6`, `0x5`, `0x4`, `0x3`, `0x2`, `0x1` |
| < −60 dB | `0x0` |
| Overload set / clear | `0xe` / `0xf` |

Feedback is delta-encoded: the host keeps the last-sent fader, LED, ring,
LCD and meter values per remote and emits only the changes. A navigation
or submix change refreshes the whole window. Meters are sent at about
25 Hz while enabled. The loopback note (§8.6) is sent every 500 ms.

## 11. TotalControl host

TotalControl is the host: a Mackie controller connected to the unit's MIDI
ports drives the TotalControl mixer. The remotes are the device ports:
port 1 is remote 1, port 2 is remote 2. The UC and UCX offer two remotes,
the Babyface-classic one. Each remote has its own navigation and feedback
state; the options apply to all remotes. Two controllers can run at once,
one per port.

The host is device-independent: it drives the shared mixer model, and the
device backend supplies the writes. It is available on every device and
personality TotalControl supports — UC PC, UC AP, UCX (both transports)
and Babyface-classic — with the same per-device capability clamps as the
GUI. On the UC AP personality the driver exposes no MIDI of its own; the
class driver provides the ports (RME documents AP MIDI as class
compliant). In CC mode the whole unit belongs to the class driver and
TotalControl does not bind it; the unit's own stand-alone MIDI and the
iPad app are the control paths there, not this host.

| Concern | Behaviour |
|---|---|
| Transport | one worker per enabled port opens the rawmidi pair; input bytes are parsed on the UI thread |
| Parser | running status expanded, SysEx framed, realtime passed through |
| Navigation | per-remote row, window offset (in strips), submix and trim state; not part of the workspace |
| Actions | dispatched as the ordinary mixer messages, shared with the GUI and OSC |
| Toggles | resolved against the confirmed model, like OSC |
| Settings | the MIDI part of the MIDI and OSC dialog (§11.1); persisted in host settings |
| Loopback guard | trips on the detection note, disables that remote, logs |
| Background disable | suspends input and feedback while unfocused or hidden |
| Window marking | the name tint of §8.5, per remote |

### 11.1 Settings

`Options > MIDI and OSC` opens the remote-control dialog, separate from
the app-level TotalControl settings. The MIDI part: an `Enable` checkbox,
the line "Use a Mackie controller to control the mixer.", `MIDI In` and
`MIDI Out` checkbox rows (`Port 1`, `Port 2` — one port on the Babyface),
the option checkboxes of §8.4 (first five), and a MIDI debug log. Each
checked MIDI In port runs a remote; its feedback goes to the same-numbered
checked MIDI Out port. Toggling applies immediately. The OSC part is
unchanged apart from the default receive port 7001, the feedback
destination field and the host address line; see the GUI spec §1.4.

The remote settings are host settings, not workspace state (STATE-SPEC
§7.1). The debug log is a bounded ring, shown on demand.

## 12. Device differences

| Aspect | UC | UCX | Babyface-classic |
|---|---|---|---|
| MIDI ports | 2 | 2 | 1 |
| IF2 declaration (normal mode) | vendor-specific (PC) | class-compliant | not captured |
| Cue phones in the Mackie map | F10–F12 (1–3) | F10–F12 (1–3) | F10–F11 (1–2) |
| Simple-note cue range | Phones 1–4 | Phones 1–4 | Phones 1–2 |
| Input gain CC9 | inputs 1/2, up to 65 | inputs 1/2, up to 65 | inputs 1/2, up to 60, steps ≥ 3 dB |
| Reverb/echo preset CC | — | `B2 0C` / `B3 0C` (open, §13) | — |
| Stand-alone MIDI | yes | yes | no |
| CC mode | no | yes | yes |

Stand-alone MIDI control is a unit mode, not a TotalControl feature: with
the unit disconnected from the computer, the unit itself acts as the
Mackie host. It is enabled from TotalMix FX (Options, Enable MIDI Control)
and persists in the unit. It covers the faders and pans of rows 1 and 3,
input mute, ganging via Select, submix selection and LED/display
feedback; the playback row is skipped, and Talkback, Dim, Mono, Solo and
relative ganging are not available in hardware. The UCX adds STOP = Dim
and F1–F6 = Setup 1–6 (plus the simple notes Dim and Setup 1–6); the UC
adds SEL+DYN = unity-gain reset. After power-on the unit does not update
the controller's faders. TotalControl must not claim to control the unit
while it is in this mode.

## 13. Open items

1. UC AP personality MIDI: the interface class and behaviour are not
   captured; the driver exposes no MIDI on AP. Needs an AP-mode
   descriptor capture.
2. Babyface-classic MIDI: the normal-mode interface class and jack
   descriptors are not captured; the port count comes from the profile.
3. UCX wire verification: the class-codec path is not verified on the
   UCX wire, including whether the UCX shares the UC's one-message-per-
   transfer parsing rule.
4. UC port 2 physical output: packets carry port 2 and the front panel
   shows activity, but the physical output it drives is not identified.
5. Live-surface verification of the host: handshake, LCD, meters and the
   option interactions (trim × submix-linked × submix-selection row)
   against a real surface.
6. Multi-remote and extender: up to four remotes forming one block, as
   in TotalMix FX. The codec is per-remote; the settings page exposes
   one.
7. FX user presets via `B2/B3 0C`: needs the reverb/echo user-preset
   model first.
8. The remaining §8.4 options (stereo-to-mono, submix-selection row,
   background disable, submix-linked) are implemented in the codec and
   persisted, but not exposed in the settings page.

## 14. Code map

| Concern | Source |
|---|---|
| USB-MIDI transport, port routing, packet codec | `kernel/ffuc-midi.c` |
| Device MIDI fields and quirk bits | `kernel/ffuc.h` |
| MIDI interface, endpoints and port count per model | `kernel/ffuc.c` |
| UC MIDI wire format | `FIREFFACE-UC-PROTOCOL-SPEC.md` §11.3 |
| UCX MIDI wire format | `FIREFFACE-UCX-PROTOCOL-SPEC.md` §2.5 |
| Babyface MIDI transport | `BABYFACE-CLASSIC-PROTOCOL-SPEC.md` §2 |
| Mackie codec, handshake, feedback encoder | `totalcontrol-gui/src/mackie.rs` |
| Rawmidi transport and port enumeration | `totalcontrol-gui/src/midi.rs` |
| Host dispatch, feedback, settings | `totalcontrol-gui/src/app.rs` |
| Remote settings persistence | `totalcontrol-gui/src/host_settings.rs` |