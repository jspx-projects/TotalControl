# Babyface-classic protocol specification

Support status: experimental.

This document defines the native-mode USB transport, PCM streams, channel
model, mixer writes, preamps, metering and hwdep ABI of the Babyface-classic as
implemented by `snd-fireface-uc` and the `BabyfaceClassic` backend.

## Contents

- [1. Device model](#1-device-model)
- [2. USB transport](#2-usb-transport)
- [3. PCM streams](#3-pcm-streams)
- [4. EP0 requests](#4-ep0-requests)
- [5. Metering](#5-metering)
- [6. hwdep ABI](#6-hwdep-abi)
- [7. Capabilities](#7-capabilities)
- [8. Code map](#8-code-map)
- [9. Channel model](#9-channel-model)
- [10. Mixer writes](#10-mixer-writes)
- [11. Preamps and settings](#11-preamps-and-settings)
- [12. State ownership](#12-state-ownership)

## 1. Device model

| Property | Value |
|---|---|
| Native USB IDs | `0424:3fc7`, `2a39:3fc7` |
| Tested firmware | 226 |
| Default transport | isochronous interface 0 |
| 1× rates | 32, 44.1, 48 kHz |
| 2× rates | 64, 88.2, 96 kHz |
| 4× rates | 128, 176.4, 192 kHz |
| Capture words per frame | 12 / 8 / 6 at 1× / 2× / 4× |
| Playback words per frame | 14 / 10 / 8 at 1× / 2× / 4× |
| ALSA capture channels | 12 at every rate |
| ALSA playback channels | 14 at every rate |
| ALSA card ID | `Babyface` |

The ALSA card ID stays within the 15-character card-ID limit. The longer
`Babyface-classic` name is used for the card and PCM names.

The unit has no mixer readback. The kernel exposes the transport and raw EP0
vendor requests; TotalControl owns the mixer model and writes it through the
hwdep node.

## 2. USB transport

| Interface | Endpoints | Type | Use |
|---|---|---|---|
| 0 | `0x03` OUT, `0x84` IN | isochronous | audio, UCX packet format |
| 1 | `0x85` IN | bulk | DSP meter packets (§5) |
| 1 | `0x0a` OUT | bulk | DSP EQ blocks; not used by the kernel |
| 2 | `0x07` OUT, `0x86` IN | bulk | USB-MIDI |

The audio interface uses the UCX packet format and the UCX stream open/close
words. The alternate settings are:

| Rate family | Alternate setting | Capture words | Playback words |
|---|---|---|---|
| 1× | 1 | 12 | 14 |
| 2× | 2 | 8 | 10 |
| 4× | 3 | 6 | 8 |

At 2× and 4× the USB packet carries fewer words than the ALSA stream. The
kernel maps the words that are present and leaves the remaining ALSA channels
silent, so the logical channel count stays constant across rates.

## 3. PCM streams

The ALSA PCM device is duplex:

- capture exposes 12 channels;
- playback exposes 14 channels.

Capture carries 12 words at 1×, 8 at 2× and 6 at 4×. Playback sends 14, 10 and
8 words respectively. The additional logical playback channels are the two
host-reverb returns; the additional logical capture channels are the FX send
bus.

The kernel does not create ALSA mixer controls. Mixer writes are EP0 vendor
requests sent through the hwdep vendor ioctl.

## 4. EP0 requests

The control writes in this section use `bmRequestType 0x40` with no data
stage.

| Request | Direction | Use |
|---|---|---|
| `0x10` | OUT | Settings, rate family and stream open/close words |
| `0x13` | OUT | Playback mute mask |
| `0x14` | OUT | Playback unmute mask |
| `0x16` | OUT | Channel initialization |
| `0x1d` | OUT | Stream start word |
| `0x20` | OUT | Global enable |

### 4.1 Initialization

The driver sends 18 channel-initialization slot pairs with request `0x16`,
then `0x10` `wValue 0x0800` / `wIndex 0xb800`, then `0x20` `wValue 0x0001` /
`wIndex 0x0000`.

### 4.2 Rate

The rate family is written with request `0x10`, `wIndex 0x0030`; bits 7..4 of
`wValue` carry the family:

| Family | Rates |
|---|---|
| 0 | 32, 64, 128 kHz |
| 1 | 44.1, 88.2, 176.4 kHz |
| 2 | 48, 96, 192 kHz |

The Babyface-classic follows the host stream rate; the driver does not write a
separate rate commit word for the isochronous transport.

### 4.3 Stream start and stop

Stream start sends:

1. `0x10` `wValue 0x8000` / `wIndex 0x8000`;
2. `0x1d` `wValue 0x0000` / `wIndex 0x0000`;
3. `0x14` with the playback unmute mask.

Stream stop sends `0x13` with the playback mute mask.

### 4.4 Playback mute mask

Requests `0x13` and `0x14` take an 18-bit mask packed like a `0x12` level:

```text
wValue = mask >> 2
wIndex = (mask & 3) << 14
```

Channel *n* is bit *n*. The driver sends all 18 bits at stream start and stop:
`wValue 0xffff`, `wIndex 0xc000`.

This packing differs from the UC/UCX mask, where channel *n* is bit *n* + 2.

## 5. Metering

The unit streams meter packets on bulk IN `0x85` without a request. The kernel
delivers each packet as an `FFUC_EV_METER` hwdep event. A packet is 456 bytes:

| Bytes | Contents |
|---|---|
| 0..303 | 38 little-endian `u64` mean-square values |
| 304..455 | 38 little-endian `u32` peak values |

| Slot | Channel |
|---|---|
| 0..9 | hardware inputs |
| 10..23 | playback 1..14 |
| 24..37 | outputs 0..13 |

Peak and power use the same scaling as the other Fireface meter streams:

```text
peak  dBFS = 20 · log10(peak  / 2^27)
power dBFS = 10 · log10(power / 2^55)
```

The kernel meter buffer is 512 bytes; each event carries the actual packet
length.

## 6. hwdep ABI

The device reports model value `3` (`FFUC_MODEL_BABYFACE_CLASSIC`) in the
hwdep INFO block. The transport is isochronous (`FFUC_XPORT_ISO`).

| Field | Value |
|---|---|
| `channels` | 12 |
| `inputs` | 12 |
| `outputs` | 14 |
| `playback` | 14 |
| `matrix_sources` | 26 |
| `meter_bytes` | 512 |
| `state_ep` | 0 |
| `meter_ep` | `0x85` |
| `status_ep` | 0 |

`inputs` and `channels` include the two FX-send capture channels. `outputs` and
`playback` include the two host-reverb returns. The hwdep node exposes the
vendor-request ioctl, the meter event queue and the transport counters.

## 7. Capabilities

| Feature | Babyface-classic |
|---|---|
| Native-mode isochronous PCM | yes |
| Capture channels | 12 |
| Playback channels | 14 |
| USB-MIDI | yes |
| DSP meters on `0x85` | yes |
| State endpoint | no |
| Status endpoint | no |
| ALSA mixer controls | no |
| Raw EP0 vendor requests through hwdep | yes |
| Matrix pair routing, faders, mute, solo | yes |
| Per-cell mono matrix | no |
| Mic gain, 6–60 dB in 3 dB steps | inputs 1/2 |
| 48 V | inputs 1/2 |
| PAD, Inst, Level, AutoSet | no |
| Phase, output balance, stereo split | no |
| Loopback | no |
| EQ, Low Cut, dynamics, AutoLevel, FX | no |
| Dim, Mono, Speaker B, Cue, clock source | no |

The kernel does not interpret mixer, preamp or scene writes. They are raw EP0
requests from userspace.

## 8. Code map

| Concern | Source |
|---|---|
| Device entry, modes, stream engine and mute packing | `kernel/ffuc.c` |
| hwdep INFO, events and vendor ioctl | `kernel/ffuc-hwdep.c` |
| USB-MIDI | `kernel/ffuc-midi.c` |
| Model enum and product ID | `kernel/ffuc-uapi.h`, `kernel/ffuc.h` |
| Mixer model, EP0 writes, preamps and meters | `ui/totalcontrol/totalcontrol-core/src/babyface_classic.rs` |
| Channel topology | `ui/totalcontrol/totalcontrol-core/src/profiles/babyface_classic.rs` |
| Device menu and backend selection | `ui/totalcontrol/totalcontrol-gui/src/app.rs` |

## 9. Channel model

| Index | Inputs | Outputs |
|---|---|---|
| 0, 1 | AN1, AN2 (mic/line) | AN1, AN2 (XLR) |
| 2, 3 | AS1, AS2 (optical ADAT 1/2 or S/PDIF) | PH3, PH4 (phones) |
| 4, 5 | ADAT3, ADAT4 | AS1, AS2 |
| 6..9 | ADAT5..ADAT8 | ADAT3..ADAT6 |
| 10, 11 | — | ADAT7, ADAT8 |
| 12, 13 | — | FX send bus L/R |

Playback channels 1..12 feed the matrix. Playback 13/14 are the return of the
host-side reverb; they do not enter the DSP, and the crosspoints `src` 22/23
have no effect. Capture channels 0..9 carry the inputs; capture 10/11 carry the
FX send bus.

The profile groups the 10 inputs and 12 outputs into the stereo pairs AN1/2,
PH3/4, AS1/2, ADAT3/4, ADAT5/6 and ADAT7/8.

## 10. Mixer writes

Request `0x12` writes one matrix crosspoint or one output master.

| Target | Address |
|---|---|
| crosspoint `src` → output `out` | `24 · out + src` |
| output master `out` | `0x3e0 + out` |

| `src` | Source |
|---|---|
| 0..9 | hardware inputs (§9) |
| 10..21 | playback 1..12 |

The level is an 18-bit linear gain, `level = gain · 0x8000` (`0x8000` =
0 dB, `0x10000` = +6 dB, 0 = off):

```text
wValue = level >> 2
wIndex = ((level & 3) << 14) | address
```

The unit powers up with no playback routed. The backend writes nothing when
it opens the device. The first full write covers the whole matrix, all output
masters, both mic gains and both 48 V bits. It happens when the saved scene is
applied, or when metering starts if there is no saved scene. If a saved scene
cannot be applied, the backend writes a silent model instead of a default mix.
The saved scene includes 48 V, so a scene saved with 48 V on switches it on at
connect.

Without a saved scene, the model is:

- playback pair *n* → output pair *n* at 0 dB, centred;
- every input crosspoint at off;
- every output master at 0 dB;
- mic gain 6 dB, 48 V off.

Mic inputs are mono sources panned into both sides of a pair. ADAT inputs and
playback channels feed only their own side; their pan acts as a balance. Mute,
solo and the output masters are applied host-side by writing 0 or the scaled
level.

Request `0x15` selects loopback: `wValue` 1 = on, 0 = off, `wIndex` = capture
channel 0..9. With loopback on, the capture channel records the output of the
same index (post-master). The backend keeps loopback off.

## 11. Preamps and settings

| Control | Request | `wValue` | `wIndex` |
|---|---|---|---|
| Mic gain AN1/AN2 | `0x1a` | 0..18 | 0, 1 |
| 48 V | `0x17` | bit mask to set | bit mask to change |

- Mic gain: `gain dB = 6 + 3 · wValue`, 6..60 dB. The unit has no gain
  readback. The backend snaps the model to the 3 dB grid and writes the gain
  at the first full write and on every change.
- 48 V: bit 0 = AN1, bit 1 = AN2. The register is write-only. The backend
  writes one bit at a time with the matching mask, so the other bits are
  unchanged, at the first full write and on every change.
- Sample rate: set by the host stream. The backend writes no rate request and
  shows the rate from the hwdep INFO block.
- Controls the backend does not implement (Dim, Mono, Speaker B, Cue, clock
  source, EQ and others) return an error.

## 12. State ownership

The unit has no mixer readback. The host owns the mix: the scene model holds
every fader, pan, mute, solo and master, and the backend sends only the `0x12`
addresses whose level changed since the last write.
