# Digiface USB support

Working note, not part of the specification. It records what is known about
the RME Digiface USB and how to add mixer control to TotalControl.

| File | Purpose | Destination once reviewed |
|---|---|---|
| [`DIGIFACE_USB_PROTOCOL_SPEC.md`](DIGIFACE_USB_PROTOCOL_SPEC.md) | Wire protocol and host binding | `docs/spec/DIGIFACE_USB_PROTOCOL_SPEC.md` |
| [`IMPLEMENTATION.md`](IMPLEMENTATION.md) | Kernel module, backend, profile, tests | Working note; the specification outcome lands in the protocol spec and the device profile spec |
| [`CAPTURE-REQUEST.md`](CAPTURE-REQUEST.md) | The evidence the device owner must provide | Working note, shareable as-is |
| [`SOURCES.md`](SOURCES.md) | Sources and the MIT licence position | `THIRD-PARTY.md` entries and code comments |

This work belongs to the person who owns the hardware. With the captures of
[`CAPTURE-REQUEST.md`](CAPTURE-REQUEST.md), the protocol spec and the
implementation plan are enough to implement, test and open a pull request from
their own tree.

## The distinction that matters

The internal mixer answers in both USB modes. What differs is the USB
declaration and the control path a third-party host can use.

| Mode | USB declaration | Control path for a third-party host |
|---|---|---|
| Normal | vendor-specific | a vendor interface with control requests and bulk mixer words |
| CC | class compliant | none documented; RME's iPad application is a private protocol over USB |

Two control models differ. A native control interface is a USB interface
described by the device, so a host can capture and drive it. A vendor protocol
is an application-level protocol over a class transport, here USB MIDI, so a
host has to learn the messages from captured application traffic.

RME uses the vendor model for the iPad. The mixer answering there shows the
hardware runs in CC mode. It leaves the CC-mode control interface unsettled for
a host, and the CC-mode descriptor capture is what settles it.

## Status

| Item | State |
|---|---|
| Normal-mode audio | Verified in mainline `sound/usb` |
| CC-mode audio | Verified in the vendor manual and mainline quirk |
| Mixer answers in CC mode | RME forum: the iPad application reaches it |
| CC-mode control interface for a host | Unknown; needs a capture |
| Mixer interface and endpoints | Public prior art, not verified here |
| Control requests and register bits | Public prior art, not verified here |
| Matrix word format | Public prior art, not verified here |
| Gain law | Public prior art, confirmed by the mainline unity write |
| Meter frame layout | Public prior art, not verified here |
| Loopback request | Conflict between prior art and mainline; needs a capture |
| Snapshot storage | Unknown; needs a capture |
| Device profile | Not written |

## Recommendation

Use a thin control-only kernel module that binds the mixer interface and
exposes one hwdep node. Keep audio on `snd-usb-audio`. Keep the mixer model,
routing policy and scenes in userspace.

This follows the repository's support policy. A direct-USB userspace backend
is a useful capture and prototype tool, but it is not part of the supported
set.

Do not start the kernel work before the captures in
[`CAPTURE-REQUEST.md`](CAPTURE-REQUEST.md) confirm the prior-art protocol.

## What the device is

- 16 micro-optical ADAT inputs at 1x: four optical ports of eight channels,
  each port also usable as stereo S/PDIF.
- 34 outputs at 1x: the same four optical ports plus one stereo Phones output.
- 34 software playback channels at 1x.
- TotalMix FX with routing, faders, mutes, pans, groups, snapshots and
  loopback.
- No microphone preamps, no instrument inputs, no EQ, no dynamics and no
  reverb or echo. The manual states that FX is not available on this model.
- Channel counts fall with the sample-rate family because ADAT uses S/MUX.

## Open items

| Status | Item |
|---|---|
| needs-capture | Normal-mode descriptors for interface 1 |
| needs-capture | TotalMix operation capture: routing, fader, mute, pan, clock, format, snapshot, loopback |
| needs-capture | Meter frames and rate |
| needs-capture | Loopback request semantics |
| open | Channel topology at 2x and 4x |
| open | Snapshot and setup storage in the device |
| open | Whether the mixer interface can be claimed while audio streams |
