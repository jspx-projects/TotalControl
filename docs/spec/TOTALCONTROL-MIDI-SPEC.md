# TotalControl MIDI transport specification

This document defines the USB-MIDI transport the kernel driver exposes and
the ALSA rawmidi surface built on it. It is generic across devices; the device
protocol documents own the per-device interface and endpoint facts and refer
here for the transport.

The control surfaces that run on this transport are specified separately:

| Surface | Document | Map file |
|---|---|---|
| Mackie Control | [`midi-controllers/MACKIE_MIDI.md`](midi-controllers/MACKIE_MIDI.md) | `Mackie_MIDI.json` |
| RME ARC USB | [`midi-controllers/RME_ARC_USB_MIDI.md`](midi-controllers/RME_ARC_USB_MIDI.md) | `RME_ARC_USB_MIDI.json` |

Further controller documents join this directory with their tested-support
pull requests.

The controller engine and the definition-file format are in
[`TOTALCONTROL-CONTROLLER-SPEC.md`](TOTALCONTROL-CONTROLLER-SPEC.md). OSC
remote control is a separate surface with its own document
([`TOTALCONTROL-OSC-SPEC.md`](TOTALCONTROL-OSC-SPEC.md)); both share the mixer
model and the fader curve.

| Part | Status |
|---|---|
| USB-MIDI transport on the Fireface UC | implemented, verified on the wire |
| USB-MIDI transport on the Fireface UCX | implemented, verified |
| USB-MIDI transport on the Babyface-classic | implemented, not verified |
| Two-port routing | implemented |
| ALSA rawmidi surface | implemented |

Not supported:

| Capability | Reason |
|---|---|
| UMP and MIDI 2.0 | not implemented |
| Clock generation | the driver carries bytes, it does not pace them |
| Messages larger than 128 bytes | the driver sends one 128-byte chunk per transfer, and a split message is lost |

## Contents

- [1. Terms](#1-terms)
- [2. USB transport](#2-usb-transport)
- [3. Device matrix and class compliance](#3-device-matrix-and-class-compliance)
- [4. ALSA rawmidi surface](#4-alsa-rawmidi-surface)
- [5. Outbound messages](#5-outbound-messages)
- [6. Inbound messages](#6-inbound-messages)
- [7. Limits](#7-limits)
- [8. Open items](#8-open-items)
- [9. Code map](#9-code-map)

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

The high nibble selects the port. RME's Windows driver sends the payload
length in the low nibble instead: `0xf` for one byte, `0x2` for two and `0x3`
for three. The UC accepts both forms. This driver sends the code index
number.

On the UC, the unit parses each bulk transfer as a separate MIDI stream. A
message must lie within one bulk transfer. A SysEx that starts in one transfer
and continues in the next loses the continuation bytes.

## 3. Device matrix and class compliance

| Model | IF2 class | Ports | Port source |
|---|---|---|---|
| Fireface UC, PC personality | vendor-specific `ff/00/00` | 2 | device profile |
| Fireface UC, AP personality | not captured | 2 expected | not captured |
| Fireface UCX | Audio, MIDI streaming `01/03` | 2 | class endpoint descriptor |
| Babyface-classic | not captured | 1 | device profile |

The port count comes from the class-specific endpoint descriptor when the
interface declares one: each embedded jack is one port. The UCX declares two
embedded IN jacks and two embedded OUT jacks, `bNumEmbMIDIJack = 2` on both
endpoints. A vendor-specific interface has no such descriptor and takes the
count from the device profile. The driver exposes at most two ports.

Class compliance has two layers:

| Layer | UC PC | UC AP | UCX | Babyface |
|---|---|---|---|---|
| Wire format | USB-MIDI 1.0, verified | assumed | USB-MIDI 1.0 | USB-MIDI 1.0 |
| USB declaration | vendor-specific | class compliant per RME documentation | class compliant, captured | not captured |

RME states in the UC product sheet and manual that the UC's MIDI ports are
class compliant under Mac OS, which is the AP personality. The UCX manual
states the same for normal mode. The Babyface manual states it for the
Babyface; its CC mode (firmware 200 and later, Select+Recall at power-on) is
fully class compliant including SysEx-capable MIDI.

The UCX declaration is verified in normal mode, not only in CC mode: the
saved descriptor capture shows the vendor-specific audio interface 0 and the
serial-bearing product string of normal mode, while interface 2 already
declares `01/03` with full MIDIStreaming descriptors. CC mode re-enumerates
the whole unit with class descriptors; MIDI stays class compliant in both
modes. The UC has the older split design and the UCX unified it.

Host binding on Linux follows from the declaration:

| Interface declaration | `snd-usb-audio` behaviour |
|---|---|
| MIDI streaming subclass 3 | binds the interface, creates one rawmidi device per embedded jack |
| vendor-specific, subclass 0 (UC PC) | skips the interface; only this driver exposes MIDI |

The driver claims IF2 on every model. It binds the audio interface first and
claims IF2 during probe, before the class driver is tried on it, so one card
carries the audio and the MIDI ports. When `snd-usb-audio` already holds IF2,
the driver logs `no MIDI port` and continues without MIDI; the ports remain
available on the class driver's card.

In CC mode the whole unit re-enumerates with class descriptors and belongs to
`snd-usb-audio`; this driver does not bind class interfaces.

The device MIDI ports are offered to the controller engine as
`port.source: device`. A controller on its own class card, like the ARC USB,
uses its own discovery.

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

The input URBs run only while an input substream is open. A bulk IN URB that
completes with an error is submitted again, so the input stream does not stop
on a transient error.

The output work takes one chunk from the rawmidi buffer per poll and writes it
as one bulk transfer. Data is acknowledged when the transfer completes. On a
failed transfer the driver logs a warning, clears a stalled endpoint and
drops the chunk, so a writer cannot block on data that the unit refused.

The driver creates rawmidi devices only. The ALSA core also registers the card
as a sequencer client with one port for each MIDI port.

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

Running status is expanded: every packet carries its status byte. A data byte
after a completed channel message reuses the previous status. `f4` and `f5`
are undefined and are dropped.

A partial message that has produced no packet is held in the port state and is
sent when it completes. Bytes already written in an earlier transfer cannot be
continued; see §7.

## 6. Inbound messages

The driver decodes every packet by its code index number and passes the
payload bytes to the rawmidi device for the port in the high nibble. A packet
for a port that does not exist is dropped. A code with no payload length is
dropped.

SysEx is delivered as a byte stream. The client sees the `f0` and `f7` bytes
and decides where a message begins and ends.

## 7. Limits

| Limit | Value |
|---|---|
| Message per transfer | the unit loses a continuation that arrives in a later transfer |
| Output chunk | 128 packets, 512 bytes |
| Input URB | 512 bytes |
| Output poll | 1 ms |

A caller must write a message in one piece that fits one output chunk. A
message written in several pieces, or larger than the chunk, is split across
transfers and the unit drops the continuation. A message larger than 128 bytes
cannot be sent whole. Every controller feedback message of the map files is
below this limit.

## 8. Open items

| Status | Item |
|---|---|
| open | UC AP personality: interface 2 class and MIDI behaviour |
| open | Babyface-classic: normal-mode interface 2 class and jack descriptors |
| open | UCX wire verification of the class-codec path |
| open | UC port 2 physical output identification |

## 9. Code map

| Concern | Source |
|---|---|
| USB-MIDI transport, port routing, packet codec | `kernel/ff-midi.c` |
| Device MIDI fields and quirk bits | `kernel/ff.h` |
| MIDI interface, endpoints and port count per model | `kernel/ff.c` |
| UC MIDI wire format | `FIREFFACE-UC-PROTOCOL-SPEC.md` §11.3 |
| UCX MIDI wire format | `FIREFFACE-UCX-PROTOCOL-SPEC.md` §2.5 |
| Babyface MIDI transport | `BABYFACE-CLASSIC-PROTOCOL-SPEC.md` §2 |
| Rawmidi worker and port enumeration | `ui/totalcontrol/totalcontrol-gui/src/midi.rs` |
| Controller engine and definition format | `TOTALCONTROL-CONTROLLER-SPEC.md` |
