# What the reviewer verifies

Working note. This is the checklist for the person with the hardware and a
source checkout. Nothing here has been implemented yet.

## 1. Put the documents in place

| From | To |
|---|---|
| `RME_ARC_USB_MIDI.md` | `docs/spec/midi-controllers/RME_ARC_USB_MIDI.md` |
| `MACKIE_MIDI.md` | `docs/spec/midi-controllers/MACKIE_MIDI.md` |
| `CONTROLLER_MODEL.md` | `docs/spec/TOTALCONTROL-CONTROLLER-SPEC.md` |
| `MIDI_TRANSPORT.md` | `docs/spec/TOTALCONTROL-MIDI-SPEC.md` (replaces the current file) |

Then update every reference to the MIDI specification:

```sh
grep -rn "TOTALCONTROL-MIDI-SPEC" docs AGENTS.md README.md
```

Each hit must point to the transport spec, the Mackie document or the
controller spec, whichever owns the paragraph.

Update `docs/spec/README.md` with the new files.

## 2. Add the map files

| From | To |
|---|---|
| `MAPS/Mackie_MIDI.json` | `data/midi-controllers/Mackie_MIDI.json` |
| `MAPS/RME_ARC_USB_MIDI.json` | `data/midi-controllers/RME_ARC_USB_MIDI.json` |

Add `$(datadir)/totalcontrol/midi-controllers/` to the install target. Add the
same path to the RPM and Debian packaging.

## 3. Review the map files

The Mackie map was extracted from `totalcontrol-gui/src/mackie.rs`. Check it
against the code, not against the manual:

| Check | Expected |
|---|---|
| Strip fader pitch bend channels | 1–8 |
| Master fader | pitch bend channel 9 |
| VPOT turn CC | `0x10`–`0x17` relative |
| Ring CC | `0x30`–`0x37` |
| F1–F8 notes | `0x36`–`0x3D` |
| Master mute, solo notes | `0x2C`, `0x2B` |
| Snapshot notes | `0x36`–`0x3D` |
| Loopback note | channel 16, note 127, 500 ms |
| Handshake header | `f0 00 00 66 14` |

The ARC map was written from the vendor command list and a public
capture-based implementation. Check it against a unit.

## 4. Verify the ARC on the unit

1. Connect the ARC and list the card:

   ```sh
   cat /proc/asound/cards
   amidi -l
   ```

   Expect a card with the id `ARC` and one rawmidi device.

2. Record the firmware version. Note whether SysEx mode is set.

3. Watch the input while pressing every key in order:

   ```sh
   amidi -p hw:<card>,0 -d
   ```

   | Key | Expected |
   |---|---|
   | 1–15 | `90 36 7F` … `90 44 7F` on press, velocity `00` on release |
   | Footswitch | `90 45 7F` and `90 45 00` |

4. Turn the encoder one click in each direction. Expect `B0 10 01` and
   `B0 10 41`. Turn it fast and record the largest step seen.

5. Write an LED and look at the unit:

   ```sh
   amidi -p hw:<card>,0 -S "90 36 7F"
   amidi -p hw:<card>,0 -S "90 36 01"
   amidi -p hw:<card>,0 -S "90 36 00"
   ```

   Expect steady on, blink, off.

6. Push the encoder knob, if it moves, and record what arrives.

7. Nothing arrives while idle. If nothing arrives at all, SysEx mode is
   probably active; say so rather than treating it as a fault.

8. Unplug and replug while the port is open. Record whether the card index
   changes. Do not store the values in a committed file.

## 5. Verify the Mackie split

The text was reordered from the current specification. Check that no claim
changed:

```sh
git show main:docs/spec/TOTALCONTROL-MIDI-SPEC.md > /tmp/old-midi.md
```

Compare the note table, the CC table, the handshake formula, the LCD layout,
the meter nibble scale and the options list. Each value must appear in either
`MACKIE_MIDI.md` or `MIDI_TRANSPORT.md` with the same bytes.

## 6. Check the transport

Build and test:

```sh
cargo test -p totalcontrol-core --lib --features alsa
cargo test -p totalcontrol-gui
```

## 7. Everything that may not appear in the commit

| Item | Reason |
|---|---|
| Vendor PDFs and screenshots | copyright, and they are not needed |
| Serial numbers, USB IDs, host names, home paths | personal identifying information |
| Captured device state | not a protocol definition |
| The map files as the reviewer's own edited copies | the shipped files are defaults |

## 8. Pull request

Open the pull request against the project repository, not a fork of the
vendor. Use the pull request template. In the description state:

- the device, transport and firmware used;
- the steps of §4 that were run, and which passed;
- any value that differed from this document;
- confirmation that the user scene and the GUI settings were preserved.

## 9. Anything that must be captured before implementation

| Item | Blocks |
|---|---|
| Encoder push message | the encoder push action |
| SysEx mode format | SysEx-only units |
| Button LED blink on a unit | the blink feedback |
| Second ARC on one host | multiple-unit support |
| Digiface mixer captures | the Digiface backend, a separate note |
