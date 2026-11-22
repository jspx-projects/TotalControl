# ARC USB — documents the device owner provides

Working note. What a contributor with the hardware supplies so that the
ARC USB remote is specified positively and verified. The general contributor
rules below apply to every device; the ARC capture list is specific.

## Captures

| Document | Content | Where it lands |
|---|---|---|
| Encoder push capture | The byte stream of one knob push, if the knob moves | `RME_ARC_USB_MIDI.md` §4 and §11 |
| SysEx mode capture | A firmware 7 unit in SysEx mode, pressing keys and writing LEDs | `RME_ARC_USB_MIDI.md` §2 and §11, only if note mode is insufficient |
| Blink check | One LED set to velocity `01` on a unit | `RME_ARC_USB_MIDI.md` §5 |
| Live test log | The steps of `TESTING.md` §4 with their results | The pull request description |

## What the contributor supplies

| Item | Reason |
|---|---|
| The specification and the map files | They are the deliverable of the drafted documents |
| The capture files | Kept outside the repository, by policy |
| The hardware test results | The pull request description |

## What no commit carries

| Item | Reason |
|---|---|
| Serial numbers, USB IDs, host names, home paths | Personal identifying information |
| Captured device state | Outside the repository, by policy |

## Sequence

1. The contributor reads the drafted specification in their own tree.
2. They run the captures and report the differences.
3. The maintainer folds the differences into the specification and the plan.
4. They implement, test on the unit, and open the pull request against our repository.
5. The maintainer reviews the specification diff and the test log.