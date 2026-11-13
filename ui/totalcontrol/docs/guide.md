# TotalControl guide

The details behind the [README](../README.md): views, background mode, how
TotalControl fits into the Linux audio stack, the workspace layout and the
project status.

The full end-user manual lives at
[`../../../docs/TOTALCONTROL-Manual.md`](../../../docs/TOTALCONTROL-Manual.md).

- [Views](#views)
- [Running in the background](#running-in-the-background)
- [The Linux audio stack](#the-linux-audio-stack)
- [Workspace](#workspace)
- [Status](#status)
- [Building from source](#building-from-source)
- [License](#license)

## Views

| Quick | Matrix |
|:---:|:---:|
| <img src="screenshots/quick.png" alt="Quick view"> | <img src="screenshots/matrix.png" alt="Matrix view"> |

- **Quick**: one source into one destination, the fastest way to a mix.
- **Mixer**: every strip, TotalMix-style, on three rows (hardware inputs,
  software playback, hardware outputs) or two on a wide window.
- **Matrix**: the full routing grid.

`totalcontrol --view quick|mixer|matrix` opens a given view. `--mock`,
`--mock-ucx` and `--mock-uc` run against a simulated Fireface UCX or UC, no
hardware needed. Each simulation carries exactly the panels its unit has: the
UCX shows EQ, dynamics, FX and the Control Room, while the UC shows the gated
set. Both are listed in the Device picker.

## Running in the background

With a system tray (KDE Plasma, or GNOME with the AppIndicator extension),
closing the window keeps TotalControl running, as TotalMix does on Windows: the
front-panel DIM button and the restore of your mix after a replug are
TotalControl's job while it runs. The tray icon toggles the window; its menu has
Dim, Recall mix 1-8, Settings and Quit. `totalcontrol --minimized` starts
hidden, and Settings (Ctrl+,) can start TotalControl with your session. Without
a tray, closing the window quits. One TotalControl runs per session: a second
launch shows the first.

## The Linux audio stack

The kernel driver ([`kernel/`](../../../kernel/)) is the production path. It
exposes a real ALSA card (PipeWire sink/source) and one hwdep control node per
card. TotalControl drives the mixer through that node; PipeWire and the DAW use
the PCM device.

The WirePlumber rule in `tools/alsa/wireplumber-uc.conf` selects the 18-channel
Pro Audio profile and keeps ALSA scheduling polled.

## Workspace

- **`totalcontrol-core`**: the device model and control API (`RmeDevice`
  trait), the Fireface UC/UCX control path, the Babyface-classic control path,
  scenes, workspaces and the mock.
- **`totalcontrol-gui`** (iced) and **`totalcontrol-tui`** (ratatui): the
  interfaces. Custom canvas faders, VU meters with real ballistics,
  multi-select, live routing matrix.
- **`totalcontrol-layout`**: channel templates, the layout document and the
  preview.
- **`kernel/`**: the Fireface UC/UCX and Babyface-classic USB transport,
  ALSA PCM and hwdep node.

## Status

- The supported devices are the Fireface UC, Fireface UCX and Babyface-classic.
  All three use the in-tree kernel driver and the hwdep control node.
- The UC and UCX control paths are hardware-validated, including preamps,
  routing, EQ, dynamics, FX and device meters.
- The Babyface-classic path is hardware-validated for the channel model,
  mixer writes, preamps and metering.
- The GUI and TUI are functionally complete; the road to 1.0 is validation on
  real hardware and desktops (tray on KDE/GNOME), see
  [`CHANGELOG.md`](../CHANGELOG.md).

## Building from source

```sh
cargo build --release          # the whole workspace
cargo test                     # protocol laws, panel state machine, GUI state, ...
```

The binaries land in `target/release/`: `totalcontrol-gui` and
`totalcontrol-tui`. The RPM installs the GUI as `totalcontrol`; rename or
symlink `totalcontrol-gui` if a source build should start from the shipped
desktop entry.

## License

`totalcontrol-core` is MIT. `totalcontrol-gui`, `totalcontrol-layout` and
`totalcontrol-tui` are GPL-3.0-only. The kernel module is GPL-2.0-only. See
[`LICENSE`](../LICENSE) and [`COPYING`](../COPYING).
