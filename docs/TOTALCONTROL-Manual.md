# TotalControl Manual

TotalControl is a control surface for RME Fireface interfaces on Linux. It
replaces the vendor's mix application on Linux: routing, gains, EQ, dynamics,
FX and snapshots, driven through the kernel driver.

This manual is written for the end user. It describes installation, setup
and use. The developer specifications are in [`spec/`](spec/README.md); the
runtime architecture is in
[`TOTALCONTROL-ARCHITECTURE.md`](TOTALCONTROL-ARCHITECTURE.md).

Only the kernel driver plus hwdep path is supported for users. The legacy
ALSA backends, direct USB and capture/probe tools are development tools; see
[`DEVELOPMENT-TOOLS.md`](DEVELOPMENT-TOOLS.md).

Panels and controls appear only where the connected unit has them. Examples
in this manual assume a Fireface UCX, which has the full set: EQ, Dynamics,
FX and the Control Room. On a Fireface UC the same layout applies with the
reduced control set.

## Contents

- [Setup](#setup)
- [1. The mixer window](#1-the-mixer-window)
- [2. Views](#2-views)
- [3. Channel strips](#3-channel-strips)
- [4. How the mixer works: submixes](#4-how-the-mixer-works-submixes)
- [5. Working in stereo](#5-working-in-stereo)
- [6. The slideout panels](#6-the-slideout-panels)
- [7. Equalizer](#7-equalizer)
- [8. Dynamics](#8-dynamics)
- [9. FX: sends, reverb and echo](#9-fx-sends-reverb-and-echo)
- [10. Control Room](#10-control-room)
- [11. Gestures](#11-gestures)
- [12. Workspace and Options menus](#12-workspace-and-options-menus)
- [13. Workspaces, snapshots, layouts, undo and redo](#13-workspaces-snapshots-layouts-undo-and-redo)
- [14. Latency](#14-latency)
- [15. Where TotalControl keeps its data](#15-where-totalcontrol-keeps-its-data)
- [16. Running in the background](#16-running-in-the-background)
- [17. Device settings at startup](#17-device-settings-at-startup)
- [18. The UCX front knob](#18-the-ucx-front-knob)
- [Appendix A — Device modes](#appendix-a--device-modes)
- [Appendix B — PipeWire](#appendix-b--pipewire)
- [Appendix C — MIDI and OSC setup](#appendix-c--midi-and-osc-setup)
- [Appendix D — Command-line options](#appendix-d--command-line-options)

## Setup

### Requirements

- A Linux system with ALSA and PipeWire.
- A supported Fireface in normal vendor mode.
- A kernel with matching headers and DKMS for the driver.
- Rust and the build dependencies for the source or RPM build.

The supported devices are the Fireface UC and Fireface UCX. Babyface-classic
support is experimental: it uses the same kernel driver and covers the controls
described in its protocol specification.

For a device whose audio already works in the standard kernel driver,
TotalControl uses that driver for the audio and adds a small second driver for
the control and meters. The two work together and appear as one device. The
Digiface USB will work this way (in development).

The source and RPM paths below are alternatives.  Use the source path to work
from a checkout; use the RPM path when you want packages that can be installed
and removed with `dnf`.

### Build and run from source

Install the build dependencies.  On Fedora:

```sh
sudo dnf install git cargo rust gcc gcc-c++ make pkgconf-pkg-config \
    alsa-lib-devel mesa-libGL-devel libX11-devel libxkbcommon-devel \
    wayland-devel systemd-devel fontconfig-devel freetype-devel \
    openssl-devel dkms kernel-devel
```

On another distribution, install the equivalent C/C++ toolchain, pkg-config,
Rust, ALSA, X11 and Wayland, fontconfig, FreeType, OpenSSL and udev
development packages, plus DKMS and the headers for the running kernel.

Clone the repository and build the GUI and TUI:

```sh
git clone https://github.com/jspx-projects/TotalControl.git
cd TotalControl
cargo build --release --locked \
    --manifest-path ui/totalcontrol/Cargo.toml \
    -p totalcontrol-gui -p totalcontrol-tui
```

Install the kernel driver from the same checkout:

```sh
./scripts/install-dkms.sh
```

The installer registers `snd-fireface-uc` with DKMS, builds it for the running
kernel and enables the load service. Keep the kernel headers installed so the
module is rebuilt after a kernel update.

Check the installation:

```sh
dkms status
modinfo -k "$(uname -r)" snd-fireface-uc
```

If the module is missing after a kernel update:

```sh
sudo dnf install kernel-devel-"$(uname -r)"
sudo dkms autoinstall -k "$(uname -r)"
sudo modprobe snd-fireface-uc
```

The GUI and TUI binaries are ready in `ui/totalcontrol/target/release/`; see
[Start TotalControl](#start-totalcontrol) below.

### Create RPM packages

Prebuilt Fedora packages are published on the repository's
[GitHub releases](https://github.com/jspx-projects/TotalControl/releases) page
when a release is available for your Fedora version.  Download and install the
two RPMs to try TotalControl without building them; use the instructions below
when no package exists for your Fedora release.

The repository contains two Fedora spec files and a build helper in
[`packaging/fedora/`](../packaging/fedora/README.md).  They produce the
`totalcontrol` application package and the separate `totalcontrol-kmod` DKMS
package.  The kernel module is separate because it has to be rebuilt for each
kernel.

Build the RPMs on the Fedora release where you will install them.  Install the
extra packaging tools:

```sh
sudo dnf install rpm-build desktop-file-utils
```

From the repository root:

```sh
./packaging/fedora/build-rpms.sh
```

The script uses the current commit as its source archive and writes the RPMs
to `packaging/fedora/out/`.  The main packages are the application RPM and
`totalcontrol-kmod`; the build also produces debuginfo and debugsource
sidecars.  The helper requires the build dependencies from the source section.

Install the kernel module first, then the application:

```sh
sudo dnf install ./packaging/fedora/out/totalcontrol-kmod-*.rpm
sudo dnf install ./packaging/fedora/out/totalcontrol-[0-9]*.rpm
```

The `totalcontrol-kmod` package builds the module for the running kernel
during installation.  If it cannot build, install the matching headers and
retry:

```sh
sudo dnf install kernel-devel-"$(uname -r)"
sudo dkms autoinstall -k "$(uname -r)"
```

The source installer and `totalcontrol-kmod` both register the same DKMS
module.  Use one of the two driver paths on a machine.  If the source
installer was used earlier, remove its DKMS entry before installing the RPM:

```sh
sudo dkms remove -m snd-fireface-uc -v 0.1 --all
sudo rm -f /etc/systemd/system/snd-fireface-uc-load.service
sudo systemctl daemon-reload
```

These are convenience packages, not an official Fedora repository: they are
not signed, are not updated by `dnf`, and should be rebuilt after a Fedora
release upgrade.  With UEFI Secure Boot enabled, sign the DKMS module and
enrol the signing key in MOK before the kernel will load it.

### Start TotalControl

From the source build:

```sh
./ui/totalcontrol/target/release/totalcontrol-gui
```

After installing the RPMs, start the application from the desktop menu or
from a terminal:

```sh
totalcontrol
```

The window opens on the Mixer view. Quick and Matrix are in the top bar.

### Desktop menu entry

The `totalcontrol` RPM installs the desktop entry and icons into the system
paths.  If you are running a source build instead, install the entry for the
current user after building:

```sh
cd <checkout>
install -Dm644 ui/totalcontrol/totalcontrol-gui/assets/totalcontrol.desktop \
  ~/.local/share/applications/totalcontrol.desktop
install -Dm644 ui/totalcontrol/totalcontrol-gui/assets/totalcontrol.svg \
  ~/.local/share/icons/hicolor/scalable/apps/totalcontrol.svg
install -Dm644 ui/totalcontrol/totalcontrol-gui/assets/totalcontrol-256.png \
  ~/.local/share/icons/hicolor/256x256/apps/totalcontrol.png
```

The entry runs `totalcontrol` from `PATH`. If your binary is somewhere else,
edit the `Exec=` line to its full path, for example:

```sh
sed -i "s|^Exec=totalcontrol|Exec=$HOME/.local/bin/totalcontrol|" \
  ~/.local/share/applications/totalcontrol.desktop
```

Refresh the desktop caches:

```sh
update-desktop-database ~/.local/share/applications
kbuildsycoca6                 # KDE
gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor   # GNOME/GTK
```

The menu entry appears in the KDE application launcher and the GNOME
Activities overview. The tray icon is separate from the menu entry: KDE
shows it through StatusNotifierItem, while GNOME needs a StatusNotifierItem
or AppIndicator extension.

### Check the device

1. Open the device card in the sidebar.
2. Confirm the model, sample rate and buffer.
3. Confirm that the input meters move when a source is connected.

From a terminal, `aplay -l` lists the card and
`ls -l /dev/snd/hwC*D*` lists the hwdep nodes. Each MIDI port appears as one
ALSA rawmidi device; `amidi -l` lists them. The Fireface UC and UCX expose
two MIDI ports.

If no device appears, check that the unit is in normal vendor mode, that the
kernel module is loaded, and that another application is not holding the
device.

The **Device** selector at the top lists every detected unit (by model and
serial); with several units connected, choose the one to control. Each unit
keeps its own workspace and settings. If the active unit is unplugged or
claimed by another system, TotalControl saves its workspace and switches to
the next available unit, or to a simulation if none is usable; the unit
returns automatically when it reappears. A unit whose driver is too old for
this TotalControl build is shown with a red "!" — hover for "incompatible
driver". TotalControl then falls back to the next unit or a simulation;
updating the driver (or TotalControl) restores the unit. This is why the
mixer may occasionally show a simulation instead of your hardware.

### Low-latency host tuning

TotalControl can hold the latency constraint for you: enable `Keep the audio
CPUs awake while streaming` in `Options > TotalControl`, and choose the `Max
sleep time`: `0 µs` forbids every sleep state (the strongest protection, the
highest idle power), `10 µs` allows the shallowest sleep states (most of the
protection, less power). The setting is off by default, takes effect while the
card streams, and costs idle power only during a session. The installed package grants the logged-in user access to
`/dev/cpu_dma_latency` the same way it grants the sound devices; for sessions
without a seat (remote or headless) the `audio` group is the fallback. The
manual commands below do the same without the application.

At the smallest quanta the host's power management decides how quickly the
audio thread wakes. In timer mode the thread sleeps between graph cycles; a
CPU that enters a deep idle state pays the exit on every wake. On our desk,
limiting the idle states dropped the audio thread's runnable-wait from about
43 to about 15 ms/s. Options, in order of effect:

- Limit the idle states of the audio CPUs, persistently:
  `sudo cpupower idle-set -D 1`, or add the kernel arguments
  `intel_idle.max_cstate=1 processor.max_cstate=1`.
- For a critical session, run the performance governor:
  `sudo cpupower frequency-set -g performance`.
- Without root (group `realtime`): hold a small latency constraint by writing
  the value in microseconds to `/dev/cpu_dma_latency` and keeping the file
  open, for example with a script that opens it, writes `50`, and stays
  running while you record. This forbids only the deepest states; it helps
  less than the first two options.

Keep virtual machines, heavy disk IO and file indexers away from the audio
CPU. The audio thread runs at real-time priority, so ordinary CPU load does
not displace it; interrupts, deep idle exits and memory-system pressure do.
These measures are tolerances, not guarantees: at 48 frames and 48 kHz the
buffer margin is only a few milliseconds. In our measurement, keeping the
audio CPUs out of deep idle turned a 48 frames/48 kHz session with
occasional dropouts into fifteen minutes with none. Pinning the audio thread
to one CPU on its own made no difference; the idle-state limit is the
effective measure.

## 1. The mixer window

The mixer shows three rows of channel strips:

1. **Hardware Inputs** — the physical inputs of your interface.
2. **Software Playback** — the channels your applications play into.
3. **Hardware Outputs** — the physical outputs.

Every row carries an uppercase header. A **Control Room** section closes the
Hardware Outputs row; it holds your monitoring destinations (see
[section 10](#10-control-room)).

The right sidebar holds the device card (rate, buffer), Undo/Redo, global
mute and solo, the FX knob, group and snapshot controls and layout. It
stays visible as the window narrows and can be collapsed.

When the channels do not all fit, one slider at the lower right scrolls all
three rows together, so the routing picture stays aligned.

The device card in the sidebar reads `Internal, <sample rate>, <buffer>` and
opens the Audio Configuration panel immediately left of the CONTROLS sidebar,
which carries clock source, sample rate, pitch, the digital input formats, the
word clock options and firmware and serial information.

`Word clock in` switches the word-clock input termination. The option appears
only on devices that support software termination. If it is not shown, your
unit sets termination with a switch on the back panel. The UC exists in two
hardware revisions: the earlier has a recessed mechanical switch (with a 75 Ω
LED) and no software control; the later revision supports the software setting.
On the UCX, termination is always software-controlled. TotalControl reads the
setting from the unit when it connects.

The panel separates what the device owns from what the host engine drives. In
the device group, `Clock source` picks one of the unit's own clock options
(Internal, SPDIF coax., Optical, Word clock). `Sample rate` shows the device
rate: an editable picker of the device-supported rates when the clock is
Internal and `Rate follows` is Device, and a read-only readout of the rate the
unit reports when `Rate follows` is PipeWire, or the clock source is external.

The engine `Rate follows` control chooses who determines the rate. It is a pair
of radio buttons, usable only when the clock source is Internal:

- **PipeWire** (the default) lets the sound server's graph conduct; the device
  follows whatever rate the graph runs at. When nothing is playing, the graph
  switches to the rate of the next stream that starts, so a DVD plays at 48 kHz
  and a CD started afterwards plays at 44.1 kHz, each at its native rate. A DAW
  that sets its own rate conducts the graph in this mode.
- **Device** uses the rate you set in the device group's Sample rate picker and
  holds it; the graph follows it. Use it for a fixed rate, or when the device is
  slaved to an external clock (see below). TotalControl re-asserts the rate if
  PipeWire restarts or another application changes it.

The device has one clock, so the graph has one rate. While a stream is already
playing, a new stream at a different rate is resampled to the graph rate — two
applications cannot play at different native rates at the same time. PipeWire
switches the graph rate when the device is idle (between tracks); it does not
interrupt a live stream to change rate. If a browser or another application
holds a live stream at one rate, a new track at another rate is resampled until
that stream stops.

Whatever the authority, the **device rate is the hub**. The authority sets the
device rate, and the graph rate and the available quantum both derive from it:

```text
authority (Device / PipeWire) -> device rate -> graph rate + quantum
```

The device rate always wins; a stale graph request never overrides it. The
quantum picker offers only quanta at or above the device rate's period floor
(one URB), so an impossible quantum cannot be selected. The `Sample rate` row
shows the device rate, and the `Graph` row shows the graph rate that follows it.

Rate and quantum changes apply over a few seconds while playback continues (the
player is never stopped). Until a change takes effect, the affected value is
orange, and a toast at the bottom of the window announces the change when it is
requested. The `Sample rate` row confirms against the rate the unit itself
reports; the `Graph` row confirms against the running stream; the quantum
listbox's selected value is orange until the graph runs at the requested
quantum and then returns to its normal colour. There is no separate "applying"
message to wait on. The graph quantum is one ALSA period in polled scheduling
and two periods under timer scheduling, so a request of 128 is confirmed when
the card runs 64-frame periods.

When the device is slaved to an external clock, the rate is determined by the
device and `Rate follows` reads `Device` (read-only). TotalControl follows the
rate the unit reports at the selected clock input and forces the graph to it,
so a DAW and every application run at the external clock's rate.

The `Graph` line always shows the rate PipeWire is running at, in every mode.
A rate change in PipeWire mode is a runtime change, so it does not add an undo
step or mark the workspace dirty. The chosen mode is remembered in the
application preferences, not in a workspace.

The `Pitch` slider changes the speed and pitch of the whole interface from
-5.0 % to +5.0 %. Drag or scroll in 0.1 % steps. Hold Shift for 0.01 % steps.
Double-click the slider to return to 0.0 %. Pitch is written to the device
as soon as it changes.

### About dialog

Click the TotalControl mark and wordmark in the top bar to open About. It
names the version and the build identifier, the device TotalControl is
driving and that device's state, links to the repository and lists the
gestures and keys the window answers to.

The build identifier looks like `921a5ec-20261112T150445Z`: the git commit the
build came from, an optional `dirty` marker for a build with uncommitted
changes, and the UTC time of the build. Quote it when you report a problem,
and run `totalcontrol-gui --build-id` to print the identifier of the installed
binary without opening the window.

### Graphics and window size

TotalControl draws the mixer window on the GPU. A redraw updates the whole
window, so the window's size sets the cost of a frame rather than the part
that changed.

The VU meters animate at 15 frames per second while a level moves; the
window redraws at twice that rate while you drag a control or a panel is
animating, so interaction stays smooth.  A held peak still appears. Channel strips,
faders, knobs and meters are drawn with crisp edges rather than multisampling,
which keeps the window's share of the GPU small enough for the desktop
compositor to keep its frame rate on a shared or integrated GPU. Text is
unaffected.

On a small integrated GPU driving a 4K display, a window that covers most of
the screen leaves the least headroom. If the desktop stutters while
TotalControl is open, make the window smaller first; closing it to the tray
stops the drawing entirely (see
[16. Running in the background](#16-running-in-the-background)).

## 2. Views

TotalControl offers three views:

- **Quick**: one source into one destination — the fastest way to build a
  basic mix. A linked input pair appears once with its pair name; a split
  pair appears as its two mono channels. The destination list follows the
  output link state the same way, and a split output selects the chosen
  side. The two strips render and behave like their Mixer counterparts.
- **Mixer**: all strips on three rows, like the host application.
- **Matrix**: the complete routing grid; drag or scroll a cell to change its
  value, and double-click it to toggle between off and unity. The `Outputs`
  row edits each hardware output channel's own master volume.

These are also the tabs in the top bar, next to the `OPTIONS` menu.

## 3. Channel strips

A channel strip shows, left to right:

- the **channel name** (click to select the channel, double-click to rename);
- a **submix label box** on inputs and playback channels; it selects the bus
  being written (see [section 4](#4-how-the-mixer-works-submixes));
- **mute** and **solo** (outputs use mute and **cue**);
- two **strength readouts** showing the current left and right channel level;
- a **level fader** with a meter beside it;
- a column of small buttons: `T` (trim), the settings gear, `EQ` and `D`
  (dynamics), when the unit provides them;
- a **thin FX fader** at the top of that column, when the unit has an FX bus.

The `T`, gear, `EQ` and `D` tabs show only their glyphs at rest. Hovering
gives them a slightly darker face; an open slideout keeps a dark face behind
its tab.

The strength readouts refresh four times a second; the meter bars keep moving
smoothly. A readout shows one decimal place, except for levels of magnitude
100 dB or more, which are printed without a fraction (`100`, `120`, `-150`).
Silence — and anything below the unit's -150.5 dBFS floor — reads `-∞`.

Mute silences a channel without moving its fader; unmuting restores
the same level. Solo silences the other inputs and software playbacks while
the soloed channel keeps its fader; releasing the last solo restores the
others. The Cue button on a hardware output copies that output's submix into
the selected `Cue/PFL to` target, so you can listen to any hardware output
through the monitor output. Cue is exclusive; releasing it restores the target
mix. An engaged Mute button is outlined blue, Solo yellow and Cue green; the
button face stays dark.

### The masters

The sidebar's `M`, `S`/`F` and `F` buttons are the three masters, each drawn
in the colour of the channel control it stands for: mute blue, solo yellow and
the fader group's own colour. They are the same size as the sidebar's other
buttons. A master that is switched off takes a muted version of its colour.

The mute and solo groups build themselves. You do not have to set anything
up: every channel you mute joins the global mute group, and every channel you
solo joins the global solo group. All you need to remember is the button.

`M` is on when TotalControl starts. Press it and the whole mute group switches
off for the moment: the master and every held channel show the pause mark
before the `M`, and the channels become audible again. Press `M` again and the
same channels are muted together. The mute group works on every routing, not
only the submix you are looking at.

The pause mark means the group is being remembered, not cleared. Clicking a
channel that is already in the group toggles it and takes it out of the
group; a linked stereo pair goes with it, and the rest of the group is
unaffected. Muting a channel that is not in the group mutes it as usual and
adds it, even while the group is off.

`S` works the same way for solos over the current submix. In PFL mode the same
button becomes the PFL master: it shows `F` while an input is listening, and
pressing it cancels the active PFL rather than restoring it.

`F` is different: it gangs faders and remembers the group, but has no pause
face. Shift+click a strip to add it to the global fader group; the first
member turns `F` on and gives the member's fader cap the group colour. Keep
Shift+clicking to add or remove channels. A plain click moves the focused
strip and leaves the group alone, so you can work on a channel in the middle
of a group without breaking it. Press `F` off and the members stop moving
together and lose the colour; press it on again and the same group gangs
again. Only level is ganged, never pan.

The group is really three groups in one: input faders gang with input faders,
software playback faders with software playback faders, and hardware output
faders with hardware output faders. One group can span all three rows, and each
row moves on its own. An empty group leaves `F` off. `F` never mutes or solos
anything, and it does not affect the stored fader groups `1`–`4` in the
`groups` card. The pause mark is two short, thick bars, half the label height:
before the `M`, and after the `S`.

Every strip has the same height. A collapse button in the readout row under
the fader folds a strip so only its channel name and meter remain, and the
meter grows to fill the space. If a slideout is open on that channel, it
closes first; the strip narrows after the panel has slid away. The collapsed
meter keeps the normal VU well colour: a linked pair shows two bars, a mono
channel shows one centred bar covering the level text. A dark display face spans the bottom, so
the `+` button sits at the same height as the full strip's collapse button;
the face is reserved for future information.

The fader range is -65 to +6 dB. The lowest cap position is -64.5 dB; the
position below it is -65 dB and reads as inf. The narrow dark channel under
the cap is the **fader slot**. The channel fader and the thin FX fader use
the same slot width; the slot is slightly lighter than the page black. The
fill below the cap is only a little lighter than the channel surface, so it
shows the level without a bright white rail; the cap stays light. Its value
is shown in a dB readout
under the fader, without a unit suffix. Double-clicking that readout opens a
numeric entry field.

Output strips also carry a **balance** knob that pans the output pair
between its two channels; it appears only while the pair is linked.

Each meter has two indicators, following the host application:

- the **RMS bar** has a deliberately slow time constant so it shows average
  loudness. It is green below -3 dB, yellow from -3 dB to 0 dB and red above
  0 dB;
- the **peak line** has zero attack so a single full-scale sample is
  visible; it is held for a fixed two seconds and then decays. Its colour
  follows the same green/yellow/red bands;
- **overload is the red OVR block** at the top of the meter channel. It
  lights after three consecutive full-scale samples (the global default).

Peak hold applies only to the peak line. The RMS bar is not held and keeps
following the RMS signal. The default fall rates follow the host application's own
meters for the device:

| Device | RMS bar release | Peak hold | Peak line release |
|---|---|---|---|
| UC | 25 dB/s | 2 s | 15 dB/s |
| UCX (and devices not yet measured) | 10 dB/s | 2 s | 10 dB/s |

You can change the RMS bar release in **TotalControl settings ▸ VU
FALLBACK**: pick 10, 15, 20, 25, 30 or 40 dB/s. The **Default** button
selects the unit's own measured rate in the list, so you can see what it
is: 25 dB/s on the UC, 10 dB/s on the UCX (and on devices not yet
measured). A slower release holds the bar up longer (a steadier readout); a
faster one follows the music more closely. This affects only the RMS bar —
the peak line keeps its own hold and release. The choice is saved with the
workspace.

The meter scale runs from -65 dB at the bottom to +6 dB at the top. The
whole area above 0 dB is red. The ruler has an unlabelled green tick at
-3 dB, the full-scale-sine RMS target, and a red tick beside the +3 dB
label. The green and red ticks are half the length of the other
graduations. Each tick extends a little under the fader cap; the cap covers
the line at its current level. The 0 dB tick is slightly brighter and is
repeated just to the right of the fader slot. The cap's centre line aligns
with the ticks; the two outer cap ridges are inset and lighter than the
centre line. A level below -65 dB reads as silence and does not draw a peak
line.
If the device disconnects, the meters fall to silence instead of freezing.

Input and playback meters are pre-fader and post-trim: they show the signal
after the channel trim but before its send fader, so moving a send or
changing the selected submix does not change them. Hardware output meters
are post-fader and show the finished bus.

On the Fireface UC the device publishes RMS for all eighteen hardware
outputs: AN1..AN4 from the flag-page bank and AN5/6 through ADAT7/8 from
the data-page header. The device words are measured before the row fader;
TotalControl applies the row's output fader and mute so the displayed
output meter is post-fader. The peak line and the OVR block use the
device's output record for every output row.

On the same unit every input row takes the device input RMS word for the
bar and the input peak record for the peak line; the input RMS and peak
banks cover all eighteen inputs. The device input words are taken after
the channel gain, so TotalControl uses them directly and does not add the
gain again. When the device RMS word is zero for a very low signal, the
bar uses the peak word's sine equivalent (peak / √2). The input OVR block
follows the device's input over word.

The RMS bar is mathematically correct RMS: a full-scale sine reads about
-3 dB on it. TotalControl does not apply an RMS +3 dB display shift.

TotalControl uses a fixed two-second peak hold and a global OVR trigger of
three consecutive full-scale samples. OVR is a sample count, not a dB
threshold.

The meters show the level as it arrives at the interface. If the host
(PipeWire) volume is below 0 dB, a full-scale recording reaches the
interface that much lower and the playback meter reads correspondingly
low. The Audio Configuration panel shows the host volume and has a `0 dB`
button; set it
to unity and use the interface's own faders for monitoring. A linked pair
shows one meter bar per channel; each bar shows that channel's own signal,
so linking controls does not copy a signal from one channel to the other.

The meters are read from the interface's own hardware meter pages
independently of the rest of the window. Moving a fader, opening a panel or scrolling does not interrupt or
freeze them. A short transient lights the peak line even when it falls
between two screen frames. When every meter has been quiet for a moment they
stop redrawing, so an idle mixer stays still; the next signal wakes them
immediately and the held peak still catches a transient shorter than a frame.

## 4. How the mixer works: submixes

The mixer is a set of buses, one per hardware output. Selecting an output
strip makes it the *current submix*. After that, every input and playback
fader edits the **send of that channel into the selected output**, and the
output's own fader is the master level of that bus.

This is the same model as the host application:

- The fader of an input or playback strip never changes what the channel
  sends to your recording or other outputs; it changes how loud that
  channel plays into the currently selected output.
- To send a channel to another output, select that output's strip and set
  the fader again. Every output remembers its own mix independently.
- The meter on an input or playback strip shows the channel after its trim
  and before the sends; the meter on an output shows the finished bus.

A routing exists only while its send level is above −∞. Drag a channel fader
all the way down and that channel's route into the current output is removed;
raise it again and the route comes back. Dragging a Matrix cell to −∞ does the
same for that crosspoint. There is no separate delete action.

## 5. Working in stereo

Input and playback pairs can be linked. When linked:

- the fader shows the louder channel of the pair and moving it shifts both
  channels together, preserving their level difference;
- the pair's balance is visible and adjustable from one knob, and keeps its
  setting at any fader level;
- the gain knob shows both values, and its position and orange arc follow the
  higher gain;
- the knob moves both preamps by the same step while the higher gain has
  range left; the wheel over the top or bottom value moves only that preamp;
- mute, solo, FX send, EQ, low cut and dynamics settings are shared from the
  left channel;
- panel buttons split into a **linked pair**: a phase pair shows `Ø L` and
  `R Ø`, for example.

Splitting a pair sets the two channels hard left and right for panning into
a submix and gives each channel its own fader. Re-linking restores the pair
level and balance you had before the split.

When the current submix is one channel of a split output pair, a linked
pair's balance works on that mono output alone. The two outputs start hard
left and hard right, each carrying its own side of the source; turn the
balance towards the centre on one of them to send both sides there, and the
other keeps its own setting.

Linking the output pair again returns each source to its natural side, with
the pair balance following the two levels. Splitting again starts the mono
outputs hard left and hard right.

When the input and the selected submix are both mono, there is no left/right
position to set: the pan/balance knob disappears and `MONO` is shown in the
same place, so the strip layout does not move. A mono input into a stereo
submix keeps a pan knob, and a linked stereo pair into a mono submix keeps a
balance knob.

Output pairs behave the same way: the balance knob exists only while
linked, and splitting converts the pair level and balance into two
independent channel volumes. A split output's own strip shows `MONO` instead
of the balance knob, whatever the input and playback link states are. A pair
assigned to the Control Room is always stereo and never splits.

An unlinked input fader is mono and writes only that input's channel. A
linked input pair is stereo and its fader writes both channels. The UC's
front-panel volume events use the same mono/stereo distinction.

## 6. The slideout panels

Pressing a symbol button on a strip opens that channel's panel beside it.
The panel opens to the right of the strip and closes when you press the
same button again. There is no extra close button; the tabs never move.

### 6.1 Settings

The gear opens the Settings panel. Its rows follow the channel type, so
every channel of one type shows the same panel shape:

- Inputs 1/2: `stereo`, `48V`, `autoset`, `gain`, `width`, `send FX`,
  monitor processing (`MS proc`) and phase.
- Inputs 3/4: like above, with `inst` and the gain in the instrument/line
  range on the UCX, plus the level reference dropdown for these inputs.
- Inputs 5–8: `stereo`, the reference dropdown, `width`, `send FX`,
  `MS proc` and phase.
- Inputs from channel 9 on: `stereo`, `width`, `send FX`, `MS proc` and
  phase.
- Software playback: like the top inputs but without microphone features.
- Hardware outputs: `stereo`, the reference dropdown, `return FX`, phase,
  `no trim`, `talkback` receive and `loopback`.

Notes:

- **Gain** is the preamp gain of the four front inputs. At connect it is
  written from the saved mix. It follows the stereo link and follows the
  unit's physical encoder when one is turned on the hardware.
- **Width** narrows or widens the pair: `1.00` full stereo, `0.00` mono,
  `-1.00` swapped channels. It is hidden on a mono input pair and on a split
  software-playback pair.
- **MS proc** encodes or decodes the pair as mid/side. It is hidden on a
  mono input pair and on a split software-playback pair.
- **Phase** is the `Ø` button; engaged phase shows green.
- The `ref.` label appears only with the reference dropdown below it. The UC
  has no per-channel reference control, so its channel panels show no `ref.`
  label.
- The **send FX** knob and the strip's thin FX fader are the same control;
  the **return FX** knob on outputs is the return level of the FX bus.
- The **reference dropdown** sets the input or output reference level. For
  inputs 5–8 the four channels move together; inputs 3/4 can be linked.
- `loopback` makes this output's signal available on its input channel, so
  recording software can capture what the output plays. The input meter keeps
  showing the physical input; the loopback signal is only in the record path.
  `no trim` and `talkback` receive are host-side settings.

On/off controls sit at the left of their row. Engaged switches light orange
(`48V`, `stereo`, `autoset`, `inst`, and others) or green (phase).

### 6.2 EQ and Dynamics

The `EQ` and `D` buttons open the equalizer and the dynamics panel. See
[sections 7](#7-equalizer) and [8](#8-dynamics).

### 6.3 Trim

`T` on an input or playback strip is the pre-fader trim of the channel: it
attenuates the channel before all of its sends, whatever output it plays
into. The tab lights while trim is active, and its tooltip shows the value.

At its bottom position the trim makes the routing currently shown reach −∞:
that routing is removed, while the channel's other routings keep their
levels. This is the same rule as moving the send fader itself to the bottom,
and the strip's fader shows the effective level.

## 7. Equalizer

EQ is available on every hardware input and hardware output. The panel shows
the response plot with three coloured segments — red low, green mid, blue
high — built from the three band frequencies. The panel's EQ button is
orange while EQ is enabled.

Three bands, each with gain, frequency and width:

| Parameter | Range |
|---|---|
| Gain | ±20 dB |
| Frequency | 20 Hz – 20 kHz |
| Width (Q) | 0.7–5.0 |

- Band 1 and band 3 can be a bell or a shelf; band 2 is always a bell; band
  3 can instead be a low pass (hi cut).
- The gain knob of each band carries the colour of its segment.
- Every band keeps its frequency even when the band is off, so the plot
  never jumps.

The low-cut row holds the low-cut on/off button, its frequency knob and the
slope buttons (6/12/18/24 dB per octave). Low cut is independent of the main
EQ; you can edit it while it is off, and it is only heard when enabled.

Double-clicking an EQ handle on the plot resets that handle.

## 8. Dynamics

`D` opens the dynamics panel, on hardware inputs and hardware outputs. The
graph is an input-to-output level curve: green where the expander works,
blue in the neutral range, red where the compressor limit applies.

The panel has:

- an on/off switch at the top;
- shared **gain**, **attack** (0–200 ms) and **release** (100–999 ms) used
  by both compressor and expander;
- a **compressor** column: threshold −60…0 dB, ratio 1:1…10:1 (red);
- an **expander** column: threshold −99…−30 dB, ratio 1:1…10:1 (green);
- an **auto level** row: on/off, maximum gain 0–18 dB, headroom 3–12 dB and
  rise time 0.1–9.9 s.

The ON button at the top is orange while dynamics is on. A change to any
knob updates the whole dynamics block of that channel.

## 9. FX: sends, reverb and echo

The `FX` knob in the right sidebar opens the FX column, when the unit has an
FX bus. It lights while the column is out. Every input and playback strip can
send to it:

- the thin fader at the top of the strip (or `send FX` in the Settings
  panel) is the send level; its fader slot fills orange when raised;
- an output strip's thin fader (or `return FX`) is how loud the FX return
  plays on that output; its fader slot fills green when raised.

The column holds **Reverb** and **Echo**, each with an on/off switch and a
type menu. Both carry predelay, room
scale, cutoffs, width and volume for reverb; delay time with a BPM readout,
feedback, high cut and width for echo. The delay and BPM readouts are two
views of one value. The echo block also shows the FX bus level meters.

## 10. Control Room

On the UCX and UC the Control Room section sits at the right of the Hardware
Outputs row. The Babyface-classic omits it until its host-side Control Room
writes are mapped. It turns ordinary hardware outputs into
monitoring destinations with fixed names: `Main`, `Speaker B` and `Phones 1–4`. Every hardware output is either
assigned in the Control Room or remains an ordinary output in the Hardware
Outputs row; there are no empty slots.

### Control Room behaviour

The Control Room assigns ordinary hardware outputs to monitoring roles. It
does not create a new mixer bus.

| Term | Meaning |
|---|---|
| Main Out | The primary engineer monitor output. |
| Speaker B | The alternative engineer monitor output. |
| Phones 1–4 | Performer headphone outputs. |
| Monitor output | Main Out while Speaker B is off; Speaker B while it is on. |
| Monitor action | Dim, Recall, Mono, Talkback, External Input or Mute FX. |
| Talkback receiver | A hardware output whose Talkback receive switch is on. |

The monitor actions apply as follows:

| Action | Main Out | Speaker B | Phones 1–4 |
|---|---|---|---|
| Dim | Yes | Linked to Main only | No |
| Recall | Yes | Linked to Main only | No |
| Speaker B | Switches to Speaker B | Switches to Main | No |
| Mono | Active monitor output | Active monitor output | No |
| Talkback | Listenback path | Talkback receiver only | Talkback path |
| External Input | Yes | No | No |
| Mute FX | Yes | No | No |
| Cue/PFL | Target only | Target only | Target only |

Speaker B is a second playback path. When Speaker B is active, Main Out is
muted and Speaker B is audible. The Speaker B strip's LINK button gangs its
fader to Main: moving either fader moves the other by the same amount and
keeps the relative offset between them. Dim and Recall move both by the same
amount. With the link off, Speaker B keeps its own level and Dim and Recall
act on Main Out only.

Speaker B needs an assigned Main and an assigned Speaker B output. Until both
are assigned, the `speaker B` button stays visible but does nothing, and the
Speaker B entry in the Assign menu stays visible but grayed out. Assign a Main
first, then Speaker B. Clearing the Main assignment also clears Speaker B.

Phones 1–4 are not monitor outputs. Dim, Recall, Mono, External Input and
Mute FX do not affect them. Talkback uses its own Phones Dim amount.

Every Control Room output is a stereo pair. An assigned pair is linked and
cannot be unlinked; assigning a split pair links it. The Settings panel's
`STEREO` control stays engaged for Control Room pairs, and `Set all channels
mono` does not split them. This stereo pair link is separate from the Speaker
B LINK button, which gangs the Speaker B fader to Main.

### What the destinations mean

- **Main** is normally the engineer’s monitor output: the speakers in the
  control room.
- **Phones 1–4** are normally the performers’ headphone outputs. They are
  not the engineer’s headphones unless you assign one of them that way.
- **Speaker B** is an alternative monitor output, for a second pair of
  speakers or a second listening position.
- **Cue/PFL to** chooses where the engineer’s cue or PFL signal is heard.

So the usual studio picture is: the engineer listens on Main, the performers
listen on Phones, and the Control Room decides which physical outputs play
those roles. Assigning a pair moves it out of the ordinary Hardware Outputs
row and gives it the fixed Control Room name.

### The button stack

The buttons are one channel wide, in the host application's order:

- **dim**, **recall**, **speaker B**;
- **mono**, **talkback**, **ext. input**, **mute FX**;
- **settings**, immediately above **assign**.

| Button | Kind | Outputs affected | In the current mix |
|---|---|---|---|
| `dim` | Latching | Main Out; Speaker B if linked | Yes, snapshot and workspace |
| `recall` | Momentary | Main Out; Speaker B if linked | No, momentary |
| `speaker B` | Latching | Switches monitoring between Main and Speaker B | Yes, snapshot and workspace |
| `mono` | Latching | Active monitor output | Yes |
| `talkback` | Toggle | Every Talkback receiver, plus Phones and Main for Listenback | No, momentary; receive flags are in the current mix |
| `ext. input` | Latching | Main Out | Yes, workspace |
| `mute FX` | Latching | Reverb and Echo returns on Main Out | Yes |
| `settings` | Toggles the dialog | — | No |
| `assign` | Opens the menu | The monitoring assignments | No, preferences |

### Dim

Dim needs an assigned Main Out. Without one it leaves the mix unchanged and
never acts on a Phones output.

**Dim** is a latching button. Engaging it saves the current Main Out level,
lowers Main Out by the configured Dim amount and engages the device Dim
state. Turning Dim off restores the saved level. Moving the Main fader while
Dim is engaged changes the level you hear, but not the saved pre-Dim level:
turning Dim off returns to the level saved when Dim was engaged. Changing the
Main Dim amount while Dim is engaged updates the current attenuation
immediately. Reassigning or clearing the Main Out turns Dim off first.

Dim affects Main Out. With the Speaker B LINK engaged, Speaker B follows the
Main fader, Dim and Recall by the same amount, keeping its offset. With the
link off, Speaker B keeps its own level. Dim does not attenuate the Phones
outputs; Talkback has its own Phones Dim amount.

Dim is part of the current mix and is stored in a snapshot or workspace. The
saved scene stores the active state, the pre-Dim speaker levels and the Dim
amount, so loading it restores the same dimmed monitoring state.

### Recall

**Recall** sets the Main Out level to the configured listening volume. It is
momentary: it does not latch and has no lit state after release. When Dim is
engaged, Recall changes the saved pre-Dim level and the current dimmed level
together.

Recall needs an assigned Main Out and never acts on a Phones output. With the
Speaker B LINK engaged, the Speaker B pair moves by the same amount as Main,
keeping its offset; unlinked, it keeps its own level.

### Speaker B

**Speaker B** is a latching button that switches monitoring from Main to the
output assigned to Speaker B. Both the Main Out and Speaker B must be
assigned first. The button lights while Speaker B is the active monitor
path.

Speaker B is part of the current mix and is stored in a snapshot or
workspace. Loading a workspace restores the saved monitor path after the
saved assignments are installed. Clearing the Speaker B assignment also
clears Speaker B and turns the button off.

The Speaker B strip shows a LINK button in place of CUE. Pressing LINK gangs
the Speaker B fader to Main as a fader group: moving either fader moves the
other by the same amount and keeps the relative offset. The FX-return fader
is ganged too, and Dim and Recall move both by the same amount. LINK lights
orange while engaged, like the stereo button. The Speaker B button already
monitors that pair, so the strip has no CUE. Unlinked, Speaker B keeps its own
fader level.

### Mono

**Mono** is a latching monitor check. It acts on exactly one output: the
active monitor output. That is Main Out while Speaker B is off and Speaker B
while it is on.

While Mono is active:

- the left and right signals of the active monitor output are each
  attenuated by 6 dB and then summed;
- the sum is fed to both sides of that output;
- the inactive monitor output keeps its normal stereo mix;
- the Phones outputs and every other hardware output keep their normal stereo
  signal.

A centred signal therefore keeps the same level when Mono is engaged. The
6 dB attenuation on each side prevents the level rise of a direct L+R sum.

The Mono state belongs to the monitor path, not to one assigned pair. If
Speaker B is switched on while Mono is active, Main Out returns to its normal
stereo mix and Speaker B becomes mono. Switching back makes Main Out mono
again and restores Speaker B. Mono stays active until it is switched off.

Mono applies to the signal currently monitored on the active output,
including the External Input pair while External Input is active. It does not
change the Cue/PFL target, Talkback routing or any source levels outside that
monitor output.

Mono is part of the current mix, so it is saved in a snapshot and restored
with it. Without an assigned Main Out there is no active monitor output and
Mono has no effect.

### Talkback and Listenback

Talkback is the engineer’s communication path to the performers. Click
**talkback** to switch it on; the button stays lit while it is active. Click
it again to switch it off. Talkback is momentary: opening a workspace or
restarting TotalControl starts with Talkback off. The per-output receive
switches are part of the current mix and are restored with it.

While Talkback is active:

- the selected Talkback microphone is sent to every output whose Talkback
  receive switch is enabled;
- the other signals on those outputs are attenuated by the Talkback Dim
  amount, so the performer hears the engineer without losing the cue mix
  permanently;
- the FX returns on the affected outputs are silenced, without switching the
  Reverb or Echo processors off;
- Listenback, when configured, sends the recording-room microphone to Main
  and attenuates Main by the Listenback Dim amount.

The Phones outputs are Talkback receivers by default. Any other hardware
output can be added with its Talkback receive switch.

Talkback only rewrites active routes; silent sends are left alone. Switching
Talkback off restores the previous sends and FX-return levels.

The Talkback source, Talkback Dim, Listenback source and Listenback Dim are
set in Control Room Settings. The Talkback receive switch is on each output’s
Settings panel. RME’s Control Room uses both directions; TotalControl exposes
the Talkback button and the receive switches.

### Mute FX

**Mute FX** is a latching control. It silences the Reverb and Echo returns on
Main Out without switching either effect off. Releasing it restores the
previous return levels.

Mute FX saves the Main Out FX-return levels, writes −65 dB, and restores the
exact saved levels when the button is released. Speaker B and Phones are not
part of this path. Mute FX is part of the mixer scene and a snapshot.

### External input

**Ext. input** replaces the Main Out mix with the stereo hardware-input pair
selected in Control Room Settings. It applies the configured gain and
latches the button. Releasing it restores the normal Main Out mix. Speaker B
and the Phones outputs keep their own mixes. The source list contains
hardware inputs only.

The button is available when a source pair is configured and a Main Out is
assigned. Changing the source or gain while External Input is active
re-engages the path with the new values. The active External Input state is
stored in a workspace and restored when that workspace is opened.

### PFL

With `Live Mode, PFL replaces Solo` enabled, the Solo button on every input
strip becomes **PFL**. PFL sends that input pre-fader and at unity to the
selected `Cue/PFL to` output, silences the other sources in that output, and
temporarily mutes that output's FX returns. Releasing the last PFL input
restores the previous submix and FX returns.

`Exclusive Solo/PFL Mode` keeps only one input pair listening at a time. The
Control Strip's global Solo master shows `F` while a PFL input is active; it
turns the active PFL inputs off and can bring them back, like the ordinary
Solo master. Playback and output Solo behave normally. The active PFL inputs
are stored in a workspace and restored when that workspace is opened.

### What is stored where

| State | Stored in |
|---|---|
| Main and Speaker B assignments | TotalControl preferences; also saved in a workspace |
| Phones 1–4 destinations | TotalControl preferences; also saved in a workspace |
| Main Dim amount, Recall level, Talkback/Listenback settings, Speaker B link | TotalControl preferences; also saved in a workspace |
| Dim and Speaker B active state | Current mix (snapshot and workspace) |
| Talkback and Listenback active state | Not stored; starts off |
| External Input and PFL active state | Workspace |
| Mono, Mute FX and the mixer levels | Current mix (snapshot and workspace) |

### Control Room Settings

The Talkback source and Dim, Listenback source and Dim, Recall level, Main Dim,
External Input and Gain, and the Solo/PFL modes are set in the Control Room
Settings panel. Press `settings`, immediately above `assign`; the panel opens
directly above the Control Room — its bottom edge meets the Control Room's top
and its right edge lines up with the Control Room's right — and is not modal.

The panel has four sections:

- **Talkback**: source and Dim.
- **Listenback**: source and Dim.
- **Main Out**: Recall, Dim, External Input and Gain.
- **Solo / PFL Mode**: `Live Mode, PFL replaces Solo` and
  `Exclusive Solo/PFL Mode`.

The source lists start with `None` and then use the hardware input names. The
five level controls cover `-65.0` to `0.0 dB` in `0.5 dB` steps. The settings
are saved per device in TotalControl's own preferences, not in the current mix.

Changes apply as you make them — the panel stays open so you can hear the
effect while you adjust, and the External Input source/gain and the Solo/PFL
modes reach the running device immediately. Pressing `settings` again, `Close`
or `Escape` dismisses the panel; there is nothing to confirm and nothing to
cancel. `default` restores the default values.

### Assigning outputs

`assign` opens the destination menu. The first level lists the destinations
— the same names as the Control Room channels — plus `Cue/PFL to`, which
chooses which output carries your cue/PFL signal and never reassigns
hardware. The second level lists the hardware channels as their predetermined
names (`None`, then every hardware output, current choice in bold). Choosing
`None` clears a destination. Main Out always holds a pair — it defaults to
AN 1/2 and has no `None` — so Speaker B can never be left as the only Control
Room channel.

A hardware output can sit in only one monitoring destination. Assigning it to
a second destination moves it there instead of showing it twice. Clearing a
destination returns the output to the Hardware Outputs row.

Speaker B needs an assigned Main. Without one, the Speaker B entry stays in
the first-level list but is grayed out and opens no second level. Assign Main
first. Clearing Main also clears Speaker B.

Control Room channels are always stereo: an assigned pair is linked and
cannot be unlinked, so the `STEREO` switch in its Settings panel stays
engaged and does nothing. A split pair is linked when it is assigned. `Set
all channels mono` leaves the assigned pairs linked.

Control Room channel names are fixed: they are always `Main`, `Speaker B`
and `Phones 1`–`Phones 4`, regardless of the View → Names setting, and they
cannot be renamed. The hardware channel feeding each destination is listed
by its own hardware name in the second menu level.

The Main and Speaker B assignments are kept with your TotalControl
preferences. They are restored at startup. A snapshot does not change them;
loading a workspace restores the assignments saved in that workspace.

Clicking a Control Room strip — including its name, fader or switches —
selects it as the current submix, like any other output. Control Room
channels are ordinary outputs for routing, so a hardware output stays a
submix even while it is your monitoring destination.

The Control Room buttons (dim, speaker B, mono, and so on) behave like the
front-panel equivalent controls of the unit.

## 11. Gestures

- **Drag** a fader, knob, matrix cell or EQ handle to change it. The drag
  keeps working even when the pointer leaves the control or the window,
  until you release it. The pointer stays ordinary over these controls; the
  resize cursor appears only while a matrix cell is being dragged.
- **Hold Shift** while dragging for fine adjustment. This works on faders,
  knobs, matrix cells and EQ handles.
- **Scroll** over a fader or knob nudges it. On a knob that shows two values,
  scrolling over one value changes only that value; scrolling over the rest
  of the knob changes both.
- **Double-click** a fader, the FX fader, a knob or an EQ handle to reset
  that control; double-click a matrix cell to toggle it between off and
  unity; double-click a channel name to rename it; double-click the dB
  readout to type a value; double-click a strip's background to collapse
  it.
- Ordinary buttons act once per click.
- **Hover** a strip to outline it in silver. The outline follows the pointer
  and is not a persistent selection; it just shows where the next click lands.
- **Shift+click** a strip or its fader adds or removes it from the global
  fader group. The first member turns `F` on and the members' caps take the
  group colour; Shift+drag still fine-adjusts. Press `F` to ungang the members
  and hide the colour without forgetting them, and press it again to bring
  both back. A plain click selects one strip for editing (the cursor keys and
  typed values follow it) and leaves the group untouched; there is no
  Ctrl+click.

## 12. Workspace and Options menus

The top bar has a `WORKSPACE` menu before `OPTIONS`. `WORKSPACE` opens and
saves the complete host-side configuration described in section 13, and
contains the built-in `Factory Workspaces` layouts.

The `OPTIONS` menu contains `Audio Config`, `TotalControl`, `Control Room`
and `Reset Mix`, in that order. `Control Room` is shown for models whose
Control Room path is implemented; the UC and Babyface-classic currently omit it
until their monitoring writes are mapped.

`Reset Mix` lists the standard mixes:

| Entry | Effect |
|---|---|
| Straight playback with all to Main Out | playbacks 1:1 to the outputs, each also mixed to Main; nothing else routed |
| Straight playback | playbacks 1:1 to the outputs, nothing else routed |
| Clear all submixes | every input and playback send set to nothing |
| Clear channel effects | EQ, low cut, dynamics, auto level, reverb, echo and width off |
| Set output volumes | outputs 0 dB; Main and Speaker B −10 dB |
| Reset channel names | user names cleared |
| Set all channels mono/stereo | the stereo or mono mode of every pair |
| Set inputs mono, outputs stereo | the ADM layout |
| Total reset | straight playback to Main, nothing else routed, effects off, output volumes set |

Choosing one rewrites the whole mixer in one go.

`TotalControl` opens the centered TotalControl settings dialog: background
behaviour, startup sync, the virtual surround outputs described in
Appendix B, and the MIDI and OSC remote controls described in Appendix C.
The mixer is dimmed and cannot be used with the mouse or keyboard
until the dialog closes. Independent options are checkboxes; one-of-many choices such as
the startup sync direction and OSC protocol are radio buttons.

## 13. Workspaces, snapshots, layouts, undo and redo

TotalControl keeps your work in four layers.  “Scene” is an internal code
term for a mixer state; the interface uses the terms below.

| Term | Meaning |
|---|---|
| **Current workspace** | The live workspace for the connected unit: mixer, snapshots, layouts, groups, masters and the complete GUI state. It is saved automatically. |
| **Snapshot** | One named, saved current mix inside a workspace. |
| **Workspace** | A portable host-side configuration file: the current mix, snapshots, labels, groups — including the global fader group — layouts, view state and Control Room state. |
| **Device setup** | One of the unit's own six states, separate from host files. |

| Layer | Contains | Stored |
|---|---|---|
| **Workspace** | The portable host-side configuration: current mix, sample rate, pitch, Dim/Speaker B state, the eight named snapshots, labels, groups (including the global fader group), layouts, view state, Control Room assignments and complete GUI state | A `.tcws` file chosen by you, or the current workspace file |
| **Snapshot** | One mixer state: routing, faders, names, trims and the device settings TotalControl manages | Inside the current workspace and explicit workspaces |
| **Current workspace** | The live workspace for the connected unit, saved every minute while it changes and on close, hide, disconnect and unit switch | The current workspace file |
| **Device setup** | The unit's own six-slot state | The interface, not TotalControl |

### Workspaces

`WORKSPACE` opens a menu:

- `Open Workspace…`
- `Save Workspace`
- `Factory Workspaces  ▸`
- `Save Workspace As…`

The Factory Workspaces child menu contains `5.1 Direct`, `7.1 Direct`,
`5.1 → 4.0` and `7.1 → 4.0`.

The file dialogs start in the last folder you used, or in your `Documents`
folder when there is no remembered folder. A workspace is one JSON file with
the `.tcws` extension. Save writes atomically; a failed save cannot destroy the
previous file.

Factory workspaces start from a clean mixer, not from the current one.
Routing, playback and input sends, FX sends and returns, Reverb/Echo, EQ,
dynamics, mutes, solos, labels, groups, layouts and snapshots are reset.
Device-owned clock, sample rate, reference levels and preamps are kept. No
return FX or other processing is switched on, and the FX column starts
collapsed.

Direct layouts route the virtual surround sink channels one-to-one to the
first available hardware-output pairs. On the Fireface UC/UCX that is:

| Workspace | Source | Hardware outputs |
|---|---|---|
| `5.1 Direct` | Front L/R | AN1/2 |
| | Center/LFE, split mono | AN3/4 |
| | Side L/R | AN5/6 |
| `7.1 Direct` | Front L/R | AN1/2 |
| | Center/LFE, split mono | AN3/4 |
| | Rear L/R | AN5/6 |
| | Side L/R | Phones 7/8 |

The 4.0 layouts do not create a new sink. They downmix the 5.1 or 7.1 sink
into two full-range pairs, named Front and Rear on AN1/2 and AN3/4:

| Workspace | Downmix |
|---|---|
| `5.1 → 4.0` | Front L/R direct; Center to front L+R at 0.707; LFE to front L+R at 0.707; Side L/R to rear L/R at unity |
| `7.1 → 4.0` | Front L/R direct; Center to front L+R at 0.707; LFE to front L+R at 0.707; Rear L/R and Side L/R both to rear L/R at unity |

Only the front pair is treated as full range, so LFE goes to the front pair
only. The 7.1 rear sum keeps both physical surround feeds at unity, matching
the loudness of having both rear and side speakers. The downmix layouts do
not add bass management or a sub output.

Front L/Front R, Rear L/R and Side L/R remain linked stereo pairs; Center,
LFE and other mono channels are split mono. Linked user labels drop the L/R
suffix (`Front L` + `Front R` displays `Front`); a split pair shows the two
individual names. The used outputs form group 1. Loading a factory workspace
also clears Control Room assignments that use its output pairs, so those
pairs stay available to the surround layout. A factory workspace has no file
yet: `Save` opens Save As and writes a normal `.tcws` file. You can remap the
output routing afterwards and save that result.

When an output pair is split, selecting one output channel selects only that
mono bus. Its strip is the only active output surface, and the source send
faders edit that mono matrix column. A linked pair keeps the pair-level
submix.

A workspace contains:

- the current mix, including sample rate, pitch/varispeed and Dim/Speaker B state;
- all eight snapshots and their names;
- user channel labels;
- the four mute/solo/fader groups, the global fader group and the mute/solo masters with their held state;
- the six layout slots and the current collapsed/expanded strips;
- view, row mode, sidebar, zoom and selected-output state;
- open channel panels and flyouts;
- Control Room assignments and monitoring settings;
- active Cue, External Input and PFL states;
- each output's Talkback receive flag, as part of the current mix;
- the virtual surround-output settings;
- the window size, position, fullscreen state and scroll positions.

Undo/redo history, open dialogs, meters, animations, pointer state, active
Talkback and Listenback, and transient selections are not saved. The unit's
six device setups and the device serial number are not part of a workspace.
Sample rate, pitch/varispeed and the active Dim and Speaker B states are
stored in the current mix.

Opening a workspace replaces all of the above and applies its mixer to the
device. TotalControl first warns that loading a workspace will overwrite the
current workspace; the buttons are `Cancel` and `Open` (or `Load` for a
factory workspace). `Cancel` leaves the current workspace unchanged. A
workspace written for a different device model is refused.

The `WORKSPACE` label shows `*` while the workspace has changes since the last
explicit Save. Saving clears the marker. The current workspace is saved
automatically; an explicit workspace file is a saved copy. Both are separate
from the unit's device setups.

Clicking `OPTIONS` while the Workspace menu is open switches to the Options
menu immediately, and the reverse. Submenus open on the first click.

### Snapshots

**Snapshots** are eight numbered slots, each with a name. Select a slot and
edit its name in the field below the Store action. They store a complete mixer
state but not the GUI state, groups or layout. Recall applies the stored mix to
the device at once. The current workspace is saved every minute while it
changes and again when TotalControl closes, hides to the tray, loses the unit
or switches units.

### Layouts

**Layouts** are six slots for the collapsed/expanded strip set. They are part
of the current workspace and of a saved workspace, not separate mixer states.
The strip layout itself — sizes, colours and which controls a channel type
shows — is the optional `gui-layout.json` presentation file. It applies to the
whole installation and can carry a section for an individual interface model.

Undo restores the mix as it was before your last mix-changing step; every
continuous drag of one control counts as a single step. Redo reapplies what
you undid. Device settings and window layout are not part of undo.

### Groups

Four groups, each with its own membership and three independent links: mute,
solo and fader. They are part of the workspace.

To build one:

1. Press `edit` in the `groups` card. All twelve storage places — `M1`–`M4`,
   `S1`–`S4` and `1`–`4` — flash.
2. Click the place to fill, for example `1` for fader group 1. Only that place
   keeps flashing, its current members are pre-selected, and the button reads
   `end edit`.
3. Click strips to add or remove them. The session is modal, so a plain click
   is enough; Shift+click also works anywhere on a strip, including its fader.
4. Press `end edit` to store. The place stops flashing and keeps its colour,
   and the fader caps of the members take the same colour.

Clicking another storage place stores the current one and starts the next, so
`1`, `M1` and `S1` can be filled in one session. Esc leaves the session
without storing what was just built.

With the fader link engaged, moving one member applies the same dB change to
the others, so their relative balance is preserved. Do not add faders parked
at the very top or bottom unless the whole group is there: a relative move
cannot go further in that direction. Mute and solo propagate the same way.
`clear` empties all four groups.

The fader groups carry red, blue, green and violet, on their cells and on the
caps of their members; mute cells are steel blue and solo cells amber.
Shift+click on a strip or its fader builds the **global fader group** (the
`F` master above the faders). While the group is ganged its members' caps take
the group colour and their levels move together; pressing `F` off ungangs them
and hides the colour without forgetting them. The global group outranks a
stored colour while it is ganged.

Groups are saved in a workspace. Loading a workspace replaces the current
groups with the stored ones.

## 14. Latency

The Audio Configuration panel shows the round-trip latency in the `RTL
latency` row (hover for "Round Trip Latency"): the value as `x.x ms`, and on
the next line its contributions in milliseconds — USB queue, device lead,
converter and graph. The **Quantum** picker sets the PipeWire graph period in
samples. **Auto** lets PipeWire choose the quantum itself — a safe default for
the device and rate. Choose a specific value from the list to set it yourself;
the list offers only quanta at or above the current rate's period floor, plus
the exact floor for the lowest achievable latency.

The value is the host round trip:

```text
RTL = USB queue + device lead + converter delay + one graph period
```

The USB queue is the driver's in-flight URBs (three URBs at the standard
setting), the device lead is the margin the unit holds ahead of its
converters, the converter delay is the interface's conversion time, and the
graph period is one PipeWire cycle (two ALSA periods under timer scheduling).

The UC device side is measured by loopback (output 5 to input 5) for both
personalities at 32, 44.1, 48, 96 and 192 kHz; the display applies the live
queue and adds the current graph period. Other rates, and the UCX, use the
model with the same terms. Treat the number as the host-side budget for
round trips; an acoustic measurement adds the room and monitoring path.

On the UC the device part is a loopback calibration taken at 32, 44.1, 48,
96 and 192 kHz in both PC and AP modes. The display says **measured PC** or
**measured AP** and applies the live USB queue and the current graph
period. If a driver change alters the USB queue, the calibration is
adjusted by the queue difference without another cable measurement. Rates
outside the calibrated set use the model estimate. A different firmware or
device path needs a new calibration.

### Scheduling mode

PipeWire can drive the audio graph from a host timer (**timer mode**, the
desktop default) or from the device's period interrupts (**IRQ mode**, PipeWire's
Pro Audio scheduling, `api.alsa.disable-tsched`). For low-latency work on a
Fireface, choose IRQ mode. In IRQ mode the driver's fill lock holds the playback
ring at a small, deterministic level (about 1.7 URBs, ≈1.7 ms) with no server
delay-locked loop on the device stream, so reliability does not cost extra
latency margin. Timer mode needs a larger `start-delay` (about three URBs,
scaled with rate) to avoid late dropouts at 96/192 kHz, which adds latency.

Timer mode remains the convenient default for mixed-rate desktop graphs and
heavy processing at large quanta, where its node resampler and independent
graph quantum help. The startup mute window below applies in both modes.

### The startup mute window

When playback starts or the sample rate changes, the driver keeps the device
outputs muted for a short window (about 50–100 ms) and unmutes once the ring
delivery is stable. This replaces the scattered crackle that a low-latency
configuration would otherwise produce at every rate switch with one clean,
brief silence. It is normal and intended: you hear one short gap at the switch,
then continuous audio. The window is bounded (256 ms) so a stalled application
cannot leave the output muted. Steady-state latency is unaffected — the window
occurs only at startup.

The driver counts these gated startups (`out_prefills`) and any that hit the
bound (`out_prefill_timeouts`) in the debugfs statistics; a clean session shows
clears with no timeouts. The `prefill` module parameter (default on) disables
the gate for comparison.

Below the latency line, **Diagnostics** lists the kernel transport error
counters. They are counted continuously whether or not this panel is open.
`in` is device-to-host (capture) and `out` is host-to-device (playback);
the host is the computer running TotalControl. The tooltips are deliberately
short; this section is the full reference. **reset** (tooltip "Clear all
counters") clears every row without changing the device or the audio graph.

The counters are grouped into three sections by **when they clear**:

- **Scheduling** — `underrun / overrun`. Cumulative: counts since the driver
  attached or the last reset. A stream restart never clears it.
- **This stream** — `usb`, `bad packets`, `feedback`. Per-run: clears when
  the audio stream restarts (see below).
- **Session** — `recover`, `gaps`, `rate mismatch`, `pointer`,
  `events dropped`. Cumulative: counts since the driver attached or the last
  reset.

**reset** clears all three sections, so every row reads zero after a reset.

### What "this stream" means — and what it does not

The per-run counters clear when the **device audio stream** restarts, not
when you restart an application. The stream restarts on a **sample-rate
change**, a **quantum (buffer) change**, or a **recovery** after a fault.

It does **not** restart when you start or stop a player. TotalControl keeps
the device stream running between player sessions (the clock stays locked),
so restarting DeaDBeeF or your DAW transport does not clear these counters.
"Per stream" means per device-stream lifecycle, not per player.

This is deliberate. The per-run counters answer "is the *current
configuration* running clean?" When you change the rate or buffer size to
tune latency, the count starts fresh so you can judge the new setting
without stale errors from the old one. The cumulative counters (Scheduling
and Session) keep the history, so nothing is hidden: a fault that happened
earlier still shows there even after a stream restart clears the per-run
detail.

### Which counter to watch

- **Tuning quantum and host load:** watch `underrun / overrun` (Scheduling).
  It persists while you experiment and clears only when you press reset, so
  you can try a setting, see whether it underruns, and reset before the next
  attempt.
- **Judging a configuration change:** watch the This stream rows right after
  changing rate or buffer. A clean new setting stays at zero.
- **Diagnosing a fault:** watch the Session rows for the accumulated history
  (recoveries, gaps, rate mismatches), and the This stream rows for what the
  current stream is doing.

### Counter reference

| Section | Row | Meaning | What to do |
|---|---|---|---|
| Scheduling | underrun / overrun | The host playback ring ran dry (underrun) or the capture ring overflowed (overrun). On an underrun the driver sends silence for the dry chunk; a single late write is heard as a brief gap. At stream start and after a rate change the startup mute window (see above) holds the output muted while the host fills the ring; starvation inside that window is intended and is not counted here. If the ring stays dry for two consecutive chunks outside that window the driver stops the playback stream and the audio graph restarts it. A dry ring is never heard as stale audio. An underrun is the usual cause of a periodic tick or dropout at a small quantum. | Raise the quantum or buffer size, or reduce system load (CPU spikes, other USB traffic, interrupt storms). |
| This stream | usb | USB packet status errors in the current stream. | Try another USB port or cable; avoid hubs; check for bandwidth contention. |
| This stream | bad packets | Short, malformed, misaligned or dropped packets in the current stream. The startup and rate-switch burst is excluded by the per-run reset. | Usually accompanies a USB link problem; see `usb` and `gaps`. |
| This stream | feedback | Clock-tracking packet detail in the current stream: a position discontinuity, a header frame-count mismatch, or feedback unavailable. The device's one-time start resync at a stream start is expected and is not counted. | Check the clock source and sample rate; a growing value indicates a device clock or USB timing fault. |
| Session | recover | The driver restarted the streaming engine after a serious fault. | Check the other rows for the cause; if it repeats, reseat the USB cable or try another port. |
| Session | gaps | Missing packets: a completion arrived late or a packet was absent. | Reduce USB bus load; try a dedicated controller or port. |
| Session | rate mismatch | The capture cadence sustainedly disagrees with the set rate (the rate write did not take effect, or the device was reprogrammed). | Check the clock source and sample rate; if it persists, reload the driver. |
| Session | pointer | The driver's buffer pointer diverged from the ALSA core pointer. | Rare; if it repeats, restart the stream or reload the driver. |
| Session | events dropped | The kernel hwdep event queue was full and the application reader fell behind, so an event was discarded. Not a USB fault. | Reduce UI or system load; harmless if occasional. |

A non-zero error row that keeps growing while audio plays means the
transport is dropping or mis-timing packets; a row that stays at a fixed
value records a past event. A few short packets at a rate change are
expected and are not counted against the new stream.


## 15. Where TotalControl keeps its data

- Current workspace per unit: `current-<unit>.tcws` in the data directory;
  saved automatically and restored on the next start.
- Explicit workspaces: `.tcws` files in the folder you choose when saving.
- Host settings: `host-settings.json` in the `~/.config/totalcontrol/`
  directory. It holds the start/tray choice, the MIDI remote, OSC, engine,
  startup sync and the last workspace folder, globally and per unit.
- Channel strip layout: `gui-layout.json` in the same directory. It holds the
  global and per-interface presentation and is optional.
- Snapshots and layout slots: inside the current workspace and explicit
  workspaces, not in separate files.

The current workspace is the live state of the session. A saved workspace is
the portable copy and contains the same mixer, snapshots, labels, groups,
layouts and GUI state.

`gui-layout.json` is optional and applies to every view and workspace. It
changes the appearance of the strips — their dimensions, text and control
sizes, colours, and which supported controls each channel type shows. The
file has a global default and may have a section for an individual interface
model. The interface capabilities always decide which controls exist: a UC
never shows EQ, Dynamics or the FX fader even if a layout section mentions
them. Without the file the built-in layout is used. It is read once at
startup, so restart TotalControl after editing it. An invalid file is ignored
and the built-in layout is used; start TotalControl from a terminal to see the
reason. The file never changes mixer state, routing or what a control does.

Create a starting document with the layout preview:

```sh
cd ui/totalcontrol
cargo run -p totalcontrol-layout --bin channel-preview -- \
    --example ~/.config/totalcontrol/gui-layout.json
```

## 16. Running in the background

With a system tray, closing the window keeps TotalControl running: the front-panel DIM button and the restore of your mix after a replug
are TotalControl's job while it runs. The tray icon toggles the window; its menu
has dim, recall mix 1–8, settings and quit. `totalcontrol --minimized` starts
hidden. Settings can start TotalControl with your session, hidden in the tray;
a normal launch opens the window. Without a tray, closing the window quits;
one TotalControl runs per session, and a second launch tops up the first.

See [`ui/totalcontrol/docs/guide.md`](../ui/totalcontrol/docs/guide.md) for the Linux
audio stack, building from source and project status.

## 17. Device settings at startup

At connect TotalControl compares the device state with the saved mix and
lets you choose, with a dialog:

- **Use TotalControl settings** — write the saved mix to the device.
- **Use device settings** — adopt the decoded device values in TotalControl
  and keep them on the unit.
- **Always use TotalControl settings** — as above and stop asking.

On the UCX the window appears first. TotalControl then requests the pre-connect
state dump and compares input and output levels, mute, EQ, dynamics, FX
returns and matrix sends. The dialog appears when the comparison is ready, so
the startup is not held back by the dump.

The dialog lists every decoded difference. When more than six are shown, the
list gets its own scrollbar and a fixed height so the dialog does not grow
beyond the window.

The dump does not include the options word (clock source, optical format,
SPDIF professional, word-clock rate and termination, EQ+D for Record), channel
names, trims or loopback. Those stay with the saved mix. On the UCX the
`48 V`, Inst, Level and reference settings are in the dump and are compared.

## 18. The UCX front knob

The UCX has one rotary encoder with push and a two-digit LED display. A green
LED shows CHANNEL or LEVEL mode. Pushing toggles between the modes; pushing
repeatedly jumps to `PH` for the phones volume.

### Channel selection

In CHANNEL mode, turning the encoder cycles through:

| Display | Channel |
|---|---|
| `i1`–`i4` | inputs 1–4 |
| `L1`, `L3`, `L5` | line output pairs 1/2, 3/4 and 5/6 |
| `PH` | phones / line output 7/8 |
| `SP` | SPDIF output |
| `A1`, `A3`, `A5`, `A7` | ADAT output pairs 1/2 … 7/8 |
| `SU` | setup |

Press the encoder to show the selected channel's value. Turning then changes
that value.

### Input gain and reference

Inputs 1/2 use the microphone range: 0 dB, then +10 to +65 dB in 1 dB
steps, with 48 V. Inputs 3/4 use the instrument/line range, 0 to +18 dB
in 0.5 dB steps, with PAD and INST. Inputs 5–8 have no variable gain;
their reference is −10 dBV, +4 dBu or Lo Gain, which places 0 dBFS at
+2 dBV, +13 dBu or +19 dBu. The line and phones outputs use −10 dBV,
+4 dBu or Hi Gain.

The input meter is post-gain. The device's SIG LED is −65 dBFS and the
CLIP LED is −2 dBFS; both follow the same post-gain signal as the meter.

A long press on an input enters pair-adjust mode. The left digit gets a dot
and the encoder moves **both** channels of the pair, whether or not the pair
is linked. For example, a gain of 26 dB shows `2.6` while both channels move,
then returns to `26`.

The device has a fixed link view: inputs `i1`–`i4` are individual, and the
outputs appear only as pairs `L1`/`L3`/`L5` and `A1`/`A3`/`A5`/`A7`. The front
knob cannot link or unlink a pair; the link state is a TotalControl setting.

A linked input pair always shows both gains in the TotalControl Gain knob:
the left gain above, the right gain below, even when they are equal. The knob
position, tick and orange arc follow the higher gain. Turning the knob moves
both preamps by the same step while the higher gain has range left. The whole
knob stops when the higher gain reaches a range limit. The wheel over the top
number moves only the left preamp; over the bottom number it moves only the
right one. The lower preamp can therefore still be moved when the higher one
is at a limit. Double-click resets both gains to the minimum. This mirrors the
device's pair-adjust mode, which moves both channels whether or not they are
linked. An unlinked input shows one value and moves only that channel.

### Output level

Output levels use 1 dB steps. The range is +6 dB down to −64 dB, then mute.
A dot after the second digit marks a negative value; `0` to `+6 dB` has no
dot.

A long press on an output enters panorama mode:

| Display | Meaning |
|---|---|
| `CE` | centre |
| `L.1` / `r.1` | full left / full right |
| intermediate | balance position |

The balance range is −100 to +100 in steps of 5. The display returns to the
output fader after about one second without a change. TotalControl follows the
balance change and shows the same position on the output's balance knob.

### Dots

| Dot | Meaning |
|---|---|
| left digit | the encoder is adjusting both channels of the pair |
| right digit on `i3`/`i4` | half-dB step (`11.` = 11.5 dB); always present for a half-dB value, absent for a whole-dB value |
| right digit on an output level | negative value, shown only below 0 dB |

The left dot is not a decimal point. `2.6` on an input means 26 dB with both
channels moving, not 2.6 dB. The right dot is never shown for `i1`/`i2`
because their step is 1 dB. It is not a link marker.

### Setups

`SU` selects a stored setup:

| Display | Meaning |
|---|---|
| `1`–`6` | stored mixer setups |
| `FA` | factory preset / Total Reset |
| `Ad` | 8-channel AD/DA converter mode, only when disconnected from USB |
| `CC` | Class Compliant mode, only when disconnected from USB |

The press that enters the `SU` menu is short. The final press that loads the
selected entry must be held for about one second; a short press does not load
it. `i.o.` means loaded and `n.o.` means an empty slot. Loading a setup while
connected changes only the front-panel parameters — input gains and output
volumes — not the routing. `CC` activates after a power cycle.

The six slots are written from the host application, **Options → Store current State
into Device → Setup 1–6**. The host does not upload a mix; the unit snapshots
its current internal state, including routing, EQ, FX, references and options.
TotalControl leaves the six device slots untouched. Its own **Mix 1–8**
snapshots are host files, not the unit's setups.

If the unit reports an error at power-up and every user slot still shows
`n.o.` after the host application has stored it, the unit's setup memory has not passed
its self-test. `FA` may still load because it comes from the firmware. In that
case the six device slots are unavailable; keep using host mixes and Mix 1–8
and have the unit serviced.

## Appendix A — Device modes

TotalControl needs the normal vendor mode of the interface. In that mode the
kernel driver exposes the PCM device and the hwdep mixer node.

In class-compliant mode the stock `snd-usb-audio` driver carries audio, but the
vendor mixer is not available. TotalControl cannot control the mixer in that
mode. Select the normal mode on the unit before starting TotalControl. On the
UCX, switch to class-compliant mode only while the unit is disconnected from
USB; power-cycle it after the change before reconnecting.

Only one application may own the mixer at a time. Close other mixer
applications before starting TotalControl.

## Appendix B — PipeWire

TotalControl uses the ALSA PCM exposed by the kernel driver. PipeWire sees it
as a normal card. Use the Pro Audio profile when the device should expose all
channels to a DAW.

The buffer-size picker in TotalControl sets the graph quantum. The driver adds
its own USB queue and device lead; the latency display explains the result.

The installer's WirePlumber rule runs the device in timer-based scheduling
with the 18-channel Pro Audio profile. PipeWire wakes the audio thread on its
own timer and holds the queued playback delay at a target: the graph quantum
plus a scheduling cushion (`api.alsa.headroom = 336` frames), plus one period
that PipeWire adds for a batch device. The target is clamped at half the
buffer. The card reports its real queue, so the level stays constant against
the device clock: the device conducts and the host follows. This is what
keeps the smallest quanta stable.

Polled scheduling (`api.alsa.disable-tsched = true`) is the alternative for
specialised low-latency work. PipeWire then wakes on the device's period
interrupts and has no level loop, so the queued level depends on the wake
timing and can drift. At a quantum equal to one USB period (48 frames at
48 kHz) one late write starves the device. Timer mode is the default for
that reason; use the cushion and a larger quantum with polled scheduling.

At 96 kHz and 192 kHz use at least two milliseconds of quantum for
low-latency recording: 192 frames at 96 kHz, 384 at 192 kHz.

The unit is the clock master: its converters run on the device's own clock
(SteadyClock) and the host mirrors the stream rate to it. The computer's
clock only paces delivery, and the buffer absorbs the small difference.

For the smallest quanta, keep the host quiet. The audio thread runs at
real-time priority and is not displaced by ordinary CPU load. Deep processor
idle states, interrupts, virtual machines and file indexers (for example KDE
Baloo) scanning the disk can still delay its wake-ups; the setup section's
"Low-latency host tuning" lists the measured measures. Keep the device on its
own USB controller where possible, and avoid heavy disk IO during a session.

The WirePlumber rule in
[`ui/totalcontrol/tools/alsa/wireplumber-uc.conf`](../ui/totalcontrol/tools/alsa/wireplumber-uc.conf)
selects polled scheduling and the 18-channel Pro Audio profile.

### Surround playback

The Pro Audio profile is the right choice for a DAW, but its 18 playback
channels are presented as `AUX0…AUX17`.  Desktop players that send a surround
stream cannot map those speaker positions onto AUX channels, so PipeWire
keeps only the front pair.

For surround media, open `Options → TotalControl` and enable `Virtual 5.1 output`,
`Virtual 7.1 output`, or both. The sinks exist while TotalControl runs and
follow the selected Fireface/MR18 device:

- `TotalControl Surround 5.1` → SW playback 1/2 FL/FR, 3/4 C/LFE, 5/6 SL/SR
- `TotalControl Surround 7.1` → SW playback 1/2 FL/FR, 3/4 C/LFE, 5/6 RL/RR,
  7/8 SL/SR

The factory workspaces in section 13 use these same orders. The 4.0 factory
layouts are downmixes of these sinks; they do not create another sink.

Select the matching sink in KDE or in the player.  An already-playing stream
keeps its old device; stop and restart it, or move it to the new sink.  The
bridge is host-side: TotalControl still sees the audio on the SW playback
channels and can route it like any other playback source.

## Appendix C — MIDI and OSC setup

Remote control has its own dialog: `Options > MIDI and OSC`, with a MIDI
part and an OSC part. Both surfaces work on every supported device and
personality — they drive the same mixer model as the GUI; the device only
supplies the writes.

**What to expect from the settings dialogs.** Changes apply as you make
them, and the dialog stays open so you can see or hear the effect while you
adjust — handy for trying a controller port or a monitoring level live.
There is no `OK` or `Apply` to remember and no `Cancel`: a single `Close`
(or `Escape`) dismisses the dialog, and everything you changed is already
saved. A section that is switched off is grayed out; only its `Enable`
switch stays active until you turn it on.

### MIDI — control surfaces and remotes

TotalControl runs MIDI controllers through one generic engine. A controller
is described by a definition file; Mackie Control is the first shipped
definition, so any Mackie Control compatible surface works (Mackie Control,
Behringer BCF2000 in "MC C" mode, X-Touch, and many others).

1. Connect the controller. A DIN surface goes into the unit's MIDI breakout
   ports; a USB surface connects to the computer directly.
2. In the MIDI part, turn on `Enable`. Pick the controller in the
   `Controller` list — every USB MIDI port and the unit's own ports are
   listed — then check `MIDI In` (and `MIDI Out` for feedback). Each checked
   input runs one controller, and changes take effect immediately. Several
   controllers can run at once.
3. The `Protocol` list preselects the matching definition; the line below
   shows which file is in use. The options below belong to that definition.
4. Move a fader: the matching channel strip moves, and its name turns
   brown while it is in the surface's 8-channel window.

With the Mackie definition, the surface controls the mix of the selected
submix. Bank and channel buttons move the window, the arrow buttons change
the row, REC selects the submix, and the control-room buttons map to Dim
(STOP), Talkback (PLAY), Mono (PAN), Speaker B (FLIP), Master Mute (EQ),
Master Solo (PLUGINS) and Recall (RECORD). F1–F8 load snapshots. The VPOTs
set pan; pressing one centers it. The full mapping is in
[`spec/midi-controllers/MACKIE_MIDI.md`](spec/midi-controllers/MACKIE_MIDI.md).

Mackie options: `Enable Mackie protocol` (off leaves only the simple CC
commands below), `Submix linked`, `Full LCD support`, `Send user defined
names`, `Map Stereo to 2 Mono Channels`, `Submix Selection in
Input/Playback Row`, `Disable MIDI in background`, `Send level messages`
and `Peak hold`. Leave `MIDI Out` unchecked when no feedback is needed.
`Show MIDI debug log` opens the traffic panel.

The RME ARC USB is supported as a remote. Plug it in; it appears in the
`Controller` list as its own card and the protocol preselects. Its 15 keys
and the footswitch trigger Control Room actions, the encoder adjusts the
volume of its target (Main Out or a phones mix), and the key LEDs follow
the mixer state. The `Key assignment` table reassigns every key; the
encoder target, step and talkback mode are options of the ARC definition.

Without a Mackie surface, any MIDI controller can use the simple commands
on the unit's ports: CC 7 on channel 1 sets the Main Out volume; CC 9 sets
input gain; CC 102–117 on channels 1–12 set the faders of the three rows;
CC 102/103 on channel 13 switch Trim Gains; CC 104+2n on channel 13 selects
a submix. The complete table is in the Mackie controller spec §9.

**Adding your own controller.** A definition file is JSON. Copy a shipped
file from `/usr/share/totalcontrol/midi-controllers/` (or
`/usr/local/share/totalcontrol/midi-controllers/`) into
`~/.local/share/totalcontrol/midi-controllers/`, edit the controls, feedback
and options, and restart TotalControl; a file of the same name overrides the
shipped one, a new name adds a protocol. The format is specified in
[`spec/TOTALCONTROL-CONTROLLER-SPEC.md`](spec/TOTALCONTROL-CONTROLLER-SPEC.md).
A tested definition can be contributed back as a pull request.

**Loopback warning.** Do not wire the controller's MIDI output back into its
input. TotalControl sends a detection note every 500 ms; if it hears it
back, it disables that controller until you re-enable it. A surface whose
faders snap back by themselves is usually in such a loop.

The unit's own stand-alone MIDI control (the unit acts as the host when
no computer is connected) is a device mode, not a TotalControl feature;
see the device protocol specifications.

### OSC

OSC is opt-in. In the OSC part, turn on `Enable` and choose `OSC 1.96` or
`Global`. The input port (default 7001), output port (default 9001) and the
`Remote controller IP` apply as soon as their content is valid and are saved
with the other settings. The line above the ports shows this host's IP
address and host name — the address to point the controller at. Disabling
the switch stops the surface immediately.

While the remote controller IP is 127.0.0.1 the socket stays on loopback.
Set it to the controller's address (for example an iPad on the LAN) and
the socket binds on all interfaces so the controller can reach us; allow
the ports through the firewall. Do not run two controllers on the same
feedback port.

`Show OSC debug log` below the enable switch opens the traffic panel; it
is greyed out until OSC is enabled and closes when OSC is switched off.

`RME 1.96` is the banked RME 1.96 address space. The older 1.90
classic surface is not supported. `RME Global` is the absolute-channel RME
2.1+ surface: channels are addressed by their 0-based position and continuous
values use native units instead of the RME 1.96 normalized scale. The selected
mode is saved with the other settings.

The command line remains a development override:

```sh
totalcontrol --osc
```

| Option | Default | Meaning |
|---|---:|---|
| `--osc-mode` | classic | `classic` (OSC 1.96) or `global` protocol |
| `--osc-port` | 7001 | UDP port TotalControl listens on |
| `--osc-send-port` | 9001 | UDP port for feedback |
| `--osc-host` | 127.0.0.1 | Feedback destination |

Configure the controller to send to the receive port and receive feedback on
the send port. Both address spaces are defined in
[`spec/TOTALCONTROL-OSC-SPEC.md`](spec/TOTALCONTROL-OSC-SPEC.md); Global OSC is
§13 of that document.

## Appendix D — Command-line options

| Option | Effect |
|---|---|
| `--osc` | Enable the OSC control surface |
| `--osc-mode classic|global` | Select OSC 1.96 or Global |
| `--view quick|mixer|matrix` | Start on a view |
| `--fullscreen` | Start fullscreen |
| `--minimized` | Start hidden in the tray |
| `--scale <n>` | Initial UI scale |
| `--select-out <n>` | Initial submix |
| `--mock` | Simulate a Fireface UCX |
| `--mock-ucx` | Simulate a Fireface UCX |
| `--mock-uc` | Simulate a Fireface UC |
| `--mock-classic` | Simulate a Babyface-classic |
| `--control-only` | Do not open the capture PCM |

The mock modes are for demonstration and development. Capture tools are
documented in `DEVELOPMENT-TOOLS.md` and are not supported installation
options.
