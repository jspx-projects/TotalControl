# TotalControl specifications

This directory contains the device and GUI specifications. Runtime structure
is defined once in [`../TOTALCONTROL-ARCHITECTURE.md`](../TOTALCONTROL-ARCHITECTURE.md);
these documents extend it with device-specific and visible detail.

| Document | Contents |
|---|---|
| [TOTALCONTROL-GUI-SPEC.md](TOTALCONTROL-GUI-SPEC.md) | layout, strip controls, panels, gestures, motion and Control Room |
| [TOTALCONTROL-STATE-SPEC.md](TOTALCONTROL-STATE-SPEC.md) | state ownership, current workspace, autosave, restore, host settings and layout files |
| [TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md](TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md) | channel-template schema, layout document, per-device overrides, invariants and preview |
| [TOTALCONTROL-OSC-SPEC.md](TOTALCONTROL-OSC-SPEC.md) | TotalMix FX OSC transport, pages, scales, feedback and mapping |
| [FIREFFACE-UCX-PROTOCOL-SPEC.md](FIREFFACE-UCX-PROTOCOL-SPEC.md) | Fireface UCX transport, registers, writes and readback |
| [FIREFFACE-UC-PROTOCOL-SPEC.md](FIREFFACE-UC-PROTOCOL-SPEC.md) | Fireface UC transport and controls |
| [BABYFACE-PRO-PROTOCOL-SPEC.md](BABYFACE-PRO-PROTOCOL-SPEC.md) | Babyface Pro FS channel model, ALSA surface, USB protocol and DSP blocks |
| [BABYFACE-CLASSIC-PROTOCOL-SPEC.md](BABYFACE-CLASSIC-PROTOCOL-SPEC.md) | Babyface-classic transport, PCM streams, metering and hwdep ABI |

The end-user manual is
[`../TOTALCONTROL-Manual.md`](../TOTALCONTROL-Manual.md). Development-only
backends and tools are in
[`../DEVELOPMENT-TOOLS.md`](../DEVELOPMENT-TOOLS.md). Open questions and
capture requests are kept in `docs/work/`.
