# Babyface-classic transport specification

This document defines the native-mode USB transport, PCM streams, metering and
hwdep ABI of the Babyface-classic as implemented by `snd-fireface-uc`. The
Babyface Pro FS is a different device; see
[BABYFACE-PRO-PROTOCOL-SPEC.md](BABYFACE-PRO-PROTOCOL-SPEC.md).

## Contents

- [1. Device model](#1-device-model)
- [2. USB transport](#2-usb-transport)
- [3. PCM streams](#3-pcm-streams)
- [4. EP0 requests](#4-ep0-requests)
- [5. Metering](#5-metering)
- [6. hwdep ABI](#6-hwdep-abi)
- [7. Capabilities](#7-capabilities)
- [8. Code map](#8-code-map)

## 1. Device model

| Property | Value |
|---|---|
| Native USB ID | `2a39:3fc7` |
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

The kernel does not interpret mixer, preamp or scene writes. They are raw EP0
requests from userspace.

## 8. Code map

| Concern | Source |
|---|---|
| Device entry, modes, stream engine and mute packing | `kernel/ffuc.c` |
| hwdep INFO, events and vendor ioctl | `kernel/ffuc-hwdep.c` |
| USB-MIDI | `kernel/ffuc-midi.c` |
| Model enum and product ID | `kernel/ffuc-uapi.h`, `kernel/ffuc.h` |
