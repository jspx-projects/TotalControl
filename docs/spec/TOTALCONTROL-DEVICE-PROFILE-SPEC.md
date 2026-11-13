# Device profiles

This specification defines how TotalControl selects one declarative device
profile per `(model, transport)`, and what that profile owns relative to the
driver, the protocol specifications and host state.

Visible behaviour is in
[`TOTALCONTROL-GUI-SPEC.md`](TOTALCONTROL-GUI-SPEC.md). Workspace, autosave and
host settings are in [`TOTALCONTROL-STATE-SPEC.md`](TOTALCONTROL-STATE-SPEC.md).
Wire formats, registers, decode laws and transport sequences are in the device
protocol specifications.

## 1. Terms

| Term | Meaning |
|---|---|
| Model | The compatibility name of the unit (`uc`, `ucx`, `babyface_classic`). A workspace is compatible by model. |
| Transport | The USB transport the driver reports for the connection: interrupt or isochronous. |
| Profile | The complete declarative description of one `(model, transport)` connection. |
| Profile id | The stable identifier of a profile. |
| Profile table | The closed, compiled-in table of supported profiles. It has one entry per supported `(model, transport)` and is the only source of profile selection. |
| Reported fact | A connection value the driver reports in `HwdepInfo`. |
| Requirement | A minimum or exact value a profile needs from a reported fact. |
| Profile data | A value supplied by the profile. |
| Host operation | An action TotalControl performs through the control interface. |
| Front-panel access | The operations and readouts available on the unit's own controls. The protocol specification documents it. |

## 2. Profile table

The profile table is compiled into the application binary and versioned with
the code. It contains one entry per supported profile.

| Profile id | Model | Transport | Unit | Support |
|---|---|---|---|---|
| `uc_pc` | `uc` | interrupt | Fireface UC, PC | production |
| `uc_ap` | `uc` | isochronous | Fireface UC, AP | production |
| `ucx_iso` | `ucx` | isochronous | Fireface UCX | production |
| `ucx_if5` | `ucx` | interrupt | Fireface UCX, IF5 | production |
| `babyface_classic` | `babyface_classic` | isochronous | Babyface-classic | experimental |

Babyface-classic support is experimental. Its profile covers the implemented
control surface; the application labels the device as experimental.

The profile table covers devices controlled through the hwdep vendor interface.
Profile selection requires the hwdep node.

One profile exists per `(model, transport)`. USB product IDs that report the
same model and transport select the same profile. Transport behaviour that
belongs to a USB product ID is owned by the driver.

## 3. Selection

The driver reports the model and transport. TotalControl selects the profile id
from those two values. This function is the only profile selection.

| Connection state | Condition | Behaviour |
|---|---|---|
| `Vendor` | A hwdep node exists and the model and transport select a profile | The profile is active; the mixer, meters and scene writes are available. |
| `ClassCompliant` | The device reports a known model and transport in class-compliant mode | The application names the device and directs the user to vendor mode. The mixer surface is available in vendor mode. |
| `Unsupported` | The model and transport are outside the profile table | The application reports an unsupported device and offers the simulation. |

Class-compliant mode is a connection state. A known device is named, and the
application directs the user to vendor mode before mixing.

> **Note:** the UCX and Babyface-classic expose their mixer in class-compliant
> mode over MIDI SysEx on the USB MIDI port. The UC operates in vendor mode
> only. Profile selection in this specification covers the hwdep path.

## 4. Ownership

A fact has exactly one owner. The driver owns reported connection facts; the
profile owns interpretation.

| Category | Owner | Runtime rule |
|---|---|---|
| Reported fact | driver | Read from `HwdepInfo`. |
| Requirement | profile | Checked once at connect against the reported fact. A failed requirement blocks the connection and is reported as a profile/connection mismatch. |
| Profile data | profile | The only source. Includes names, order, maps, scales, ranges, operations, template selection and defaults. |
| Host setting | host settings | Overrides a profile default where the setting is defined. |

The reported value is authoritative at runtime. The protocol specification is
authoritative for the meaning of that value. A requirement gates the
connection; the reported value remains authoritative.

A profile states requirements for structure it interprets: the channel counts
it names, the meter packet length its layout decodes, and the matrix shape it
addresses.

The ALSA card id used to locate the audio card is a system fact. The
application reads it from the card associated with the hwdep node.

## 5. Schema

| Field | Contents |
|---|---|
| `id` | Profile id |
| `model` | Compatibility model |
| `display_name` | Name shown for the unit |
| `channels` | Inputs, outputs, playback pairs and output display order |
| `mixer` | Matrix shape, fader range, pan law and mute/phase semantics |
| `operations` | Host operations for the input, playback and output rows |
| `globals` | Host-operable device-wide controls |
| `clock_sources` | Clock sources offered by the clock-source control, in order |
| `preamps` | Per-input type, gain range and step, switches, references and read policy |
| `meters` | Meter layout id and whether hardware and playback meters exist |
| `layout` | Template id and control flags |
| `requires` | Preconditions on reported facts |

The profile contains device data. Geometry belongs to the layout document;
user state belongs to the workspace.

## 6. Accessible channels

`channels` defines the rows the host can address.

| Field | Contents |
|---|---|
| `inputs` | One entry per input: display name and input type |
| `outputs` | One entry per hardware output pair in device block order: left and right display name |
| `playback` | One entry per software playback pair in device order |
| `output_order` | The display position of each output pair |

Channel ids are the device ids: input `i`, playback `2p`/`2p+1`, output pair
`p`. The profile fixes the mapping; the GUI and the scene use it unchanged.

A profile lists every row the host can address. The device protocol
specification documents the unit's front panel.

User channel labels are workspace state; the profile supplies the factory
names.

## 7. Host operations

`operations` lists the host operations for each channel class.

| Field | Contents |
|---|---|
| `inputs` | Operations on hardware input rows |
| `playback` | Operations on software playback rows |
| `outputs` | Operations on hardware output rows |

Each operation declares whether the host can set it and how the host reads it.

| Field | Contents |
|---|---|
| `set` | The host can write the value |
| `read` | `device`, `host` or `none` |

| Read policy | Meaning |
|---|---|
| `device` | The host reads the current value from the unit |
| `host` | The host scene is authoritative; the unit has no readback |
| `none` | The operation is not available |

| Operation | Rows | Meaning |
|---|---|---|
| `send` | input, playback | Level into an output pair |
| `routing` | input, playback | Pair routing or individual matrix cells |
| `pan` | input, playback | Pan into an output pair |
| `mute` | all | Channel mute |
| `solo` | all | Channel solo |
| `phase` | input, output | Phase invert |
| `trim` | input, playback | Stored level offset |
| `name` | all | Host channel name |
| `link` | input, playback, output | Pair link and split |
| `eq` | input, output | Three-band EQ |
| `low_cut` | input, output | Low cut |
| `dynamics` | input, output | Compressor, expander and Auto Level |
| `fx_send` | input, playback | FX send level |
| `fx_return` | output | FX return level |
| `loopback` | output | Loopback tap |
| `width` | input, playback | Stereo width |
| `ms` | input, playback | M/S processing |
| `no_trim` | output | No Trim flag |
| `talkback_receive` | output | Talkback injection |
| `volume` | output | Output volume |
| `balance` | output | Output balance |
| `gain` | input | Preamp gain |
| `phantom` | input | 48 V |
| `pad` | input | PAD |
| `instrument` | input | Instrument mode |
| `level` | input | Input reference level |
| `autoset` | input | AutoSet |

`routing` is `pair` when the profile addresses whole output pairs, and `cell`
when it addresses individual matrix cells.

`globals` lists the host-operable device-wide controls:

`clock_source`, `sample_rate`, `optical_format`, `spdif_professional`,
`word_clock`, `eq_for_record`, `talkback`, `dim`, `speaker_b`, `mono`, `pitch`,
`an12_link`.

Each global has the same `set` and `read` policy as a channel operation.

The UC and UCX expose the global controls listed above. Physical controls that
only the unit itself uses are part of the device, not of the profile.

Front-panel access is a per-operation property of the unit, and the protocol
specification documents both directions: which operations the front panel can
set, which values it reads, and the step of each value. On the UC and UCX the
front panel reads and sets input gain on inputs 1-4 and output volume on the
outputs, each with its own step size. The host interface additionally sets
sends, preamp switches and the remaining channel operations. `set` and `read`
in this specification describe the host interface and do not inherit a
front-panel step.

## 8. Preamps

`preamps` describes each input the unit exposes.

| Field | Contents |
|---|---|
| `type` | Mic, line, instrument, S/PDIF, ADAT or loopback |
| `gain` | Minimum, maximum and step |
| `switches` | Phantom, PAD, instrument, level and AutoSet, where the input has them |
| `references` | Reference levels the input supports |
| `read` | `device`, `host` or `none` for each switch |

A switch with read policy `host` is host-written from the scene, and the GUI
shows it as host-held.

Reference levels for line inputs, line outputs and phones are profile data.

## 9. Meters

`meters` selects a layout by id. A protocol layout variant is an alternative
encoding of the same meter data, such as two page formats of the UC meter
packet. The protocol specification is the single definition of the packet, the
record order, the scales, the page discriminator and the variant identification
for that id.

| Field | Contents |
|---|---|
| `layout` | Protocol meter layout id, or none |
| `hardware` | The unit publishes its own Peak/RMS meters |
| `playback` | The unit publishes software playback meters |

Release and peak hold are GUI behaviour; the device publishes raw Peak/RMS
values. When `layout` is none, the host audio tap supplies the levels.

## 10. Layout

`layout` selects the channel template and the control flags a unit uses.

The GUI layout document defines geometry, tokens and colours. The profile
selects which template and which controls are present. The profile's host operations
are authoritative: a layout override selects among the controls the profile
supports.

## 11. Scene and workspace compatibility

The scene schema is single and model-independent. The profile supplies the
host operations that gate its field groups.

| Rule | Behaviour |
|---|---|
| Workspace compatibility | Canonical model |
| Unit selection | Serial when available |
| Workspace load | The canonical model must match; a different model refuses the load |
| Scene apply | Fields gated off by the active profile are skipped and counted |
| Reporting | The GUI reports the number of skipped settings |
| Transport switch | PC/AP or ISO/IF5 on the same unit reuses the same workspace |
| Profile selection | Uses the unit's existing workspace |

A profile switch preserves the saved state of the other transport mode.
Settings outside the active profile's host operations are skipped and reported;
the rest is applied.

## 12. Contract tests

The implementation is checked by hardware-free tests:

| Test | Assertion |
|---|---|
| Profile table completeness | Every reported `(model, transport)` selects one profile, and every profile id is unique |
| Requirement check | A synthetic `HwdepInfo` satisfies or fails a profile requirement as intended |
| Operation consistency | Every scene field group has a gate, and every gate names a host operation |
| Layout reference | Every meter layout and template id resolves |
| Transport equivalence | Profiles that share a model differ only in the fields the specification classifies as transport-specific |

The equivalence test covers `uc_pc` against `uc_ap`, and `ucx_iso` against
`ucx_if5`. A new field must be classified as transport-specific before it may
differ between the two.
