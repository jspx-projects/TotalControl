# ARC USB live-unit checklist

Working note. The checklist for the person with the hardware. The engine,
the definition file and the specification are already in the repository;
this verifies them against a unit.

## 1. Build and enumerate

```sh
cargo build -p totalcontrol-gui
cat /proc/asound/cards
amidi -l
```

Expect a card with the id `ARC` and one rawmidi device. Record the firmware
version. Note whether SysEx mode is set.

## 2. Wire values

Watch the input while pressing every key in order:

```sh
amidi -p hw:<card>,0 -d
```

| Key | Expected |
|---|---|
| 1–15 | `90 36 7F` … `90 44 7F` on press, velocity `00` on release |
| Footswitch | `90 45 7F` and `90 45 00` |

Turn the encoder one click in each direction. Expect `B0 10 01` and
`B0 10 41`. Turn it fast and record the largest step seen.

Write an LED and look at the unit:

```sh
amidi -p hw:<card>,0 -S "90 36 7F"
amidi -p hw:<card>,0 -S "90 36 01"
amidi -p hw:<card>,0 -S "90 36 00"
```

Expect steady on, blink, off.

Push the encoder knob, if it moves, and record what arrives. Nothing
arrives while idle. If nothing arrives at all, SysEx mode is probably
active; say so rather than treating it as a fault.

Unplug and replug while the port is open. Record whether the card index
changes. Do not store the values in a committed file.

## 3. In TotalControl

1. Open `Options > MIDI and OSC`, enable the MIDI part, select the ARC in
   the Controller list, check MIDI In and MIDI Out. The protocol
   preselects as RME ARC USB.
2. Press each key: the assigned action fires, and the LED follows the
   mixer state.
3. Turn the encoder: the target volume moves in the selected step; the
   GUI fader follows.
4. Reassign a key in the Key assignment table; the new action fires.
5. Set the talkback key to Momentary; talkback is on only while held.
6. Switch the encoder target with a key bound to Volume Phones; the
   encoder then moves that phones volume.

## 4. Tests

```sh
cargo test -p totalcontrol-core --lib --features alsa
cargo test -p totalcontrol-gui
```

## 5. Everything that may not appear in the commit

| Item | Reason |
|---|---|
| Vendor PDFs and screenshots | copyright, and they are not needed |
| Serial numbers, USB IDs, host names, home paths | personal identifying information |
| Captured device state | not a protocol definition |

## 6. Pull request

Open the pull request against the project repository. Use the pull request
template. In the description state:

- the device, transport and firmware used;
- the steps above that were run, and which passed;
- any value that differed from the specification;
- confirmation that the user scene and the GUI settings were preserved.