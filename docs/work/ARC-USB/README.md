# ARC USB — remaining work for the device owner

Working note. The generic controller engine, the ARC definition file and
the ARC protocol specification are in the repository. This note lists only
what the person with the ARC hardware still has to do.

## Already in place

| Item | Location |
|---|---|
| Controller engine and definition format | `ui/totalcontrol/totalcontrol-gui/src/controllers/`, `docs/spec/TOTALCONTROL-CONTROLLER-SPEC.md` |
| ARC definition file, shipped and embedded | `data/midi-controllers/RME_ARC_USB_MIDI.json` |
| ARC protocol specification | `docs/spec/midi-controllers/RME_ARC_USB_MIDI.md` |
| Engine tests incl. the ARC remote shape | `ui/totalcontrol/totalcontrol-gui/src/controllers/parity.rs` |
| Licence attribution | `THIRD-PARTY.md`, Openface-Mixer entry |

## Your steps

1. Rebase onto `main` once the generic-controller pull request is merged,
   then build: `cargo build -p totalcontrol-gui`.
2. Run the live-unit checklist in [`TESTING.md`](TESTING.md). Record the
   firmware version and whether the unit is in note mode or SysEx mode.
3. Where the unit differs from the specification, correct
   `data/midi-controllers/RME_ARC_USB_MIDI.json` and
   `docs/spec/midi-controllers/RME_ARC_USB_MIDI.md` in the same change,
   and adjust the tests.
4. Provide the captures of the open items below, or state that the unit
   cannot produce them.
5. Update the status table of the ARC protocol specification: live-unit
   verification verified, with the evidence. Update the manual only if
   the visible behaviour differs.
6. Open the pull request with the project template: device, transport and
   firmware; the checklist steps run and their results; every value that
   differed; confirmation that the user scene and
   `~/.config/totalcontrol/gui.json` were preserved.

## Open items to capture

| Status | Item |
|---|---|
| needs-capture | Encoder push message, if the knob is pushed |
| needs-capture | SysEx mode message format (firmware 7 option) |
| needs-capture | LED blink confirmed on a unit |
| open | Standard-step curve, calibrated against a unit |
| open | Momentary talkback behaviour on a unit |
| open | Second ARC on one host |

## Testing without a pull request

The shipped definition loads as-is. To try a corrected map before
committing it, copy it to
`~/.local/share/totalcontrol/midi-controllers/RME_ARC_USB_MIDI.json`; the
user file overrides the shipped one. `TOTALCONTROL_CONTROLLERS` points the
loader at any directory for development.