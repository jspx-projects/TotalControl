# Contributing a new device

This guide is for a contributor adding support for another RME interface. It
describes the smallest implementation we can integrate, the full-support
expectations, the evidence we need, and the tests that protect later bug fixes.

It complements the authoritative documents:

- [`TOTALCONTROL-ARCHITECTURE.md`](TOTALCONTROL-ARCHITECTURE.md) — transport,
  device service and data flow;
- [`spec/TOTALCONTROL-DEVICE-PROFILE-SPEC.md`](spec/TOTALCONTROL-DEVICE-PROFILE-SPEC.md)
  — profile selection, capabilities, host operations and scene compatibility;
- [`spec/TOTALCONTROL-STATE-SPEC.md`](spec/TOTALCONTROL-STATE-SPEC.md) —
  workspace and persistence;
- [`spec/TOTALCONTROL-GUI-SPEC.md`](spec/TOTALCONTROL-GUI-SPEC.md) — visible
  behaviour;
- the device protocol specification under `docs/spec/`;
- [`DEVELOPMENT-TOOLS.md`](DEVELOPMENT-TOOLS.md) — capture and probe tools.

## 1. Support policy

The supported transport is the **in-tree kernel module plus its hwdep node**.
A new device must share that path:

- the device is enumerated by `kernel/`;
- the kernel exposes its ALSA PCM and one hwdep control node;
- TotalControl controls the mixer through hwdep vendor requests;
- the device model, scene and workspace live in userspace.

Contributions that depend on a driver outside this repository, a direct-USB
crate, or a vendor userspace daemon are not part of the supported set. A
class-compliant path can be described as a development or fallback path, but it
does not replace the vendor hwdep path.

We may integrate a **minimal, device-specific implementation** before every
control is mapped. We do not expose guessed writes or host-only placeholders
for controls the unit actually provides.

## 2. What we expect from a contributor

A device contribution needs a person with access to the hardware, not only a
protocol dump. The contributor is expected to:

- provide captures and a written protocol specification;
- implement and test the kernel transport against the unit;
- test the integrated tree, not only the initial branch;
- report firmware, transport and test conditions with each hardware result;
- preserve the user scene and workspace, and leave the unit on the user's
  scene;
- avoid personal data, serial numbers, USB IDs, hostnames and captured device
  state in commits, pull requests and issues.

Keep raw captures and measurements outside the repository. Use synthetic
fixtures for tests. Working notes, open questions and capture requests belong
in `docs/work/`.

## 3. Minimum first integration

The first integration may be marked **experimental**. It must be honest about
what is implemented.

| Area | Required for a first integration |
|---|---|
| Kernel | Descriptor, transport, ALSA PCM, hwdep info/counters/events as applicable |
| Identity | Model and transport are reported and select exactly one profile |
| Profile | One profile entry per `(model, transport)` with channel table, requirements and capabilities |
| Basic controls | Real device-backed volume/mute and at least one documented routing path for the channel classes the profile exposes |
| Read policy | Every exposed operation is classified as device-read, host-held or unavailable |
| Scene | Supported controls round-trip through the scene; unsupported fields are skipped and reported |
| Meters | A documented device meter layout, or `None` with an explicit host fallback |
| Tests | Profile/contract tests, parser tests and the repository test suites |
| Documentation | Device protocol specification and user-facing notes |

The application must omit or disable controls the unit cannot perform. A
control that returns success without changing the device is not acceptable.

The following are valid follow-ups when the first integration is explicitly
experimental:

- full matrix-cell routing;
- all preamps, switches and reference levels;
- EQ, dynamics, FX and other DSP blocks;
- complete device meter decoding;
- every supported rate and both transports;
- hotplug, suspend/resume and recovery edge cases;
- latency calibration.

## 4. Full support

Full support means the profile describes the device's implemented control
surface and the contributor has tested the integrated tree. It includes, where
the hardware has them:

- every supported sample rate and transport personality;
- clock-source and digital-format controls;
- input preamps, pad, instrument, phantom and reference levels;
- pair/cell routing, sends, pans, mutes, solos and output volumes;
- link/split, phase, trim and balance behaviour;
- EQ, low cut, dynamics and FX;
- device or documented host meters, over detection and peak hold;
- scene apply, workspace restore and transport switching;
- front-panel readback where the unit exposes it;
- disconnect, reconnect, suspend and resume;
- a measured latency model where the device path needs one.

A new field must be classified as transport-specific before it may differ
between two profiles for the same model. The profile contract tests enforce
this.

## 5. Evidence before coding

Start with a protocol specification, not with UI code. It must define, as
applicable:

| Topic | What to record |
|---|---|
| USB transport | interfaces, endpoints, alternate settings, packet layout |
| Modes and rates | mode table, frame/word counts, rate families |
| Control transport | vendor requests, register banks, read/write widths, latching |
| Mixer model | channel counts, block order, matrix shape, scales, mute and phase semantics |
| Preamps | gain law, steps, switches, references, readback |
| State/events | state words, status packets, meter packets, framing and sequence |
| Quirks | initialisation order, write-only registers, rate-change reloads, recovery |
| Unknowns | open questions and capture requests in `docs/work/` |

The specification is present-tense and describes the shipped mapping. Do not
write a capture diary or a changelog. If evidence is incomplete, state the
implemented mapping positively and keep the open question in `docs/work/`.

Do not include USB vendor/product IDs or device serial numbers in the
specification, test evidence or pull request. The kernel descriptor table
contains the values it needs; the contribution document does not repeat them.

## 6. Repository map

| Concern | Location |
|---|---|
| Kernel descriptors, transports, PCM and hwdep ABI | `kernel/ffuc.c`, `kernel/ffuc.h`, `kernel/ffuc-uapi.h` |
| Kernel transport notes | `kernel/README.md` |
| Device model and hwdep backend | `ui/totalcontrol/totalcontrol-core/src/fireface/` |
| Profile schema and selection | `totalcontrol-core/src/profile.rs`, `models.rs`, `profiles/` |
| Device protocol specifications | `docs/spec/` |
| GUI, TUI and mocks | `totalcontrol-gui/`, `totalcontrol-tui/` |
| Capture and probe tools | `scripts/`, `tools/`, `totalcontrol-core/examples/` |

The existing profiles are the reference:

- `profiles/uc.rs` — two transports and a no-DSP control surface;
- `profiles/ucx.rs` — the DSP/FX control surface;
- `profiles/babyface_classic.rs` — a smaller experimental surface.

## 7. Integration steps

1. **Capture and specify.** Capture the descriptors, transport, state/meter
   packets and the control writes. Write the protocol specification and list
   unknowns in `docs/work/`.
2. **Add the kernel descriptor and transport.** Enumerate the device, select
   modes/rates, expose playback/capture and the hwdep node. Build and load the
   module; confirm the PCM and hwdep paths before touching the GUI.
3. **Add the profile id.** Extend the model/personality and profile id enums,
   register the profile in `profiles::ALL`, and add the match arms in the
   profile lookup. The profile table must remain the only selection source.
4. **State the requirements.** Add minimum input/output/playback counts and
   the meter packet length. A mismatch blocks the connection with a clear
   message.
5. **Bind the backend.** Report transport, personality, capabilities and the
   channel model from the profile. Do not duplicate channel names in the
   backend or GUI.
6. **Implement controls.** Add the real vendor/register writes. Set the read
   policy for every operation. Omit unsupported rows. Add a protocol test for
   each encoding and a scene test for each stored field.
7. **Add meters.** If the unit publishes meters, decode the exact packet and
   add a layout id. If it does not, use the documented host fallback and say so
   in the profile and specification. Never present an estimate as a device
   reading.
8. **Add the mock.** The mock exercises the profile and GUI without hardware.
   It must expose the same capability gating as the real backend.
9. **Add tests and documentation.** Update the device profile spec, GUI spec
   and manual only where the implemented behaviour changes. Add the user-facing
   limitations to the manual.

## 8. Tests

### Required repository tests

From `ui/totalcontrol`:

```sh
cargo test -p totalcontrol-core --lib --features alsa
cargo test -p totalcontrol-gui
cargo test -p totalcontrol-tui -p totalcontrol-layout
```

From the repository root:

```sh
python3 scripts/check-docs.py
```

A kernel change also requires a kernel build with the matching headers. A
transport, rate, channel-count or routing change requires
`scripts/ffuc-48k-gate.sh` on a UC; add an equivalent gate if the new device
needs one.

### Hardware test matrix

Record the exact model, firmware, transport, rate and quantum for every run.
Test at least:

- cold start, disconnect and reconnect;
- each supported transport personality;
- 44.1 and 48 kHz, plus every rate the contribution claims;
- playback and capture;
- one routing change, one fader/mute change and one scene apply;
- preamps and switches if implemented;
- meters and overload indication if implemented;
- scene save, workspace restore and transport switch;
- suspend/resume if the transport claims it.

Use one Fireface at a time. Preserve the user scene and the current workspace,
and leave the unit on the user's scene at the end.

### Bug-fix evidence

A bug fix needs a reproduction that fails before the change and passes after
it. Include:

1. firmware, transport, rate and quantum;
2. the control or signal path;
3. the exact steps and expected/actual result;
4. whether the failure is deterministic or intermittent;
5. the before/after result;
6. a regression test where the behaviour can be tested without hardware.

Do not attach raw device-state dumps, mixer scenes or captured user audio to a
pull request. Keep them locally and describe the synthetic case in the test.

## 9. Pull requests

- One branch and one pull request per logical change.
- Open a draft early; keep the hardware evidence and test results in the
  description.
- Include what is implemented, what is not, and which controls are omitted.
- Do not merge unrelated driver, UI, documentation and tooling work together.
- Do not merge or close a pull request until the maintainer has approved the
  completed change.
- After the work is integrated, sync `main`, build against your kernel, test
  the integrated tree, and close any original tracking pull request once
  verified.

A useful device pull request contains:

```text
Device:
Firmware:
Transport:
Kernel:
Rates/quantum tested:
Implemented:
Not implemented:
Test commands and results:
Scene/workspace preserved: yes/no
```

## 10. Keeping future support possible

The profile table and protocol layer are deliberately declarative. To keep a
new device supportable after the first pull request:

- keep every known register and packet meaning in the protocol spec;
- keep unknown registers in `docs/work/` with a capture request;
- use capability flags and read policies rather than model checks scattered
  through the UI;
- keep transport-specific behaviour in the driver or profile, not in generic
  channel code;
- add a profile contract test whenever a new operation or scene field is
  introduced;
- keep the mock on the same profile path as the real backend;
- do not replace device-backed behaviour with a host-side approximation.

When the device supports something and the register is unknown, add a note to
`docs/work/` and request a capture. That is the path to future support; a
guessed write or an inert control is not.
