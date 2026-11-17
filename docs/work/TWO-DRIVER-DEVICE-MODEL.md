# Two-driver device model — refactor plan

Working note, not part of the specification. The plan for letting TotalControl
drive a device whose audio runs in the mainline kernel driver while a small
second driver of ours carries only control and meters. The Digiface USB is the
first such device; the refactor makes the pattern reusable for later devices.

## 1. Goal and non-goals

Goal: a device can be a **control card** (our hwdep module) paired with a
separate **audio card** (mainline `snd-usb-audio`), shown and handled as one
device. Adding such a device becomes a small, well-defined contribution.

Non-goals:

- The Digiface protocol backend itself. It needs the contributor's verified
  spec and kernel module; it plugs into this framework later.
- A grand new abstraction layer. The change is the minimum generalization of
  the existing pattern, proven by a mock.
- Any change to the Fireface or Babyface-classic path. For those, one card is
  still both audio and control.

## 2. Current architecture (verified from the code)

| Fact | Where |
|---|---|
| A backend implements `RmeDevice`; the app wraps it in `DeviceHandle` | `totalcontrol-core/src/device.rs`, `totalcontrol-gui/src/app.rs` |
| `DeviceHandle` variants: `Fireface`, `BabyfaceClassic`, `Mock`, `Mirror`; methods dispatch via the `delegate!` macro | `app.rs` §DeviceHandle |
| Discovery scans `/proc/asound/cards` for RME/Fireface, opens `/dev/snd/hwC{card}D0`, verifies the ABI INFO, filters by model | `fireface/hwdep.rs::enumerate`, `candidate_cards` |
| `Fireface::open_model` builds a `HwdepTransport` on the found card | `fireface/device.rs::open_model` |
| `card_id()` reads `/sys/class/sound/card{info.card}/id` — the hwdep card's id | `fireface/device.rs::card_id` |
| The app uses `card_id()` for the PipeWire clock stream, controller discovery and meters | `app.rs` (`pw_clock::card_stream`), `controllers/discover.rs` |
| `available_targets()` lists hwdep units plus the simulations; `open_kind`/`open_target` open one | `app.rs` |
| The device service spawns a `DeviceHandle` and copies `card_id()` into the mirror info | `device_service.rs::spawn`, `MirrorInfo` |
| Profiles are selected by `(model, transport)`; `ProfileId` is a closed enum | `totalcontrol-core/src/profile.rs` |

The load-bearing assumption: **the hwdep control card is also the audio card.**
`card_id()` returns the control card's id, and everything downstream (clock,
controllers, meters) treats it as the audio card. A control-only device breaks
that assumption: its hwdep card carries no PCM.

## 3. The conceptual change

Separate two identities that are today conflated:

| Identity | Meaning | Fireface | Control-only device |
|---|---|---|---|
| control card | the hwdep node the backend opens | card N | card N (our module) |
| audio card | the ALSA card whose PCM PipeWire uses, and whose id the clock/controller/meter paths need | card N (same) | card M (mainline, different) |

`card_id()` already semantically means "the audio card". The refactor makes
the distinction explicit and gives a control-only backend a way to report the
paired audio card's id rather than its own hwdep card's id.

## 4. Design

### 4.1 A control-only descriptor table

A small declarative table, one entry per control-only device:

```rust
pub struct ControlOnlyDescriptor {
    pub kind: DeviceKind,            // new variant, e.g. Digiface
    pub profile: ProfileId,          // new profile id
    pub model_name: &'static str,    // "Digiface USB"
    pub control_card_id: &'static str, // "DigifaceControl" (hwdep card id)
    pub audio_card_match: CardMatch, // how to find the mainline audio card
}
```

`CardMatch` matches the mainline audio card by id or by a description
fragment (never by card number). The table is the single registration point;
a new device adds one entry plus its backend and profile.

### 4.2 Pairing discovery

A generic discovery function:

1. Scan the hwdep cards for one whose id equals `control_card_id` and whose
   ABI INFO answers (reuse the `hwdep::enumerate` scan pattern, generalized to
   match by card id rather than by Fireface model).
2. Find the mainline audio card matching `audio_card_match`.
3. Both present → one `DeviceTarget` carrying the control card number, the
   audio card id, and the kind. Control card absent → no entry. Audio card
   absent → a "no mixer interface" entry the GUI shows but does not open.

The pairing rule and the ids are stored, never card numbers (the same
discipline as the controller work).

### 4.3 Backend pattern

A control-only backend is a normal `RmeDevice` implementation that:

- opens hwdep on the control card (its own ABI, like `fireface/hwdep.rs`),
- implements the protocol codec (device-specific: matrix words, registers,
  meters),
- reports `card_id()` = the paired **audio** card id,
- gates capabilities on what the kernel module reports (the ABI capability
  flags), so a partially implemented driver shows honest unavailable controls.

The framework provides the pairing, the audio/control separation, the
device-service integration and the gating; the contributor provides the codec,
the profile and the descriptor entry.

### 4.4 App integration

- A new `DeviceKind` variant and `DeviceHandle` variant per control-only device
  (or one generic `ControlOnly` variant parameterized by the descriptor — decide
  at implementation; the generic variant avoids touching the `delegate!` macro
  for every new device).
- `available_targets()` consults the descriptor table.
- `card_id()` returns the audio card id for a control-only handle.
- The device menu shows one entry; the "no mixer interface" state is shown but
  not openable.

## 5. Concrete steps (phased)

| Phase | Work | Files |
|---|---|---|
| A | Make the control/audio distinction explicit: add an audio-card id path distinct from the hwdep card; keep Fireface/Babyface unchanged (audio = control) | `fireface/device.rs`, `app.rs` `card_id` |
| B | Descriptor table + pairing discovery + `CardMatch`; register an empty table initially | new `totalcontrol-core/src/control_only.rs` (or `discovery.rs`), `app.rs` `available_targets` |
| C | Generic control-only `DeviceHandle` path + device-service integration + capability gating | `app.rs`, `device_service.rs` |
| D | Mock control-only device + tests: pairing rule (synthetic card lists), audio/control id separation, GUI entry, gating, "no mixer interface" state | `mock.rs` or a test backend, `app.rs` tests |
| E | Documentation: the control-only pattern in CONTRIBUTING-NEW-DEVICE and the architecture; update the device profile spec when the first real profile lands | docs |

Phases A–D are the refactor and are hardware-independent. The Digiface backend
and profile are a later contribution that registers a descriptor entry.

## 6. What a future contributor adds

1. A kernel control module exposing hwdep (policy: in-tree, whitelisted
   requests), with capability flags in INFO.
2. A verified protocol specification.
3. A profile (`ProfileId`, channel table, capabilities, meter layout).
4. A backend implementing `RmeDevice` over the hwdep ABI.
5. One `ControlOnlyDescriptor` entry.
6. Tests on the unit; every specified function reachable in the GUI.

The framework (pairing, audio/control separation, device service, gating,
mock) is already there.

## 7. Risks and open questions

| Item | Note |
|---|---|
| Speculative abstraction | Mitigated by the mock and by keeping the change minimal; no grand trait layer. Reassess after phase D whether the generic variant earned its keep. |
| Generic vs per-device `DeviceHandle` variant | Decide at phase C; the generic variant scales better but must fit the `delegate!` macro and the mirror info. |
| Audio-card matching robustness | Match by id/description, never card number; cover the "audio card absent" and "control card absent" states in tests. |
| Capability flags ABI | The control-only INFO needs a capability bitmask; agree it with the first kernel module (the Digiface uapi review). |
| Clock/latency model | The Digiface audio path is mainline's; the latency line must use the audio card, not the control card. Verify in phase A. |

## 8. Validation

- `cargo test -p totalcontrol-core --lib --features alsa` and
  `cargo test -p totalcontrol-gui` stay green; the Fireface/Babyface paths are
  unchanged.
- New tests: pairing with synthetic card lists; audio/control id separation;
  the mock control-only device opens, gates and shows one entry; the "no mixer
  interface" state.
- `python3 scripts/check-docs.py` introduces no new errors.