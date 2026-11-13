# Development tools

This note describes the tools that are not part of the supported user
installation. The supported path is the kernel driver plus hwdep; everything
else here is for development, reverse engineering, comparison and hardware
testing.

## Contents

- [Support status](#support-status)
- [Mock modes](#mock-modes)
- [Environment variables](#environment-variables)
- [Capture and probe tools](#capture-and-probe-tools)
- [Build and test helpers](#build-and-test-helpers)
- [Licensing](#licensing)

## Support status

| Path | Status |
|---|---|
| Kernel driver + hwdep | supported user path |
| Probe and capture programs | development only |
| Mock backends | development and tests |

The manual does not describe these as installation options. They are kept
because they are useful for protocol work, capture sessions and comparisons.

## Mock modes

The mock backends run the complete UI without hardware:

```sh
totalcontrol --mock       # Fireface UCX
totalcontrol --mock-ucx   # Fireface UCX
totalcontrol --mock-uc    # Fireface UC
```

The mocks are used by the unit tests and by headless UI checks.

## Environment variables

All development switches use the `TOTALCONTROL_` prefix:

| Variable | Effect |
|---|---|
| `TOTALCONTROL_DEVICE_SERVICE=0` | Keep the device on the UI thread |
| `TOTALCONTROL_MOCK_MODEL=ucx\|uc` | Select the mock model |
| `TOTALCONTROL_MOCK_CC=1` | Simulate class-compliant mode |
| `TOTALCONTROL_MOCK_DISCONNECT=1` | Simulate an unplugged unit |
| `TOTALCONTROL_NO_CAPTURE=1` | Do not open the capture PCM |
| `TOTALCONTROL_DEFER_METER_TAP=1` | Defer the input meter tap |
| `TOTALCONTROL_METER_DEBUG=1` | Print meter values once per second |
| `TOTALCONTROL_CAPTURE_DUMP=<path>` | Dump captured samples |
| `TOTALCONTROL_MULTIPLE_INSTANCES=1` | Bypass the single-instance guard |
| `TOTALCONTROL_PROBE_STREAM=1` | Enable the stream probe |
| `TOTALCONTROL_PROBE_PLAYER=1` | Enable the probe player |
| `TOTALCONTROL_PROBE_SCENE_ONLY=1` | Apply the probe scene only |

## Capture and probe tools

| Tool | Purpose |
|---|---|
| `scripts/capture-usbmon.sh` | Capture USB traffic |
| `tools/parse_capture.py` | Parse a captured stream |
| `tools/decode-totalmix-capture.py` | Decode TotalMix FX captures |
| `tools/rme-probe.c` | Direct vendor-request probe |
| `scripts/ffuc-counters.c` | Read driver transport counters |
| `tools/uc-loopback-phase-probe.sh` | UC loopback phase probe |
| `tools/uc-rate-probe.sh` | UC rate probe |
| `scripts/uc-latency-stress.py` | UC latency stress run |
| `scripts/ffuc-48k-gate.sh` | 48 kHz transport gate |
| `scripts/mixer-demo.sh` | Legacy mixer demonstration |
| `ucx-state-watch` example | Print changed UCX state words |
| `ucx-dump-diff` example | Request the full dump, change one control, print the changed words |
| `fireface-meterdump` example | Dump raw UC `0x85` meter pages (`fireface-meterdump <card> <seconds>`) |
| `fireface-rawdecode` example | Replay a `fireface-meterdump` capture through the UC parsers (`fireface-rawdecode <capture-file>`) |
| `uc-route-out` example | Route a playback pair to one matrix output pair (`uc-route-out <source> <matrix_channel> [gain_db]`) |
| `uc-loopback` example | Write the UC loopback bank (`uc-loopback <card> <index> [more]`) |
| `ucx-dump-compare` example | Request the full dump and compare it with a saved scene |

Protocol captures and measurements are evidence, not specifications. Record
the device model, transport, firmware and rate with every capture.

## Build and test helpers

From `ui/totalcontrol`:

```sh
cargo test -p totalcontrol-core --lib --features alsa
cargo test -p totalcontrol-gui
cargo test -p totalcontrol-layout
cargo build --workspace
```

## Channel layout preview

`totalcontrol-layout` owns the shared channel template schema and produces
the preview used to review channel layout changes without launching the GUI:

```sh
cd ui/totalcontrol
cargo run -p totalcontrol-layout --bin channel-preview -- /tmp/channel-preview.svg 2
```

The second argument is the zoom.  Open the SVG directly or convert it for a
quick raster review:

```sh
magick /tmp/channel-preview.svg /tmp/channel-preview.png
```

Render a candidate configuration without rebuilding:

```sh
cargo run -p totalcontrol-layout --bin channel-preview -- \
    --config ~/.config/totalcontrol/gui-layout.json \
    /tmp/my-layout.svg 2
```

Write the current built-in defaults as a starting example:

```sh
cargo run -p totalcontrol-layout --bin channel-preview -- \
    --example /tmp/gui-layout.json
```

Render one template on its own with an element state:

```sh
cargo run -p totalcontrol-layout --bin channel-preview -- \
    --focus InputMono --state hover /tmp/focus.svg 2
```

`--state` accepts `rest`, `hover`, `open` or `active`.  The open state draws a
labelled placeholder beside the card, because a panel body is not part of the
layout document.  `--device <model>` resolves the document's per-device
override for that model before rendering, for example
`--device "Fireface UC"`.

The command writes `/tmp/gui-layout.json` and the matching
`gui-layout.schema.json` beside it; the document references that sibling file
through `$schema` for editor validation and completion.  The checked-in files
are [`examples/gui-layout.json`](examples/gui-layout.json) and
[`schema/gui-layout.schema.json`](schema/gui-layout.schema.json).  See
[`spec/TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md`](spec/TOTALCONTROL-CHANNEL-TEMPLATE-SPEC.md)
for the field reference, the validation rules and the change workflow.

`scripts/audio-health.sh` and `scripts/reload-kernel.sh` are transport and
driver helpers. `scripts/install-dkms.sh` is the supported driver installer;
`scripts/install-kernel.sh` is its compatibility wrapper.

`scripts/run-totalcontrol.sh` builds the current GUI sources in the release
profile, stops a running instance of any profile and starts the freshly built
binary. `TOTALCONTROL_BUILD_PROFILE=dev` builds the development binary instead.
A local development desktop entry can point at it so a menu launch never starts
a stale build.

## Licensing

The application license split is described in `ui/totalcontrol/LICENSE`:

- `totalcontrol-core`: MIT
- `totalcontrol-gui`, `totalcontrol-layout` and `totalcontrol-tui`: GPL-3.0-only

The kernel module is GPL-2.0-only. Tools and scripts follow the repository
license unless their own notice says otherwise.
