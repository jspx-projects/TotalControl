# RME ARC USB MIDI specification

This document defines the RME ARC USB as a control surface: its USB
declaration, its MIDI wire format, its feedback, its default key assignment,
its options and the TotalControl host model. It follows the controller model
of [`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
and is described by the map file `RME_ARC_USB_MIDI.json`.

The ARC is independent of the audio interface. It drives the TotalControl
mixer through whichever device backend is open.

| Part | Status |
|---|---|
| USB declaration | verified against the vendor manual |
| Key, footswitch, encoder and LED wire values | verified against the vendor command list and a capture-based public implementation |
| Host model | implemented in the controller engine; live-unit verification open (§11) |
| SysEx mode | documented by the vendor; message format open (§11) |
| Live unit verification | open (§11) |

## Contents

- [1. Terms](#1-terms)
- [2. Transport](#2-transport)
- [3. Keys](#3-keys)
- [4. Encoder](#4-encoder)
- [5. Feedback](#5-feedback)
- [6. Default key assignment](#6-default-key-assignment)
- [7. Host model](#7-host-model)
- [8. Options list](#8-options-list)
- [9. Map file](#9-map-file)
- [10. Differences from Mackie Control](#10-differences-from-mackie-control)
- [11. Open items](#11-open-items)

## 1. Terms

| Term | Address or shape | Definition |
|---|---|---|
| ARC USB | USB device | RME's wired remote for TotalMix FX. |
| key | Note On/Off, channel 1 | One of the 15 front-panel keys. |
| footswitch | Note On/Off, channel 1 | The external switch input on the 1/4" jack. |
| encoder | Control Change 16, channel 1 | The single rotary control. |
| LED | Note On, channel 1 | Host feedback for a key. |
| note mode | firmware default | Keys and LEDs use notes; the encoder uses CC. |
| SysEx mode | firmware 7 option | The device communicates only through SysEx. |
| snapshot | mixer slot | A stored mixer state of the TotalControl workspace. |
| encoder target | model selection | The output whose volume the encoder changes. |

Direction in the tables is relative to the host: `→` is device to host, `←` is
host to device.

## 2. Transport

| Property | Value |
|---|---|
| USB class | UAC 1 |
| ALSA card id | `ARC` |
| Rawmidi device | index 0, node `/dev/snd/midiC<card>D0` |
| Streams | input and output on the same device |
| Kernel driver | `snd-usb-audio`, class driver |
| Handshake | none in note mode |
| Idle traffic | none |

The unit is a USB 1.1 device operating as a UAC 1 class device. Linux's class
driver creates the rawmidi device on the unit's own card. No hwdep node and no
udev rule are involved; the desktop user needs the ordinary rawmidi
permission.

Port discovery:

1. read `/proc/asound/cards`;
2. select the card whose id is `ARC`, or whose description contains `ARC` as
   a whole word;
3. open device 0 in both directions.

The card number is resolved at open time and is never configured by hand.

## 3. Keys

| Key | Note | Press | Release |
|---|---|---|---|
| 1–15 | `0x36`–`0x44` | `90 <note> 7F` | `90 <note> 00` |
| Footswitch | `0x45` | `90 45 7F` | `90 45 00` |

Release is accepted as either `90 <note> 00` or `80 <note> 00`. The parser
matches the status high nibble and ignores the channel nibble; the unit sends
channel 1.

A key reports a press and a release. The velocity carries no value beyond on
and off.

## 4. Encoder

| Value | Direction | Magnitude |
|---|---|---|
| `0x01`–`0x0F` | clockwise | 1–15 |
| `0x41`–`0x4F` | counter-clockwise | 1–15 |
| `0x00` | no movement | 0 |

| Property | Value |
|---|---|
| Message | `B0 10 <value>` |
| Encoding | relative signed-bit |
| Direction bit | bit 6 |
| Magnitude | low bits, 1–15 |
| Acceleration | performed by the unit |
| Push | open (§11) |

The unit performs the acceleration and reports the resulting step count. The
host applies the value as reported.

The encoder has no target of its own. The target is the `encoder_target`
setting of §8.

## 5. Feedback

| Bytes | LED |
|---|---|
| `90 <note> 7F` | on |
| `90 <note> 01` | blink |
| `90 <note> 00` | off |
| `90 <note> 02`–`7E` | off |

| Rule | Behaviour |
|---|---|
| LED state owner | host |
| LED memory | the unit keeps the state until the host writes a new one |
| Initial sync | the host writes all defined LEDs after connect |
| Updates | the host writes only LEDs whose state changed |
| Disconnect | the host clears its cached state |

The three lower keys light dim before the first host LED write: Talkback on bus
power, Speaker B when USB communication works, Dim when a host application has
connected. The first full sync replaces that state.

The encoder ring has no documented message and is not driven.

## 6. Default key assignment

The unit ships with these functions printed on the keys. TotalMix FX offers
more than 40 commands; the table lists the functions printed on a new unit.

| Key | Default function | TotalControl action |
|---|---|---|
| 1–8 | Snapshot 1–8 | `snapshot` 1–8 |
| 9 | Mono (Main Out) | `mono` |
| 10 | Volume Phones 1 | `encoder_target` `phones1` |
| 11 | Volume Phones 2 | `encoder_target` `phones2` |
| 12 | External Input | `external_input` |
| 13 | Talkback | `talkback` |
| 14 | Speaker B | `speaker_b` |
| 15 | Dim | `dim` |
| Footswitch | user-assigned | `toggle_window` |

Every key and the footswitch carry any action of §7.3. The map file holds the
printed default assignment. A user assignment is stored in the host settings
under the protocol id and the control id
([`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md) §10).

## 7. Host model

### 7.1 Connection

| Event | Host action |
|---|---|
| Controller selected and enabled | open the port, send the full LED sync |
| Port missing | mark the controller unavailable; keep the selection |
| Port busy | mark the port "in use by another program" |
| Input ends | close the controller, clear pressed keys, clear the LED cache |
| Reopen | full LED sync |

The controller runs while the audio device runs and survives a device switch.

### 7.2 Press resolution

The ARC reports presses. The host resolves the target state from the confirmed
model, the same way the GUI and OSC do.

| Action kind | Resolution |
|---|---|
| `toggle` | read the model value, write the opposite |
| `momentary` | write on while held, off on release |
| `snapshot` | recall that snapshot slot |
| `encoder_target` | set the encoder target |

The talkback key is a `toggle` unless the `talkback_mode` option of §8 selects
`momentary`.

A held key produces one state change. It does not repeat.

### 7.3 Actions

| Action | Effect |
|---|---|
| `none` | nothing |
| `snapshot` | recall snapshot 1–8 |
| `dim` | Main Out dim |
| `mono` | Main Out mono |
| `talkback` | Talkback |
| `speaker_b` | Speaker B |
| `external_input` | Control Room External Input |
| `recall_volume` | restore the pre-Dim monitoring volume |
| `mute_main` | mute the Main Out pair |
| `global_mute` | mute every channel |
| `global_solo` | solo every channel |
| `mute_group` | mute group 1–4 |
| `solo_group` | solo group 1–4 |
| `fader_group` | fader group 1–4 |
| `cue_main` | cue the Main Out |
| `cue_phones` | cue Phones 1–4 |
| `mute_phones` | mute Phones 1–4 |
| `encoder_target` | set the encoder target |
| `layout_preset` | layout preset 1–6 |
| `toggle_window` | show or hide the TotalControl window |

An action whose target is absent from the current model is unavailable: the
settings table marks it, and the host ignores it when it arrives.

### 7.4 Encoder target and step

| Setting | Value |
|---|---|
| Target | `main`, `phones1`–`phones4`, or `none` |
| Step | `standard` or `fine` |
| Standard step | the fader curve's 0.5 dB region above the coarse knee |
| Fine step | 0.1 dB |
| Option `encoder_follows_selection` | the encoder changes the output selected in the mixer instead of the fixed target |

The target is settable from a key bound to `encoder_target`. The encoder
writes the target's volume through the ordinary volume message, so the GUI
fader, the OSC path and the ARC stay in sync.

Fractional steps accumulate, so a slow turn keeps its sub-step remainder.

### 7.5 LED derivation

| LED | Lit when |
|---|---|
| A toggle action | the model value is on |
| A snapshot | that snapshot slot is current |
| `encoder_target` | the target matches the key's argument |
| `none` | never |
| An unavailable action | never |

Blink is reserved for a pending state. The `snapshot_blink_store` option of §8
uses it while a snapshot store is pending.

### 7.6 Multiple units

The wire format carries no unit identifier, so two units produce
indistinguishable messages. The host drives one ARC and logs a second unit.

## 8. Options list

The options are part of the controller definition,
[`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
§4.6. The settings dialog shows them in file order inside their groups. The
map file carries their defaults; the host settings carry the user's values.

### 8.1 Keys

| Key | Label | Form | Default | Behaviour |
|---|---|---|---|---|
| `led_feedback` | Light key LEDs | checkbox | on | The host drives the 15 key LEDs from the model. When off, the unit keeps its start-up dim state. |
| `enable_diag_leds` | Startup indicator | checkbox | off | The Dim key lights as a host-connected indicator during the first LED write. |
| `snapshot_blink_store` | Blink the snapshot key being stored | checkbox | off | The snapshot key blinks while a store is pending. |
| `default_keys` | Restore the printed key layout | button | — | Reset the key map to §6. |

`enable_diag_leds` and `snapshot_blink_store` require `led_feedback`.

### 8.2 Encoder

| Key | Label | Form | Choices | Default |
|---|---|---|---|---|
| `encoder_target` | Encoder target | choice | `main`, `phones1`–`phones4`, `none` | `main` |
| `encoder_step` | Encoder step | choice | `standard`, `fine` | `standard` |
| `encoder_follows_selection` | Encoder follows the selected output | checkbox | — | off |

### 8.3 Action behaviour

| Key | Label | Form | Choices | Default | Behaviour |
|---|---|---|---|---|---|
| `talkback_mode` | Talkback key | choice | `toggle`, `momentary` | `toggle` | Latching key, or on only while held. |
| `recall_double_press` | Recall on a double press | checkbox | — | off | A second press within 400 ms of the recall key restores the pre-Dim volume. |

### 8.4 Host behaviour

| Key | Label | Form | Default | Behaviour |
|---|---|---|---|---|
| `background_disable` | Disable the ARC in background | checkbox | off | Suspend input and LED updates while the window is unfocused or hidden. |

### 8.5 Stored values

Each row writes its value to the host settings under the definition id
`rme_arc_usb` and the option key, as
[`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
§10 defines. A missing value reads as the entry's `default`.

### 8.6 Defaults

| Option | Default |
|---|---|
| `talkback_mode` | `toggle` |
| `recall_double_press` | off |
| `encoder_target` | `main` |
| `encoder_step` | `standard` |
| `encoder_follows_selection` | off |
| `led_feedback` | on |
| `enable_diag_leds` | off |
| `snapshot_blink_store` | off |
| `default_keys` | button, no stored value |
| `background_disable` | off |

The defaults reproduce the unit's printed key layout, the main-volume encoder
and lit LEDs. The map file carries this state in the `default` field of each
option entry.

## 9. Map file

The ARC is described by `RME_ARC_USB_MIDI.json`. Its anatomy, the loader and
the search order are in
[`../TOTALCONTROL-CONTROLLER-SPEC.md`](../TOTALCONTROL-CONTROLLER-SPEC.md)
§4–6.

| Field | Value |
|---|---|
| `id` | `rme_arc_usb` |
| `shape` | `remote` |
| `port.source` | `card` |
| `port.match.card_id` | `ARC` |
| `port.devices` | 1 |
| `feedback` | `key_led`, value encoding `arc_led` |
| `numeric` | `encoder`, `relative_signed_bit`, `max_step` 15 |
| `options` | the options of §8 |
| `extra.tables.actions` | the action names of §7.3 |

## 10. Differences from Mackie Control

| Aspect | Mackie Control | ARC USB |
|---|---|---|
| Role | mixer surface | remote for single actions |
| Channels | 8 strips with a fader, a VPOT and buttons | none |
| Value input | faders and VPOTs | one encoder |
| Feedback | faders, rings, LCD, meters, LEDs | LEDs |
| Handshake | SysEx device query and challenge | none |
| Loopback guard | required | none |
| Port | the device MIDI ports | the unit's own class card |
| Options | protocol, LCD, names, levels, mapping | LEDs, encoder, talkback, key reset |

## 11. Open items

| Status | Item |
|---|---|
| needs-capture | Encoder push message, if the knob is pushed |
| needs-capture | SysEx mode message format |
| open | Second unit behaviour on one host |
| open | Standard-step curve, calibrated against a unit |
| open | Momentary talkback behaviour on a unit |
| open | Blink output confirmed on a unit |
