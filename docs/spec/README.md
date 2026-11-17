# TotalControl specifications

This directory contains the device and GUI specifications. Runtime structure
is defined once in [`../TOTALCONTROL-ARCHITECTURE.md`](../TOTALCONTROL-ARCHITECTURE.md);
these documents extend it with device-specific and visible detail.

References point downward only: a device protocol specification describes the
device wire protocol and names no application behaviour; the GUI, state,
profile and OSC specifications own the host behaviour and reference the
protocol specs for the wire detail. See the layering rule in
[`../README.md`](../README.md).

| Document | Contents |
|---|---|
| [DRIVER-DESIGN.md](DRIVER-DESIGN.md) | kernel transport contract, URB engines, hwdep ABI, quirks, recovery and driver extension |
| [TOTALCONTROL-GUI-SPEC.md](TOTALCONTROL-GUI-SPEC.md) | layout, strip controls, panels, gestures, motion and Control Room |
| [TOTALCONTROL-STATE-SPEC.md](TOTALCONTROL-STATE-SPEC.md) | state ownership, current workspace, autosave, restore, host settings and layout files |
| [TOTALCONTROL-DEVICE-PROFILE-SPEC.md](TOTALCONTROL-DEVICE-PROFILE-SPEC.md) | profile table, selection, host operations, capabilities and scene compatibility |
| [TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md](TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md) | channel-template schema, layout document, per-device overrides, invariants and preview |
| [TOTALCONTROL-OSC-SPEC.md](TOTALCONTROL-OSC-SPEC.md) | OSC 1.96 and Global OSC transport, address spaces, scales, feedback and mapping |
| [TOTALCONTROL-MIDI-SPEC.md](TOTALCONTROL-MIDI-SPEC.md) | USB-MIDI transport, rawmidi surface and limits |
| [TOTALCONTROL-CONTROLLER-SPEC.md](TOTALCONTROL-CONTROLLER-SPEC.md) | MIDI controller engine, definition-file format, discovery, settings and persistence |
| [midi-controllers/MACKIE_MIDI.md](midi-controllers/MACKIE_MIDI.md) | Mackie Control wire protocol, mixer mapping, feedback and options |
| [midi-controllers/RME_ARC_USB_MIDI.md](midi-controllers/RME_ARC_USB_MIDI.md) | RME ARC USB wire protocol, key assignment, encoder, LEDs and options |
| [FIREFFACE-UCX-PROTOCOL-SPEC.md](FIREFFACE-UCX-PROTOCOL-SPEC.md) | Fireface UCX transport, registers, writes and readback |
| [FIREFFACE-UC-PROTOCOL-SPEC.md](FIREFFACE-UC-PROTOCOL-SPEC.md) | Fireface UC transport and controls |
| [BABYFACE-CLASSIC-PROTOCOL-SPEC.md](BABYFACE-CLASSIC-PROTOCOL-SPEC.md) | Babyface-classic transport, channel model, mixer writes, preamps, metering and hwdep ABI (experimental) |

The end-user manual is
[`../TOTALCONTROL-Manual.md`](../TOTALCONTROL-Manual.md). Development-only
backends and tools are in
[`../DEVELOPMENT-TOOLS.md`](../DEVELOPMENT-TOOLS.md). Open questions and
capture requests are kept in `docs/work/`.
