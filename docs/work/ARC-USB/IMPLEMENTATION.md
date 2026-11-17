# ARC USB implementation plan

Working note. [`CONTROLLER_MODEL.md`](CONTROLLER_MODEL.md) owns the engine
and the definition format. This file keeps the concrete wiring for the GUI
and the tests.

## 1. Shape

One engine, one instance per enabled input port, no controller-specific menu.
The first shipped definition is Mackie Control; the ARC definition joins with
its tested-support pull request.

| Part | Where |
|---|---|
| Definition format, loader, search order | `CONTROLLER_MODEL.md` |
| Mackie wire values | `MACKIE_MIDI.md`, `MAPS/Mackie_MIDI.json` |
| ARC wire values | `RME_ARC_USB_MIDI.md`, `MAPS/RME_ARC_USB_MIDI.json` |
| Engine | new `ui/totalcontrol/totalcontrol-gui/src/controllers/` module |
| Packaged files | `data/midi-controllers/` |

`mackie.rs` stays as the parity oracle while the engine lands: the engine's
decode, dispatch and feedback must reproduce it byte for byte on the Mackie
map. `midi.rs` supplies the worker the engine generalises to every
controller.

## 2. Files

| Concern | File |
|---|---|
| Definition types, parse, validate | new `controllers/map.rs` |
| Argument expressions | new `controllers/expr.rs` |
| Loader, search order, embedded fallback | new `controllers/load.rs` |
| Device discovery | new `controllers/discover.rs` |
| Decode, dispatch, feedback | new `controllers/engine.rs` |
| Engine worker | `midi.rs`, generalised to `ControllerWorkerConfig` |
| Messages and dispatch | `app.rs` |
| Settings persistence and migration | `host_settings.rs` |
| Settings card | the MIDI part of `Options > MIDI and OSC` |
| Shipped maps | `data/midi-controllers/*.json` |
| Install rule | `Makefile`, `packaging/fedora`, Debian packaging |

## 3. Instances

An instance is one enabled input port with its protocol. The unit's own
ports are instances as they are today; a USB controller card adds instances.
Each instance owns its worker, parser, navigation, encoder target, feedback
cache and pressed set. Option values are shared per protocol id.

| `port.source` | Discovery |
|---|---|
| `device` | `midi::device_ports(card_id, count)` on the open audio device's card |
| `card` | the USB card whose id or description matches, device indices 0..`devices` |

The card id for the ARC is `ARC`. Never store the card number.

## 4. Messages

```rust
ControllerReady { instance: usize, tx: Sender<Vec<u8>> },
ControllerIn { instance: usize, bytes: Vec<u8> },
SetControllersEnabled(bool),
SelectControllerDevice(usize),
SetControllerIn(usize, bool),
SetControllerOut(usize, bool),
SetControllerProtocol(usize, String),
SetControllerOption { protocol: String, key: String, value: ControllerValue },
SetControllerAssignment { protocol: String, control: String, action: String },
ResetControllerKeys(String),
ToggleControllerLog,
ClearControllerLog,
```

Assignments and option values write to the host settings
(`CONTROLLER_MODEL.md` §10), never to a map file.

## 5. Worker

Extract the read loop of `midi::worker` into one helper that takes a port
pair and a message constructor. `MackieWorkerConfig` becomes
`ControllerWorkerConfig { instance, input, output }`. The read timeout,
shutdown and write-error handling stay in one place.

## 6. Decode

```rust
pub enum ControlEvent {
    Press { control: usize, index: usize },
    Release { control: usize, index: usize },
    Value { control: usize, index: usize, value: f32 },
}
```

Rules:

| Input | Handling |
|---|---|
| running status | expanded |
| Note On below the press threshold, Note Off | release |
| split message | kept in the parser state across reads |
| SysEx | framed; matched against `sysex` patterns, else the handshake |
| realtime | passed through, otherwise ignored |
| unknown note or CC | ignored, logged once per address |
| cc_pair | MSB and LSB accumulated into one 14-bit value |

The loader expands every control over its `expand` axis and rejects two
controls covering one address. Decode is a lookup, not a per-controller
branch.

## 7. Dispatch

`engine.rs` maps an action name and argument to existing app messages.

| Action | Message |
|---|---|
| `snapshot` | `Message::SnapshotClicked(n)` |
| `dim`, `mono`, `talkback`, `speaker_b` | `Message::RmeToggle(MainDim / MainMono / MainTalkback / MainSpeakerB)` |
| `external_input` | `Message::ExternalInput(on)` |
| `recall_volume` | `Message::ControlRoomRecall` |
| `mute_main`, `master_solo` | `Message::MackieAction(MasterMute / MasterSolo)` |
| `global_mute`, `global_solo` | `Message::RmeToggle(GlobalMute / GlobalSolo)` |
| `cue_main`, `select_cue_main`, `cue_phones` | `Message::MackieAction(...)` |
| `encoder_target` | instance state, no mixer write |
| `encoder_target_volume` | `Message::EncoderNudge { target, db }` |
| `layout_preset` | `Message::LayoutClicked(n)` |
| `strip_*`, `master_volume`, `row_fader`, `submix_select*`, `navigate`, `trim_*`, `input_gain` | the Mackie equivalents |
| `mute_group`, `solo_group`, `fader_group`, `toggle_window` | unavailable; marked in the dialog, inert when received |

Momentary controls write on while held and off on release, through the
direct set messages (`TalkbackChanged(on)` and friends).

## 8. Toggle resolution

| Action kind | Resolution |
|---|---|
| `press` mode | read the model value, write the opposite |
| `momentary` mode | on while held, off on release |
| `snapshot` | recall the slot |
| `encoder_target` | set the target, no mixer write |

A held key sends one press and one release. No repeat.

## 9. Encoder

The encoder settings are map options, read by the engine behaviours
`encoder_target`, `encoder_step` and `encoder_follows_selection`.

The device reports 1–15 steps. The standard step is the fader curve's 0.5 dB
region; the fine step is 0.1 dB. Calibrate against a unit and record the
result in `RME_ARC_USB_MIDI.md` §7.4. Accumulate fractional steps so a spin
of ten clicks keeps the sub-step remainder.

## 10. Feedback

| Step | Behaviour |
|---|---|
| After connect | write every `sync` entry |
| After a model change | recompute, diff against the last sent, write changes |
| On disconnect | clear the cache |
| An entry with `enabled_by` | active only while that option is on |
| `action_state` | derived from the bound control's action, `CONTROLLER_MODEL.md` §4.9.2 |

The Mackie handshake, LCD and meters and the ARC LED use the same diff loop.
Parity is the multiset of emitted MIDI messages per refresh; the wire order
of independent messages carries no meaning.

## 11. Settings

```rust
pub struct ControllerSettings {
    pub enabled: bool,
    pub show_log: bool,
    pub devices: BTreeMap<DeviceKey, DeviceChoice>,   // card id + midi index
    pub values: BTreeMap<String, BTreeMap<String, ControllerValue>>,
    pub assignments: BTreeMap<String, BTreeMap<String, Assignment>>,
}

pub enum ControllerValue { Bool(bool), Choice(String) }
```

`#[serde(default)]` on every field so `host-settings.json` keeps loading.
The legacy `mackie` field is read once and folded into the unit-port device
choices and `values["mackie_control"]`; the field is omitted when the file is
rewritten.

## 12. Tests

Unit tests, no hardware:

| Test | Expectation |
|---|---|
| Mackie decode parity | every note, CC and pitch bend of the current codec, same messages |
| Mackie feedback parity | same message multiset as `mackie::refresh`, empty second refresh |
| handshake reply | same four bytes as the current code |
| Mackie defaults | protocol, full LCD, user names, submix linked on |
| map validation | unknown action, behaviour, state, encoding, duplicate id, address overlap rejected |
| argument expressions | row fader law, submix law, snapshot law |
| ARC keys | 15 keys and the footswitch decode; encoder sign and magnitude |
| option rendering | file order, groups, `requires`, `break` |
| search order | env, user, shipped, embedded; first match wins |
| settings round-trip | `host-settings.json` loads and saves; `mackie` migration |

Integration tests:

- dispatch every action through the app and assert the model change;
- run the actions against every profile and skip unavailable ones;
- reconnect with a synthetic stream and check the full feedback sync.

Hardware test: `TESTING.md` §4.

## 13. Specification work

| Document | Work |
|---|---|
| `docs/spec/TOTALCONTROL-CONTROLLER-SPEC.md` | new, from `CONTROLLER_MODEL.md` |
| `docs/spec/midi-controllers/MACKIE_MIDI.md` | new, split from the MIDI spec |
| `docs/spec/midi-controllers/RME_ARC_USB_MIDI.md` | with the ARC pull request |
| `docs/spec/TOTALCONTROL-MIDI-SPEC.md` | reduced to the transport |
| `docs/spec/TOTALCONTROL-GUI-SPEC.md` | §1.4 rewritten for the controller dialog |
| `docs/spec/TOTALCONTROL-STATE-SPEC.md` | §7.1 field list |
| `docs/TOTALCONTROL-Manual.md` | controller selection and the map files |
| `docs/spec/README.md` | index |
| `THIRD-PARTY.md` | the MIT entry |

## 14. Open items

| Status | Item |
|---|---|
| needs-capture | Encoder push message |
| needs-capture | SysEx mode format |
| open | Standard-step curve |
| open | Momentary Talkback on a unit |
| open | Second unit behaviour |
| open | Group and layout actions, once the model has them |