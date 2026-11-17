# MIDI controller definitions

This document would become `docs/spec/TOTALCONTROL-CONTROLLER-SPEC.md`. It
defines what a MIDI controller is in TotalControl, how a controller is
described by a definition file, how the engine runs it, and how the settings
dialog presents it.

Its purpose is one engine for every controller. The controller is a USB MIDI
device the user picks. The protocol is a definition file that describes the
device's messages. Mackie Control and the RME ARC USB differ in their wire
values and in their options, not in their machinery.

| Part | Status |
|---|---|
| Device discovery | specified, not implemented |
| Definition file format | specified, not implemented |
| Loader and search order | specified, not implemented |
| Engine | specified |
| Settings dialog | specified, not implemented |
| Shipped definition files | written |

## Contents

- [1. Terms](#1-terms)
- [2. Why a general model](#2-why-a-general-model)
- [3. Controller anatomy](#3-controller-anatomy)
- [4. Definition file format](#4-definition-file-format)
- [5. Device discovery](#5-device-discovery)
- [6. Where the files live](#6-where-the-files-live)
- [7. Loading](#7-loading)
- [8. Engine](#8-engine)
- [9. Settings dialog](#9-settings-dialog)
- [10. Persistence](#10-persistence)
- [11. Shipped definitions](#11-shipped-definitions)
- [12. Adding a controller](#12-adding-a-controller)
- [13. Open items](#13-open-items)

## 1. Terms

| Term | Shape | Definition |
|---|---|---|
| device | ALSA rawmidi port | A USB MIDI port the user selects: a card and one rawmidi device on it. |
| controller | device | A human interface that sends messages to TotalControl and receives feedback. |
| protocol | definition file | The file that describes the messages of one controller model. |
| instance | one enabled input port | One running controller: a device, a protocol and its engine state. |
| match | string list | The device description fragments that preselect a protocol. |
| shipped file | file in the data directory | The file installed with TotalControl. |
| user file | file in the user data directory | A copy of a shipped file that the user edited; it overrides the shipped file. |
| engine | code | The shared code that opens, parses, dispatches, resolves and sends feedback. |
| action | name | A TotalControl operation a control can trigger, for example `dim`. |
| state | name | A model value a feedback entry carries, for example `strip.mute`. |
| parameter group | section | A heading in the dialog that collects the options of one area: Mapping, Keys, Encoder, Surface, Feedback. |
| presentation | object | How one option appears: a label, a checkbox, a radio row, a listbox or a heading. |

## 2. Why a general model

Mackie Control and the ARC USB share everything except their tables:

| Shared | Mackie | ARC |
|---|---|---|
| Device discovery | USB MIDI ports of a unit | its own USB MIDI card |
| Reader, parser, worker, reconnect | yes | yes |
| Dispatch to mixer messages | yes | yes |
| Toggle resolution against the model | yes | yes |
| Feedback diff and full sync | yes | yes |
| Debug log | yes | yes |

A controller therefore needs a definition file, not code. The engine runs one
path for every file.

## 3. Controller anatomy

A definition names one shape. The shape selects the engine machinery, never
which actions exist.

| Shape | Definition | Example |
|---|---|---|
| `remote` | Independent controls, each firing one action. No channels, no navigation. | RME ARC USB |
| `surface` | Channel strips over a window of channels, with navigation, a master control and a screen. | Mackie Control |
| `keys` | Raw messages mapped to actions, with no feedback and no window. | A pad or foot controller |

## 4. Definition file format

The file is UTF-8 JSON. Schema 1 is the only syntax; a line-oriented layout
syntax is deferred (§13).

### 4.1 Top level

| Key | Type | Required | Meaning |
|---|---|---|---|
| `schema` | integer | yes | Format version; this document defines 1. |
| `id` | string | yes | Stable identifier, lower case, underscores. |
| `name` | string | yes | Label shown in the protocol list. |
| `version` | string | yes | File version, for the dialog line and the log. |
| `shape` | string | yes | `remote`, `surface` or `keys`. |
| `match` | string array | no | Device description fragments that preselect this protocol by name, §5.3. |
| `channels` | integer | no | Number of channel strips; 0 for a remote. |
| `port` | object | no | The port source and defaults of §4.2. |
| `options` | array | yes | The option presentation entries of §4.6, in display order. |
| `assignment` | object | no | The assignment table of §4.5. |
| `controls` | array | yes | The control entries of §4.3. |
| `feedback` | array | no | The feedback entries of §4.4. |
| `numeric` | object | no | Named numeric encodings of §4.7. |
| `boolean_encodings` | object | no | Named on/off/other output values of §4.8. |
| `extra` | object | no | Protocol-specific data of §4.10. |

A key outside this document is logged and ignored. A file with a `schema`
above the engine's is rejected with a clear message.

### 4.2 Port block

| Key | Type | Meaning |
|---|---|---|
| `source` | string | `device`: the open audio unit's own MIDI ports. `card`: a separate USB card. |
| `match` | object | For `card`: `card_id` and `description` fragments that find the card. |
| `devices` | integer | How many rawmidi devices the controller uses. |
| `direction` | string | `duplex` or `input`. |

The port block declares where the controller lives. Which ports are enabled
is instance state, stored in the host settings (§10), not in the file.

### 4.3 Control entry

| Key | Type | Used by | Meaning |
|---|---|---|---|
| `id` | string | all | Stable name, unique in the file; the assignment row key. |
| `label` | string | all | Text shown in the assignment table. |
| `kind` | string | all | `note`, `cc`, `cc_pair`, `pitch_bend`, `channel_pressure`, `program_change`, `sysex`. |
| `note` | integer | note | Note number, 0–127. |
| `message` | integer | cc, cc_pair, channel_pressure | CC number. For `cc_pair` the MSB controller; the LSB is `message + 32`. |
| `channel` | integer | channel messages | MIDI channel, 1–16. Omitted: any channel. |
| `channel_range` | [int, int] | channel messages | Inclusive channel range the entry covers, instead of one `channel`. |
| `pattern` | string | sysex | Hex bytes with `??` wildcards, starting `f0`, ending `f7`. |
| `value_byte` | integer | sysex | Index of the payload byte carried as the value. |
| `expand` | object | all | `{ "axis": "note" \| "cc" \| "channel" \| "index", "count": n }`. The entry covers `count` consecutive addresses on the axis, starting at `note`, `message` or `channel`. `index` expands without an address change; the instance index feeds `index_nibble` feedback and the `strip` expression variable. |
| `value` | string | value controls | Numeric encoding name from `numeric`. The control fires a value event on every message. |
| `press` | integer | button controls | Velocity or CC threshold: at or above it the control is pressed. Default 64. Below it, and note off, is release. |
| `mode` | string | button controls | `press`: fire the action on press, the engine resolves the state. `momentary`: on while held, off on release. Default `press`. |
| `mode_option` | object | button controls | `{ "key", "value" }`: the control is `momentary` while that option holds that value. |
| `simple` | boolean | all | The control works while `protocol_support` is off. Default false. |
| `action` | string | all | The action of §4.9. |
| `argument` | int, string or object | some actions | A constant, `{ "option": key }` reading an option value, or `{ "expr": text }` evaluated per message, §4.9.1. |

A control carries either `value` or button semantics (`press`/`mode`), not
both. Two expanded controls may not cover one message address; the loader
rejects the overlap.

### 4.4 Feedback entry

| Key | Type | Meaning |
|---|---|---|
| `id` | string | Stable name, unique in the file. |
| `enabled_by` | string | Option key; the entry is active only while that option is on. |
| `kind` | string | `note`, `cc`, `pitch_bend`, `channel_pressure`, `sysex`. |
| `address` | object | `note`, `cc` or `channel`, with optional `expand` and `index_nibble`. `note: "control"` binds one entry per note control of the file, at that control's note. `index_nibble: "high"` puts the expansion index in the high nibble of the value byte. |
| `state` | string | The model state of §4.9.2 the entry carries. |
| `value` | string | Encoding name from `numeric` or `boolean_encodings`. |
| `sync` | boolean | Send the full state after connect. |
| `rate_hz` | number | Send rate for a continuous entry. |
| `period_ms` | number | Send period for a periodic entry, for example the loopback note. |

### 4.5 Assignment table

The dialog offers one action picker per assignable control.

| Key | Type | Meaning |
|---|---|---|
| `enabled_by` | string | Option key; the table is shown only while that option is on. |
| `controls` | string array | The control ids the user can assign, in row order. |
| `choices` | string array | The action names offered, from §4.9. |

An assignment is stored in the host settings under the protocol id and the
control id (§10). The definition file keeps the shipped default assignment.

### 4.6 Option presentation

Every option is one object. The order of the array is the order on screen.

| Key | Type | Required | Meaning |
|---|---|---|---|
| `key` | string | yes | Stable name, unique in the file; the stored value key. |
| `label` | string | yes | Text next to the control, or the heading text of a `text` entry. |
| `help` | string | no | Tooltip. |
| `form` | string | yes | `checkbox`, `radio`, `listbox`, `text` or `button`. |
| `group` | string | no | Parameter group: `mapping`, `keys`, `encoder`, `surface`, `feedback`. |
| `break` | boolean | no | Start a new presentation row before this entry. |
| `choices` | array | radio, listbox | Each `{ "value", "label" }`. |
| `default` | bool, string or `null` | yes | The value the entry starts with, and the value a reset restores. `null` for `text` and `button`. |
| `requires` | string | no | Option key that must be on for this entry to be available. |
| `maps_to` | string | no | Engine behaviour name from §4.11. |

| Form | `default` | Stored value |
|---|---|---|
| `checkbox` | `true` or `false` | boolean |
| `radio` | one `value` of `choices` | choice string |
| `listbox` | one `value` of `choices` | choice string |
| `text`, `button` | `null` | nothing |

A `radio` or `listbox` default outside its `choices` is rejected at load. A
reset restores every entry to its `default`. Schema 1 renders one entry per
row; the choices of a `radio` sit side by side on its row. A `text` entry is
a heading. A `button` runs its behaviour and stores nothing. `help` and
`break` are accepted and reserved: tooltips and multi-column rows are open
items (§13).

### 4.7 Numeric encodings

`numeric` maps a name to how a raw value becomes a model value, and back for
feedback.

| Encoding | Kind | Meaning |
|---|---|---|
| `absolute_7bit` | absolute | `v / 127` |
| `absolute_14bit` | absolute | `v / 16383`; `order` is `low_first` (default) or `high_first` |
| `relative_7bit` | relative | 1–63 up, 65–127 down, magnitude as reported |
| `relative_signed_bit` | relative | bit 6 is the direction bit, magnitude in the low bits; `max_step` clamps it |
| `ring_position` | absolute | 0–11 dot position, bit 6 the centre LED |
| `mackie_meter_nibble` | absolute | the meter scale of the Mackie document |
| `ascii` | text | SysEx payload text |
| `constant` | constant | `{ "value": n }`; the same byte every time |

A definition selects an encoding by name. An unknown name is rejected at
load.

### 4.8 Boolean encodings

`boolean_encodings` maps a name to the output values of a state-carrying
entry.

| Key | Value |
|---|---|
| `on` | `{ "kind": "note" \| "cc", "velocity": n }` |
| `off` | the same shape |
| `blink` | optional; a third state |

### 4.9 Action names

The action vocabulary is fixed by the engine.

| Action | Argument | Value |
|---|---|---|
| `none` | — | — |
| `snapshot` | slot 1–8 | — |
| `dim`, `mono`, `talkback`, `speaker_b`, `external_input`, `recall_volume` | — | — |
| `mute_main`, `global_mute`, `global_solo` | — | — |
| `mute_group`, `solo_group`, `fader_group` | group 1–4 | — |
| `cue_main`, `select_cue_main`, `cue_phones`, `mute_phones` | phones slot 1–4 for `cue_phones` | — |
| `encoder_target` | `main`, `phones1`–`phones4`, `none` | — |
| `encoder_target_volume` | — | relative steps |
| `layout_preset` | 1–6 | — |
| `toggle_window` | — | — |
| `strip_volume`, `strip_pan`, `strip_pan_center`, `strip_mute`, `strip_solo`, `strip_select` | strip index | position, steps or — |
| `row_fader` | flat row channel index | position |
| `master_volume` | — | position |
| `master_mute`, `master_solo` | — | — |
| `input_gain` | input index | raw gain |
| `navigate` | `row_up`, `row_down`, `bank_left`, `bank_right`, `channel_left`, `channel_right` | — |
| `trim_mode`, `trim_on`, `trim_off` | — | — |
| `submix_select` | strip index | — |
| `submix_select_index` | output pair index | — |

`select_cue_main` selects the Main Out submix and cues it, in that order.
`row_fader` writes the channel at the flat index: row `idx / 64`, channel
`idx % 64`, rows in input, playback, output order. `submix_select` reads the
output under the strip; `submix_select_index` takes the pair index directly.

An action whose target is absent from the current model is unavailable: the
dialog marks it, and the engine ignores it. `mute_group`, `solo_group`,
`fader_group` and `toggle_window` are unavailable until the model carries
them.

#### 4.9.1 Argument expressions

`{ "expr": text }` is integer arithmetic evaluated per message. Variables:

| Variable | Value |
|---|---|
| `channel` | MIDI channel, 1–16 |
| `note` | note number |
| `cc` | CC number |
| `value` | raw value byte |
| `strip` | expansion index, 0-based |

Operators: `+`, `-`, `*`, `/`, `%`, parentheses, integer literals. Division
truncates toward zero. Example, the simple CC row faders:
`((channel - 1) / 4) * 64 + ((channel - 1) % 4) * 16 + (cc - 102)`.

#### 4.9.2 Feedback states

| State | Value |
|---|---|
| `action_state` | the state of the action bound to the addressed control: a toggle's model value, a snapshot slot's currency, an encoder-target match, a strip mute/solo/select, a submix-select match |
| `strip.mute`, `strip.solo`, `strip.select`, `strip.rec` | the window strip's state; `strip.rec` is true while the strip's output pair is the selected submix |
| `strip.fader`, `master.fader` | normalized position 0–1 |
| `strip.pan` | pan −64..63 |
| `main.dim`, `main.talkback`, `main.mono`, `main.speaker_b`, `main.mute`, `main.solo`, `main.rude_solo` | Control Room state |
| `nav.trim` | Trim Gains mode |
| `meter` | the strip level in dB, through the meter encoding |
| `lcd` | the 112-byte LCD layout; the `full_lcd` and `send_user_names` behaviours select full or brief layout and user names |
| `loopback` | the loopback-detection note; the engine sends it at `period_ms` and disables the instance when it hears it on the input |

### 4.10 Protocol block

`extra` holds what the protocol needs beyond the general model.

| Key | Shape | Meaning |
|---|---|---|
| `handshake` | string or `null` | `mackie`: the SysEx device query, challenge/response connection handshake and go-offline of the Mackie document. `null`: none. |
| `tables` | object | Tables the protocol document defines, for example the assignable action list. |

An `extra` key the engine does not know is rejected at load.

### 4.11 Engine behaviours

An option with `maps_to` names one behaviour from this table. A definition
rejects a `maps_to` value outside it.

| Behaviour | Effect |
|---|---|
| `protocol_support` | accept the full surface protocol; off leaves the simple CC path |
| `full_lcd` | the eight-name LCD layout; off writes the brief layout |
| `send_user_names` | user channel names in the LCD text |
| `send_level_messages` | meter feedback |
| `peak_hold` | meters follow the held peak |
| `stereo_to_mono` | one strip per channel |
| `submix_any_row` | submix selection from any row |
| `submix_linked` | the remote follows the mixer's submix |
| `background_disable` | suspend input and feedback while unfocused |
| `led_feedback` | drive the key LEDs |
| `encoder_target` | the option value selects the encoder target |
| `encoder_step` | the option value selects the encoder step |
| `encoder_follows_selection` | the encoder changes the selected output |
| `recall_double_press` | a second recall press restores the pre-Dim volume |
| `enable_diag_leds` | the startup indicator on the first LED write |
| `snapshot_blink_store` | blink the snapshot key while a store is pending |

A behaviour that is specific to one protocol is listed as such in that
protocol's document.

## 5. Device discovery

### 5.1 The device lists

Device discovery reads `/proc/asound/cards` and the rawmidi devices under
`/proc/asound/card<N>/`. A card is a USB card when its device path resolves
under a USB bus.

| Class | Definition | Appears in |
|---|---|---|
| Device USB MIDI port | A rawmidi device of a USB card | the controller listbox |
| Unit MIDI port | A rawmidi device of the open audio device's own card | the controller listbox |

A device is identified by card id and rawmidi device index, with a display
string of the form `Card Name — MIDI 1`, for example `Digiface USB — MIDI 1`
or `ARC — MIDI 1`. The display strings are generated at open time. The stored
selection is matched against the card id and the rawmidi index, so enumeration
order never changes the choice.

### 5.2 Feedback output

A controller input arrives on one USB MIDI port. Its feedback leaves on the
same rawmidi device, opened for playback. Each instance has an input enable
and an output enable; control works with the output off.

### 5.3 Protocol preselection

The dialog preselects a protocol for a device by this rule, in order:

| Order | Rule |
|---|---|
| 1 | The stored protocol of the device, when the file still exists. |
| 2 | A shipped file whose file name equals the device's card id, ignoring case and separators. |
| 3 | A file whose `port.match` names the device's card id or a fragment of its description. |
| 4 | A shipped file whose `match` list contains a fragment of the device description. |
| 5 | Mackie Control, when the device is a Fireface unit MIDI port. |
| 6 | The first protocol in the list. |

A preselection is a starting value. The user changes it with the protocol
listbox of §9, and the change is stored per device.

## 6. Where the files live

### 6.1 The Linux convention

Installed, read-only data belongs in the data files directory.

| Path | Basis |
|---|---|
| `/usr/share/totalcontrol` | a build with `--prefix=/usr`, and an RPM package via `%{_datadir}` |
| `/usr/local/share/totalcontrol` | a build with `--prefix=/usr/local` |

The path comes from the build's `datadir`, `$prefix/share`. The program
compiles it in, so it finds its files after a move.

Per-user data belongs in the XDG user data directory.

| Path | Basis |
|---|---|
| `$XDG_DATA_HOME/totalcontrol` | the XDG Base Directory specification |
| `~/.local/share/totalcontrol` | the fallback when `XDG_DATA_HOME` is unset |

TotalControl uses this path for its workspaces
(`totalcontrol-core/src/workspace.rs`, `data_dir()`).

Two directories are reserved for other data:

| Path | Owner |
|---|---|
| `/etc` | administrator configuration |
| `$XDG_CONFIG_HOME` | settings the program writes, including `host-settings.json` |

### 6.2 Search order

The loader looks for a definition by name; first match wins.

| Order | Location | Purpose |
|---|---|---|
| 1 | `TOTALCONTROL_CONTROLLERS` directory, when set | tests and development |
| 2 | `$XDG_DATA_HOME/totalcontrol/midi-controllers` | user file |
| 3 | `@datadir@/totalcontrol/midi-controllers` | shipped file |
| 4 | the definitions embedded in the binary | a relocated binary without a data directory |

A user file with the same file name as a shipped file overrides it. A user
file with a new name adds a protocol. A malformed user file is reported and
skipped.

### 6.3 Packaging

| Item | Handling |
|---|---|
| Repository path | `data/midi-controllers/*.json` |
| Installed path | `$(datadir)/totalcontrol/midi-controllers/` |
| RPM | `TOTALCONTROL_DATADIR=%{_datadir}` at build; `%{_datadir}/totalcontrol/midi-controllers/*.json` in `%files` |
| Relocated binary | the embedded definitions of §6.2 order 4 |

The dialog line and the debug log show the resolved path.

## 7. Loading

1. List the directories in §6.2 order.
2. Read every definition file name from the first directory that holds it.
3. Parse and validate against §4.
4. Reject a file with an unknown `schema`, a missing required key, an unknown
   action, an unknown behaviour, an unknown state, an unknown encoding, an
   unknown `extra` key, two controls with one `id`, or two controls covering
   one message address.
5. Keep the file name, path and version for the dialog line and the log.
6. Sort the protocols by `name` for the listbox.

A parse error keeps the program running. The protocol is listed as
unavailable with the reason.

## 8. Engine

| Part | Responsibility |
|---|---|
| Discovery | build the device list of §5 |
| Worker | open the rawmidi pair, read bytes, forward to the UI thread |
| Parser | running status, SysEx framing, realtime passthrough |
| Decode | one control message to one `(control id, expansion index, press/release/value)` |
| Dispatch | action plus argument to shared mixer messages |
| Resolve | toggles and momentary actions against the confirmed model |
| Feedback | compute the target state, diff against the last sent, write |
| Sync | full feedback after connect and after a model reset |
| Guard | loopback detection and background suspension |

The engine keeps per-instance state: the device, the protocol, the parser
state, the navigation (surface shape), the encoder target, the last sent
feedback, the pressed set and the debug log ring. Several instances run at
once, one per enabled input port, each with its own protocol. Option values
are shared by every instance of one protocol.

## 9. Settings dialog

`Options > MIDI and OSC` holds the MIDI part. The rows, in order:

| Row | Control | Form |
|---|---|---|
| 1 | Enable | checkbox, the master switch of the MIDI part |
| 2 | Show MIDI debug log | checkbox |
| 3 | Controller | listbox: one entry per device of §5.1 |
| 4 | MIDI In / MIDI Out | two checkboxes for the selected device |
| 5 | Protocol | listbox: one entry per loaded definition, preselected by §5.3 |
| 6 | Protocol file | read-only line: file name and version |
| 7 | Options | the presentation of the selected definition, §4.6 |
| 8 | Assignment | the assignment table of §4.5, when present |

`Enable` switches the whole MIDI part. With `Enable` off the engine opens no
port. The device, protocol and option choices stay stored while it is off.
Every change applies at once. An option with `requires` is grayed out while
the requirement is off.

## 10. Persistence

| Data | Where | Why |
|---|---|---|
| Enable, log switch, per-device in/out and protocol, option values, assignments | host settings, `$XDG_CONFIG_HOME/totalcontrol/host-settings.json` | settings the program writes |
| Debug log | memory only | a diagnostic |

The host settings key the device by card id and rawmidi index, the option
values by protocol `id` and option `key`, and the assignments by protocol
`id` and control `id`.

| Field | Shape |
|---|---|
| `enabled` | boolean |
| `show_log` | boolean |
| `devices` | `{ card_id, midi } → { in: bool, out: bool, protocol: id }` |
| `values` | `{ protocol id } → { option key } → boolean or choice string` |
| `assignments` | `{ protocol id } → { control id } → { action, argument }` |

An option whose `key` leaves a file keeps its stored value until the file
returns. A device that is absent keeps its stored protocol and values. The
legacy `mackie` block is read once and folded into the Mackie device entries
and option values; it is omitted when the file is rewritten.

## 11. Shipped definitions

| File | `id` | `shape` | `match` |
|---|---|---|---|
| `Mackie_MIDI.json` | `mackie_control` | `surface` | `Fireface UC`, `Fireface UCX`, `Babyface` |

The ARC USB definition joins the shipped set with its tested-support pull
request. Both files carry a `version`. A shipped file is replaced only by a
user file of the same name.

## 12. Adding a controller

1. Capture the device's messages into a protocol document under
   `docs/spec/midi-controllers/`.
2. Write the definition file with the controls, the feedback, the options and
   the `match` list.
3. Add it to `data/midi-controllers/` and to the packaging.
4. Add the document to the `docs/spec/README.md` index.
5. Test with the synthetic byte streams of the document.

No engine change is needed unless the controller needs a new encoding, a new
action, a new state or a new behaviour.

## 13. Open items

| Status | Item |
|---|---|
| open | Whether the TUI gets the controller engine |
| open | The layout text syntax, deferred from schema 1 |
| open | Option tooltips (`help`) and multi-column rows (`break`) |
| open | Extender linking: several surface instances forming one fader block |
| open | Validation of the `sysex` control kind against a live SysEx controller |