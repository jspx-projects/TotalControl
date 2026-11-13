# Contributing to TotalControl

TotalControl supports the Fireface UC, Fireface UCX and Babyface-classic
through the kernel hwdep interface. Contributions that keep the device model,
the protocol specifications and the UI consistent are welcome.

## Development setup

```bash
git clone https://github.com/jspx-projects/TotalControl
cd TotalControl
cargo build --release --workspace
cargo test --workspace

# Run the GUI/TUI against a simulated device — no hardware required:
cargo run -p totalcontrol-gui -- --mock
cargo run -p totalcontrol-tui -- --mock
```

Dependencies: `rustc`, `cargo`, `alsa-lib` (dev headers) and a kernel with the
matching headers for the driver.

## Architecture

The kernel module owns USB transport, the ALSA PCM device and one hwdep control
node per card. `totalcontrol-core` owns the device model and writes through
hwdep. The GUI and TUI are two front ends over the same `RmeDevice` trait.

- Wire encoding lives in `totalcontrol-core/src/fireface/`.
- Channel topology and per-model data live in `totalcontrol-core/src/profiles/`.
- Visible behaviour is specified in `docs/spec/`.

## Adding a device

A new device needs a descriptor and hwdep node in this repository's kernel
module. Backends that depend on a driver outside this repository, or that speak
USB directly, are not part of the supported set.

1. Add the kernel descriptor and its hwdep transport in `kernel/`.
2. Add a `DeviceProfile` under `totalcontrol-core/src/profiles/` with the
   channel names, counts and output display order.
3. Register the model and transport in the profile selection.
4. Add the device protocol specification under `docs/spec/`.
5. Add profile and backend tests. Use the existing mock for the model where it
   applies.

## Tests

Run both required suites before opening a pull request:

```bash
cd ui/totalcontrol
cargo test -p totalcontrol-core --lib --features alsa
cargo test -p totalcontrol-gui
cargo test -p totalcontrol-tui -p totalcontrol-layout
cd ../..
python3 scripts/check-docs.py
```

## Hardware tests

Record the device model, transport, firmware, quantum and the test steps.
Preserve the user scene and the current workspace, and leave the unit on the
user's scene when the test ends. Use one Fireface at a time.

## Pull requests

Open one pull request per logical change. Keep `main` linear; do not merge
without the required checks. Explain why the change is needed and which
behaviour or specification it updates.
