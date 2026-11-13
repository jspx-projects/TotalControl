# Documentation

TotalControl has one architecture document, a GUI specification, a state and
workspace specification, a shared channel-template schema, one protocol
specification per device, an OSC specification and a user manual.

## Core documents

| Document | Contents |
|---|---|
| [`TOTALCONTROL-ARCHITECTURE.md`](TOTALCONTROL-ARCHITECTURE.md) | runtime layers, device service, data flow, state ownership, driver and latency model |
| [`spec/TOTALCONTROL-GUI-SPEC.md`](spec/TOTALCONTROL-GUI-SPEC.md) | layout, controls, panels, gestures and motion |
| [`spec/TOTALCONTROL-STATE-SPEC.md`](spec/TOTALCONTROL-STATE-SPEC.md) | state ownership, current workspace, autosave, restore, host settings and layout files |
| [`spec/TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md`](spec/TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md) | shared channel-template schema, layout configuration, per-device overrides, invariants and preview |
| [`spec/TOTALCONTROL-OSC-SPEC.md`](spec/TOTALCONTROL-OSC-SPEC.md) | OSC 1.96 and Global OSC transport, address spaces, scales and feedback |
| [`TOTALCONTROL-Manual.md`](TOTALCONTROL-Manual.md) | installation, setup, use and troubleshooting |
| [`DEVELOPMENT-TOOLS.md`](DEVELOPMENT-TOOLS.md) | development backends, capture and probe tools |

## Device protocol specifications

| Document | Device |
|---|---|
| [`spec/FIREFFACE-UCX-PROTOCOL-SPEC.md`](spec/FIREFFACE-UCX-PROTOCOL-SPEC.md) | Fireface UCX |
| [`spec/FIREFFACE-UC-PROTOCOL-SPEC.md`](spec/FIREFFACE-UC-PROTOCOL-SPEC.md) | Fireface UC |
| [`spec/BABYFACE-CLASSIC-PROTOCOL-SPEC.md`](spec/BABYFACE-CLASSIC-PROTOCOL-SPEC.md) | Babyface-classic |

## Layout reference files

| File | Contents |
|---|---|
| [`examples/gui-layout.json`](examples/gui-layout.json) | Complete starting layout document, written from the built-in defaults by `channel-preview --example` |
| [`schema/gui-layout.schema.json`](schema/gui-layout.schema.json) | JSON Schema for the layout document, including the per-device `devices` overrides |

Both files are generated fixtures, not a second source: the compiled defaults
in `totalcontrol-layout` are authoritative, and the layout tests keep the
example and schema in step with them.

Open questions, capture requests and working notes are kept in
`docs/work/`; raw captures are kept in `docs/captures/`.

The specifications are present-tense and describe the current implementation.

RME device manuals are proprietary and are not part of the source
distribution.
