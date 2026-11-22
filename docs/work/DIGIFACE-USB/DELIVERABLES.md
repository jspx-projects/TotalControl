# Digiface USB — documents the device owner provides

Working note. What a contributor with the hardware supplies so that the
Digiface USB is specified positively and verified. The general contributor
rules below apply to every device; the Digiface capture list is specific.

## Captures

| Document | Content | Where it lands |
|---|---|---|
| Descriptor capture | Interface 1 class, alternate settings, endpoints, sizes, interval | `DIGIFACE_USB_PROTOCOL_SPEC.md` §4 and §5, cross-checked against the mainline quirk table |
| Normal-mode TotalMix capture | One write per control, with a known before and after state, per `CAPTURE-REQUEST.md` §4 | Each prior-art row becomes a verified row and names its capture |
| Meter capture | Five seconds with a known signal, all three frame types | `DIGIFACE_USB_PROTOCOL_SPEC.md` §7 |
| Loopback capture | On and off for one input | `DIGIFACE_USB_PROTOCOL_SPEC.md` §5.5 |
| Clock and rate capture | One change of each while streaming, with the status reply | `DIGIFACE_USB_PROTOCOL_SPEC.md` §5.3 and §9 |
| Snapshot capture | Store and recall one snapshot | `DIGIFACE_USB_PROTOCOL_SPEC.md` §10 |
| Firmware and mode note | Version, normal or CC, speed mode | Header of the spec and the pull request |
| Hardware test log | The steps of `IMPLEMENTATION.md` §7 with their results | The pull request description |
| Their existing application | Source code or protocol notes, when shareable | Cross-check only; no code is lifted |

The contributor runs the capture script and reports what differs from the
specification.

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