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
- [Bench verification tooling](#bench-verification-tooling)
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
totalcontrol --mock-classic  # Babyface-classic
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
| `tools/decode-totalmix-capture.py` | Decode host application captures |
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
| `tools/fireface-hwdep.c` | Unified hwdep access: counters, words, `dump`, `loopback`, `loopback-setup` |
| `tools/alsa-latency-probe.c` | PCM loopback measurement: impulse latency + `pattern` sample-exact mode |

Protocol captures and measurements are evidence, not specifications. Record
the device model, transport, firmware and rate with every capture.

## Bench verification tooling

`tools/bench/` holds the reusable verification and measurement set for the
driver and the host audio path: the PipeWire graph lab (`pw-lab.py`
run/matrix/watch), device-clock writes (`clock-write.c`), hwdep diagnostics,
host/ALSA fill and latency probes, soak/load/recovery tests, and the RTL /
loopback measurement tools. The generic layer is device-agnostic (`--card N` /
`CARD=N`); the protocol layer dispatches per model. See `tools/bench/README.md`
for the tool list, build flags and runtime dependencies, and
`tools/bench/RTL-MEASUREMENT.md` / `CHANNEL-NUMBERING.md` for the measurement
method. Measurement data is not committed; the tools write to a directory you
choose.

## Loopback measurement (UCX)

Reproducible procedure for routing a playback pair to an output and capturing
it back, either through the device's internal loopback or an external cable.
`fireface-hwdep loopback-setup` performs the device configuration in one call;
`alsa-latency-probe` performs the measurement.

Build: `gcc -O2 -Ikernel -o fireface-hwdep tools/fireface-hwdep.c` and
`gcc -O2 -o alsa-latency-probe tools/alsa-latency-probe.c -lasound -lm -lpthread`.

| Step | Command | Why |
|---|---|---|
| 1. Close the GUI | stop `totalcontrol-gui` | One shared hwdep event FIFO: a second reader steals dump/state events, and the GUI's scene reapply overwrites routing and mutes mid-measurement. |
| 2. Free the PCM | `pactl set-card-profile <card> off` | A direct-ALSA tool needs `hw:` open; IF1 writes are bulk transfers with a 100 ms timeout that race ISO audio traffic, so they land only while the PCM is idle. |
| 3. Configure | `fireface-hwdep loopback-setup AN5/6 pb1` | Zeroes the output submix, writes unity matrix coefficients, sets output volume 0 dB, **unmutes the capture inputs**, enables internal loopback. Add `--cable` for an external cable (loopback flag off). |
| 4a. Internal, sample-exact | `alsa-latency-probe pattern hw:0,0 48000 2 1 2 5 6 18` | Digital tap: bit-exact comparison is meaningful. Reports exact / Q23-truncated / zero / other per channel. |
| 4b. Cable, latency | `alsa-latency-probe hw:0,0 48000 1024 48 0 4 -1 18 0.5` | Impulse-train phase gives the analog round-trip latency. |
| 5. Restore | `pactl set-card-profile <card> pro-audio` | Returns the card to the graph. |

`loopback-setup <out> <src>`: `<out>` is `AN1/2 AN3/4 AN5/6 PH7/8 SPDIF
ADAT1/2 ADAT3/4 ADAT5/6 ADAT7/8`; `<src>` is `pb1..pb17` or `in1..in7`
(N feeds the left side, N+1 the right).

Channel ordering: TotalControl's internal output pairs are Phones-first
(pair 0 = Phones 7/8); the UCX DSP channels are AN-first (dsp 0 = AN1/2,
dsp 6 = Phones 7/8). `loopback-setup` works in DSP/hardware numbers, not the
internal pair index. The loopback capture input mirrors the DSP channel for
the analog outputs only (AN1/2 → in1/2, AN5/6 → in5/6, verified); for
Phones/SPDIF/ADAT the capture channel is unverified — scan the capture
channels rather than assuming a number.

Verified failure causes, each baked into the procedure above:

- **Muted capture input** is the usual silent-capture cause. The input word
  `NN00` bit 0 is mute; `loopback-setup` clears it. A silent capture otherwise
  looks identical to an absent cable.
- **Matrix coefficients are write-only live DSP** (`0x4000 + dsp*0x40 + src`,
  unity coarse `0x9000`). The state dump's `NNa0` words are fader-derived and
  do not reflect them; verify routing by audio, not by the dump.
- **Internal loopback quantizes to Q23** (23 fractional bits, truncation toward
  zero, max error 2^-23 ≈ −138 dBFS). About half of full-scale F32 samples lose
  one mantissa bit; this is a device property, present on every output pair.
  The host path (ALSA FLOAT_LE ↔ driver ↔ wire) is a bit-exact word copy.
- **A cable path is not sample-exact.** The converters add fractional-sample
  group delay and noise; use the impulse mode, not the pattern mode, for it.
- **The startup gate mutes the first ≤256 ms.** The pattern mode skips that
  window and aligns on several anchor frames.

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
driver helpers. `scripts/bench-reload.sh` is the bench module swap: it stops
the user audio graph, asserts no `/dev/snd` holder, reloads the built module
(unbinding the USB interface if the card holds a reference), and restores the
graph with `--restore`; `--build` builds first. `reload-kernel.sh` delegates
to it. `scripts/install-dkms.sh` is the supported driver installer;
`scripts/install-kernel.sh` is its compatibility wrapper.

## Lab helpers (installed, password-free)

The bench has three root-owned helpers in `/usr/local/sbin`, each with a
scoped `NOPASSWD` sudoers rule so they run without a password prompt. They
are the preferred way to recover or reload the unit; reach for them before
raw `sudo rmmod`/`insmod`. Source copies live in [`scripts/lab/`](../scripts/lab/).

| Helper | Usage | Purpose |
|---|---|---|
| `reset-fireface-usb` | `sudo -n reset-fireface-usb` | USB port reset (`usbreset`) without unplugging. Finds the unit by product string, waits for the card to reappear in `/proc/asound/cards`. Use when the card vanished or the interface will not re-bind. |
| `reload-fireface-uc` | `sudo -n reload-fireface-uc` | Full module reload: discovers the newest `snd-fireface-uc.ko` (prefers `/usr/local/lib/ffuc-lab/`, else the newest build under any checkout), refuses a world-writable module, unbinds interfaces, `rmmod` + `insmod`, prints cards and hwdep nodes. If `rmmod` fails (the module is still referenced), it re-binds the interfaces it unbound, reports the holders (`refcnt`, `fuser`) and exits non-zero — the unit keeps working on the loaded module. A true swap while the audio graph holds the device needs `scripts/bench-reload.sh`, which stops the graph first. |
| `ffuc-lab` | `sudo -n ffuc-lab stats` / `ringtrace` / `pause <ms>` / `double <0\|1>` / `kill` | Driver debugfs surface. `stats` reads the transport counters; `ringtrace` reads the playback ring-fill trace (one record per URB completion). `pause`/`double`/`kill` probe the UC meter pump. Fixed subcommands only; `stats` and `ringtrace` are read-only. |

Recovery sequence when the card disappears or a reload fails: stop the user
audio graph (`systemctl --user stop wireplumber pipewire-pulse pipewire
pipewire.socket pipewire-pulse.socket`), then `sudo -n reset-fireface-usb`,
then `sudo -n reload-fireface-uc`. The ALSA card can hold a module reference
until the USB interface disconnects, so a reset is often needed before
`rmmod` succeeds.

`scripts/bench-reload.sh` automates the stop/assert/reload/restore sequence
for a checkout build and prefers `sudo -n` when the scoped rule is present.

### Scoped sudoers rule

The helpers and the raw bench commands are covered by
`/etc/sudoers.d/ffuc-bench` (0440 root:root). It grants **no general root** —
only these commands:

```
Cmnd_Alias FFUC_BENCH = /usr/bin/rmmod snd_fireface_uc, \
    /usr/bin/modprobe snd-pcm, \
    /usr/bin/dmesg, \
    /usr/bin/tee /sys/bus/usb/drivers/snd-fireface-uc/unbind, \
    /usr/bin/insmod <checkout>/kernel/snd-fireface-uc.ko *

<user> ALL=(root) NOPASSWD: FFUC_BENCH
```

`<checkout>` is the absolute checkout path and `<user>` the bench user; the
`insmod` line is pinned to that module so it cannot load arbitrary modules.
Install with `sudo visudo -f /etc/sudoers.d/ffuc-bench` (validate syntax
first with `visudo -cf`). The three `/usr/local/sbin` helpers have their own
`NOPASSWD` lines. `bench-reload.sh` probes with `sudo -n dmesg` (a covered
command) to decide whether to use plain `sudo` or the `ksshaskpass` GUI
fallback.

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
