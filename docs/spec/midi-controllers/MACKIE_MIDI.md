# Mackie Control MIDI specification

This document defines Mackie Control as a TotalControl control surface: the
wire protocol, the mapping onto the mixer model, the feedback and the host
behaviour. It follows the controller model of
[`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
and is described by the map file `Mackie_MIDI.json`.

The surface connects to a MIDI port. On a Fireface UC, UCX or Babyface-classic
that port is the unit's own USB-MIDI interface, specified in the device
protocol documents and in [`../TOTALCONTROL-MIDI-SPEC.md`](../TOTALCONTROL-MIDI-SPEC.md).

| Part | Status |
|---|---|
| Wire protocol | implemented |
| Mixer mapping | implemented |
| Feedback | implemented |
| Live-surface verification | open (§15) |

## Contents

- [1. Terms](#1-terms)
- [2. Transport](#2-transport)
- [3. Note map](#3-note-map)
- [4. Control change map](#4-control-change-map)
- [5. Pitch bend](#5-pitch-bend)
- [6. Connection handshake](#6-connection-handshake)
- [7. LCD SysEx](#7-lcd-sysex)
- [8. Mapping onto the mixer](#8-mapping-onto-the-mixer)
- [9. Simple CC and note commands](#9-simple-cc-and-note-commands)
- [10. Feedback](#10-feedback)
- [11. Host behaviour](#11-host-behaviour)
- [12. Options list](#12-options-list)
- [13. Device differences](#13-device-differences)
- [14. Map file](#14-map-file)
- [15. Open items](#15-open-items)

## 1. Terms

| Term | Address or shape | Definition |
|---|---|---|
| surface | MIDI device | A Mackie Control compatible controller. |
| host | — | The application the surface controls: TotalControl. |
| strip | — | One channel column: fader, VPOT, MUTE, SOLO, SELECT, REC, fader touch. |
| master fader | pitch bend channel 9 | The ninth fader, mapped to the Main Out pair. |
| VPOT | CC `0x10`–`0x17` in, `0x30`–`0x37` out | The rotary encoder above each strip. |
| ring | CC `0x30`–`0x37` out | The LED ring around a VPOT. |
| window | 8 strips | The channels the surface currently addresses. |
| row | — | One mixer row: hardware inputs, software playback, hardware outputs. |
| submix | output pair | The output whose mix the faders edit. |
| trim mode | — | The state where strip faders write channel trim instead of send level. |
| loopback note | `9f 7f 5a` | The host's detection note for a wired-back output. |

Arrows in the tables: `→` is device to host, `←` is host to device.

## 2. Transport

The surface uses any MIDI port the host offers. On the device MIDI ports, the
port rules and the message-size limit are in
[`../TOTALCONTROL-MIDI-SPEC.md`](../TOTALCONTROL-MIDI-SPEC.md) §2, §4 and §7.

| Property | Value |
|---|---|
| Messages | channel messages on channel 1 unless a row states otherwise |
| Button press | Note On, velocity 127 |
| Button release | Note Off, or Note On with velocity 0 |
| LED | Note On with velocity 127 for on, 1 for flashing, 0 for off |
| Max message length | 128 bytes on the device ports |

Messages use running status freely in both directions. The host state is one
parser per remote.

## 3. Note map

| Control | Notes |
|---|---|
| REC 1–8 | `0x00`–`0x07` |
| SOLO 1–8 | `0x08`–`0x0f` |
| MUTE 1–8 | `0x10`–`0x17` |
| SELECT 1–8 | `0x18`–`0x1f` |
| VPOT push 1–8 | `0x20`–`0x27` |
| Assignment: track, send, pan, plug-in, EQ, instrument | `0x28`–`0x2d` |
| Bank left, bank right | `0x2e`, `0x2f` |
| Channel left, channel right | `0x30`, `0x31` |
| Flip, global view | `0x32`, `0x33` |
| F1–F8 | `0x36`–`0x3d` |
| MIDI tracks, inputs, audio tracks, audio instruments, aux | `0x3e`–`0x42` |
| Shift, option, control, alt | `0x46`–`0x49` |
| Save, undo, cancel, enter | `0x50`–`0x53` |
| Rewind, forward, stop, play, record | `0x5b`–`0x5f` |
| Cursor up, down, left, right, zoom, scrub | `0x60`–`0x65` |
| User switch 1, 2 (Assignable 1, 2) | `0x66`, `0x67` |
| Fader touch 1–8, master | `0x68`–`0x70` |
| SMPTE, beats, rude solo LEDs | `0x71`–`0x73` |

## 4. Control change map

| Message direction | CC | Control | Values |
|---|---|---|---|
| → | `0x10`–`0x17`, ch 1 | VPOT turn 1–8 | 1–63 clockwise, 65–127 counter-clockwise |
| ← | `0x30`–`0x37`, ch 1 | ring 1–8 | bit 6 centre LED, bits 5..4 mode, bits 3..0 position |
| → | `0x3c`, ch 1 | jog wheel | relative, same encoding as a VPOT |
| ← | `0x3c` | jog wheel LED | 127 on, 0 off |

The ring uses mode 0, a single dot, with position 0–11 from the pan. The
centre LED lights at pan 0.

## 5. Pitch bend

| Direction | Channel | Value |
|---|---|---|
| ← | 1–8 | strip fader 1–8, 14-bit, low byte first |
| ← | 9 | master fader, 14-bit |

A fader position is `p = value / 16383`, which goes through the fader curve of
the OSC specification. Only the host writes these messages; a surface reports
a moved fader with a touch message and the host echoes the position.

## 6. Connection handshake

SysEx header: `f0 00 00 66 14`, closed by `f7`.

| Code | Direction | Payload | Meaning |
|---|---|---|---|
| `0x00` | ← | — | device query |
| `0x01` | → | serial[7], challenge[4] | host connection query, also sent at power-on |
| `0x02` | ← | serial[7], response[4] | host connection reply |
| `0x03` | → | serial[7] | host connection confirmation |
| `0x04` | → | serial[7] | host connection error |
| `0x0f` | ← | `0x7f` | go offline |
| `0x13` | ← | `0x00` | firmware version request |
| `0x14` | → | version[5] | version reply |

The response bytes derive from the challenge `c[0..4]`:

```text
r[0] = 0x7f & (c[0] + (c[1] ^ 0x0a) - c[3])
r[1] = 0x7f & ((c[2] >> 4) ^ (c[0] + c[3]))
r[2] = 0x7f & ((c[3] - (c[2] << 2)) ^ (c[0] | c[1]))
r[3] = 0x7f & (c[1] - c[2] + (0xf0 ^ (c[3] << 4)))
```

The host sends the device query when the port opens and answers a connection
query with the reply. A surface that skips the handshake is online on its
first control message. On close the host sends go-offline.

## 7. LCD SysEx

`f0 00 00 66 14 12 <offset> <text> f7`.

| Field | Value |
|---|---|
| Offset | 0–55 top row, 56–111 bottom row |
| Text | ASCII, at most 112 bytes per message |
| Full layout | eight 7-character names on top, eight 7-character values below |
| Brief layout | row and first channel number at offset 0 |

## 8. Mapping onto the mixer

### 8.1 Surface mapping

| Surface control | TotalControl action |
|---|---|
| Strip faders 1–8 | volume of the window channel, through the fader curve |
| Master fader | Main Out pair volume |
| VPOT turn | pan of the window channel; balance on outputs |
| VPOT push | pan = centre |
| MUTE 1–8 | channel mute toggle |
| SOLO 1–8 | channel solo toggle |
| SELECT 1–8 | ad-hoc fader-group selection |
| REC 1–8 | select the submix under the strip |
| BANK LEFT / RIGHT, ARROW LEFT / RIGHT | move the window eight strips |
| CHANNEL LEFT / RIGHT, REWIND / FAST FORWARD | move the window one strip |
| ARROW UP / DOWN, Assignable 1 / 2 | move one row |
| EQ | mute the Main Out pair |
| PLUGINS / INSERT | solo the Main Out pair |
| STOP | Dim |
| PLAY | Talkback |
| PAN | Mono |
| FLIP | Speaker B |
| DYN / INSTRUMENT, SEL + DYN | Trim Gains mode |
| RECORD | Recall |
| F1–F8 | load snapshot 1–8 |
| F9 | select the Main Out and cue it |
| F10–F12 | cue Phones 1–3, device dependent |
| Fader touch | carried, no action |
| Transport, zoom, scrub, save, undo, shift, assignment buttons | ignored |

A linked stereo pair is one strip and moves together. With "Map Stereo to
2 Mono Channels" each channel is its own strip.

### 8.2 View

MIDI remote control always operates in Submix view: the faders edit the mix of
the selected submix, even when the GUI is in Free mode. The remote submix
follows the GUI selection, and selecting a submix on the surface selects it in
the GUI.

### 8.3 Window marking

Channels in the window show their name field in the remote tint. Channels
hidden by the channel layout are skipped.

## 9. Simple CC and note commands

These work from any controller, with or without a Mackie surface. The notes
are the same notes as the surface buttons and are part of this protocol.

| Direction | Command | Action |
|---|---|---|
| → | CC 7, ch 1 | Main Out volume |
| → | CC 9, ch 1–12 and 14–16 | input gain, value = the gain; inputs are device dependent |
| → | CC 102–117, ch 1–4 | row 1 faders |
| → | CC 102–117, ch 5–8 | row 2 faders |
| → | CC 102–117, ch 9–12 | row 3 faders |
| → | CC 102, ch 13 | Trim Gains on |
| → | CC 103, ch 13 | Trim Gains off |
| → | CC 104+2n, 105+2n, ch 13 | select submix n |
| → | Note `0x5D` | Dim |
| → | Note `0x2A` | Mono |
| → | Note `0x5E` | Talkback |
| → | Note `0x5F` | Recall |
| → | Note `0x32` | Speaker B |
| → | Note `0x3E` | cue the Main Out |
| → | Notes `0x3F`–`0x42` | cue Phones 1–4 |
| → | Notes `0x36`–`0x3D` | snapshot 1–8 |
| → | Note `0x2D` | Trim Gains |
| → | Note `0x2C` | mute the Main Out pair |
| → | Note `0x2B` | solo the Main Out pair |

The CC value and the pitch-bend value map to the same fader position:
`p = v/127` for 7-bit CC and `p = v/16383` for 14-bit pitch bend. The position
goes through the fader curve of the OSC specification: 104/127 is about 0 dB,
127/127 is +6 dB, 0 is −∞.

## 10. Feedback

| Feedback | Message |
|---|---|
| Fader position | pitch bend, same channels as §5 |
| Button LED | Note On with velocity 127 or 0 |
| Ring | CC `0x30`–`0x37` |
| LCD text | SysEx `0x12` |
| Meter | channel pressure, high nibble strip 0–7, low nibble level |

The meter nibble follows the Mackie scale:

| Level | Nibble |
|---|---|
| 0 dB and above | `0xd` |
| ≥ −2, −4, −6, −8, −10 dB | `0xb`, `0xa`, `0x9`, `0x8`, `0x7` |
| ≥ −14, −20, −30, −40, −50, −60 dB | `0x6`, `0x5`, `0x4`, `0x3`, `0x2`, `0x1` |
| below −60 dB | `0x0` |
| overload set | `0xe` |

Feedback is delta-encoded. The host keeps the last sent fader, LED, ring, LCD
and meter value per remote and emits only the changes. A navigation or submix
change refreshes the whole window. Meters are sent at about 25 Hz while
enabled.

## 11. Host behaviour

| Concern | Behaviour |
|---|---|
| Transport | one worker per enabled port opens the rawmidi pair; input bytes are parsed on the UI thread |
| Parser | running status expanded, SysEx framed, realtime passed through |
| Navigation | per-remote row, window offset, submix and trim state; not part of the workspace |
| Actions | dispatched as the ordinary mixer messages, shared with the GUI and OSC |
| Toggles | resolved against the confirmed model, like OSC |
| Loopback guard | trips on the detection note, disables that remote, logs |
| Background disable | suspends input and feedback while unfocused or hidden |
| Window marking | the name tint of §8.3, per remote |

### 11.1 Loopback detection

While the remote is online with an output port, the host sends `9f 7f 5a`
every 500 ms. Hearing that note on the input means the output is wired back
into the input; the host disables the remote until the user re-enables it.
A surface never sends this note.

### 11.2 Trim mode

In Trim Gains mode the strip faders write channel trim in dB instead of the
send volume. Outputs keep their volume.

## 12. Options list

The options are part of the controller definition,
[`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
§4.6. The settings dialog shows them in file order inside their groups. A
user changes them often, so they are listed here and stored in the map file,
not hard-coded in the dialog.

### 12.1 Mapping

| Key | Label | Form | Default | Behaviour |
|---|---|---|---|---|
| `protocol` | Enable Mackie protocol | checkbox | on | Accept the full Mackie protocol, and the surface buttons of §8. When off, only the simple CC commands of §9 work, and the surface LED, fader, ring, LCD and loopback feedback stops. |
| `submix_linked` | Submix linked | checkbox | on | The remote follows the submix selected in the mixer. `maps_to` `submix_linked`. |

### 12.2 Surface

| Key | Label | Form | Default | Behaviour | Requires |
|---|---|---|---|---|---|
| `full_lcd` | Full LCD support | checkbox | on | Eight names and eight values on the LCD at offsets 0–55 and 56–111. When off, only the row and first channel are written at offset 0. `maps_to` `full_lcd`. | `protocol` |
| `send_names` | Send user defined names | checkbox | on | User labels replace the default channel names in the LCD text. `maps_to` `send_user_names`. | `protocol` |
| `stereo_to_mono` | Map Stereo to 2 Mono Channels | checkbox | off | One strip per channel instead of one per linked pair. `maps_to` `stereo_to_mono`. | `protocol` |
| `submix_any_row` | Submix Selection in Input/Playback Row | checkbox | off | REC selects the submix from any row, not only the output row. `maps_to` `submix_any_row`. | `protocol` |
| `background_disable` | Disable MIDI in background | checkbox | off | Input and feedback are suspended while the window is unfocused or hidden. `maps_to` `background_disable`. | — |

### 12.3 Feedback

| Key | Label | Form | Default | Behaviour | Requires |
|---|---|---|---|---|---|
| `send_levels` | Send level messages | checkbox | off | Send meter data to the surface at about 25 Hz. The `meter` feedback entry is active only while this is on. `maps_to` `send_level_messages`. | — |
| `peak_hold` | Peak hold | checkbox | off | The meter nibble follows the held peak instead of the current level. `maps_to` `peak_hold`. | `send_levels` |

"Send level messages" off is the required state of every other meter source:
the engine sends no meter feedback while the option is off, even if the
surface is online.

### 12.4 Stored values

Each row writes one boolean to the host settings under the controller `id`
and the option `key`,
[`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
§10. A missing value reads as `default`.

### 12.5 Defaults

The defaults reproduce the remote's documented starting state: protocol,
full LCD, user names and submix linked on; the remaining options off. The
map file carries this state in the `default` field of each option entry.

## 13. Device differences

| Aspect | UC | UCX | Babyface-classic |
|---|---|---|---|
| MIDI ports | 2 | 2 | 1 |
| Cue phones in the map | F10–F12, Phones 1–3 | F10–F12, Phones 1–3 | F10–F11, Phones 1–2 |
| Simple-note cue range | Phones 1–4 | Phones 1–4 | Phones 1–2 |
| Input gain CC 9 | inputs 1/2, up to 65 | inputs 1/2, up to 65 | inputs 1/2, up to 60, steps ≥ 3 dB |
| Reverb and echo preset CC | — | `B2 0C`, `B3 0C` (open) | — |

A stand-alone unit acts as the Mackie host itself and is not a TotalControl
surface. TotalControl must not claim to control a unit in that mode.

## 14. Map file

The protocol is described by `Mackie_MIDI.json`. Its anatomy, the loader and
the search order are in
[`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
§5–6.

| Field | Value |
|---|---|
| `id` | `mackie_control` |
| `shape` | `surface` |
| `port.source` | `device`, the open unit's MIDI ports |
| `port.devices` | 2 |
| `controls` | strips, buttons, VPOTs, faders, the simple CC block |
| `feedback` | `note`, `pitch_bend`, `cc`, `channel_pressure`, `sysex` |
| `numeric` | `relative_7bit`, `absolute_7bit`, `absolute_14bit`, `ring_position`, `mackie_meter_nibble`, `ascii`, `constant` |
| `options` | the options of §12 |
| `extra.handshake` | `mackie` |
| loopback | the `loopback` feedback entry, note `9f 7f 5a` on channel 16, every 500 ms |

## 15. Open items

| Status | Item |
|---|---|
| open | Live-surface verification: handshake, LCD, meters and the option interactions |
| open | Extender linking: several surface instances forming one block |
| open | FX user presets via `B2`, `B3` `0C` |
