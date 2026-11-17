# ARC USB support

Working note. This directory holds the draft specification and the
implementation plan for the RME ARC USB and for the generic MIDI controller
model it belongs to. The generic engine is implemented and parity-tested
against the Mackie codec; the ARC definition awaits its live-unit test.

## Documents

| File | Purpose | Destination once reviewed |
|---|---|---|
| [`RME_ARC_USB_MIDI.md`](RME_ARC_USB_MIDI.md) | Wire protocol and host model of the ARC | `docs/spec/midi-controllers/RME_ARC_USB_MIDI.md` |
| [`MACKIE_MIDI.md`](MACKIE_MIDI.md) | Wire protocol and host model of Mackie Control, split out of the MIDI spec | `docs/spec/midi-controllers/MACKIE_MIDI.md` |
| [`CONTROLLER_MODEL.md`](CONTROLLER_MODEL.md) | Anatomy of a controller definition file, the generalised host model, and the settings UI | New `docs/spec/TOTALCONTROL-CONTROLLER-SPEC.md` |
| [`MIDI_TRANSPORT.md`](MIDI_TRANSPORT.md) | Trimmed `TOTALCONTROL-MIDI-SPEC.md`: the transport only | `docs/spec/TOTALCONTROL-MIDI-SPEC.md` |
| [`MAP-FILES.md`](MAP-FILES.md) | The two map files, where they live, how they are found and loaded | Part of `TOTALCONTROL-CONTROLLER-SPEC.md` §5–6 |
| [`MAPS/Mackie_MIDI.json`](MAPS/Mackie_MIDI.json) | Mackie Control map, extracted from `totalcontrol-gui/src/mackie.rs` | `data/midi-controllers/Mackie_MIDI.json` |
| [`MAPS/RME_ARC_USB_MIDI.json`](MAPS/RME_ARC_USB_MIDI.json) | ARC USB map, from the vendor command list | `data/midi-controllers/RME_ARC_USB_MIDI.json` |
| [`SOURCES.md`](SOURCES.md) | Sources and the MIT licence position | `THIRD-PARTY.md` entries and code comments |
| [`TESTING.md`](TESTING.md) | What the reviewer must run and verify | PR checklist |

## The idea

Two things change together.

1. The MIDI controller support becomes generic. A controller is described by a
   definition file; the host runs the same engine for every file.
2. The Mackie Control part moves out of `TOTALCONTROL-MIDI-SPEC.md` into its
   own document beside the ARC, and the MIDI spec keeps only the transport.

The ARC then needs no special code path and no dedicated settings menu. It is
one entry in the controller list, and its map file is the same kind of file as
the Mackie map.

## Is the listbox a good idea

Yes, with three constraints.

- Each controller type carries its own options list in its map file. Mackie
  options cover the LCD, meters and the mixer surface; ARC options cover the
  key map and the encoder. The listbox selects a controller, and the dialog
  shows that controller's option groups.
- The ENGINE is shared. Parsing, ports, worker, LED diff, reconnect and the
  action dispatch run the same code for every controller. A map file carries
  data only.
- The MAP FILES are data. The shipped files are the defaults; a user copy in
  the user data directory overrides them.

## Status

| Item | State |
|---|---|
| ARC wire protocol | Verified against the vendor command list and a public implementation |
| ARC host model | Designed in `RME_ARC_USB_MIDI.md` |
| Mackie split | Landed: transport, Mackie and controller documents in `docs/spec/` |
| Map file format | Landed as `docs/spec/TOTALCONTROL-CONTROLLER-SPEC.md`, schema 1 |
| Map files | Written; Mackie shipped in `data/midi-controllers/`, ARC here |
| Engine implementation | Implemented, parity-tested against the Mackie codec |
| GUI listbox and options | Implemented |
| Live unit test | Not done |

## Decided design points

| Decision | Choice |
|---|---|
| Instances | One per enabled input port, each with its own protocol; several run at once |
| Assignments | Host settings, keyed by protocol id and control id; map files stay pristine |
| Schema 1 scope | note, cc, cc_pair, pitch_bend, channel_pressure, program_change, sysex patterns |
| Port selection | Engine level, per instance; the map's `port` block declares source and defaults |
| Embedded fallback | Yes, the shipped definitions are embedded in the binary |
| Layout text syntax | Deferred; schema 1 is JSON only |
| TUI | Deferred; GUI first |

## Open questions

| Status | Question |
|---|---|
| needs-capture | Encoder push message, if the knob is pushed |
| needs-capture | SysEx mode format, only if note mode is insufficient |
