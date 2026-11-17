# ARC USB sources and licence

Working note. The MIT position is the same for both features; the Digiface
directory holds the longer version.

## 1. Vendor documentation

| Source | Use |
|---|---|
| RME, *Advanced Remote Control USB User's Guide*, version 1.6, 03/2025, <https://rme-audio.de/downloads/arcusb_e.pdf> | Class compliance, default keys, encoder modes, SysEx option, stand-alone mode |
| RME, *ARC USB MIDI command list* (image) | Key notes, LED velocities, encoder CC values |
| RME, ARC USB product page, <https://rme-audio.de/arc-usb.html> | Firmware 7 SysEx note |

Relevant manual sections: 3 (supported interfaces), 6 (overview, default keys,
diagnostic LEDs), 7 (configuration, available functions, encoder modes, SysEx
only), 8 (stand-alone mode).

The command list is the only public table of the exact note and CC values.
It is reproduced in the article at
<https://ameblo.jp/holycater/entry-12467355916.html>. The facts are restated as
our own tables; no text or image from the article is copied.

## 2. Public implementations

| Source | Licence | Use |
|---|---|---|
| Openface-Mixer, <https://github.com/thomas-nelson23/Openface-Mixer> | MIT | Port discovery by card id, parser behaviour, encoder decode, LED write, default key map |
| Mackie Control protocol reference: N. Jarnoux, *Mackie Control Protocol* (2016); TouchMCU `doc/mackie_control_protocol.md` | factual reference | Note and CC map, SysEx handshake, meter nibbles |

The Openface-Mixer ARC module states that its key and encoder values were
captured from a real unit in note mode. That is the strongest available
evidence for the ARC wire format besides the vendor command list.

## 3. Repository licences

| Part | Licence |
|---|---|
| `kernel/` | GPL-2.0-only |
| `ui/totalcontrol/totalcontrol-core` | MIT |
| `ui/totalcontrol/totalcontrol-gui` | GPL-3.0-only |
| Tools, scripts and documentation | GPL-3.0-or-later |
| Map files under `data/` | GPL-3.0-or-later, as data of the application |

## 4. MIT position

Openface-Mixer is MIT. MIT permits use, modification, sublicensing and
distribution, provided the copyright notice and permission notice are
retained. MIT is compatible with GPL-2.0 and GPL-3.0.

For this work:

| Target | Allowed | Obligation |
|---|---|---|
| `totalcontrol-gui` code, GPL-3.0-only | yes | keep the MIT notice on adapted files and in `THIRD-PARTY.md` |
| Documentation, GPL-3.0-or-later | yes | keep the MIT notice for transcribed tables or text |
| Map files | yes | a JSON table of protocol facts is not a copy of the licence text; attribute in `THIRD-PARTY.md` anyway |

No licence change is needed.

### Entry to add to `THIRD-PARTY.md`

```markdown
## Openface-Mixer — RME ARC USB protocol reference

<https://github.com/thomas-nelson23/Openface-Mixer>

MIT, Copyright (c) 2026 The Openface Mixer contributors.  The ARC USB map file
and the ARC protocol section follow this project's notes.  The upstream
commit is <hash>.
```

Append the full MIT text below it, as the file already does for its other MIT
sources.

### Per-file header for adapted code

```rust
// SPDX-License-Identifier: GPL-3.0-only
//
// Portions derived from Openface-Mixer, MIT,
// Copyright (c) 2026 The Openface Mixer contributors.
```

## 5. What is not copied

| Item | Handling |
|---|---|
| Vendor PDFs, screenshots | local only, never committed |
| Captured device state | not used; the map files are protocol definitions |
| The command-list image | its facts are restated as tables |

## 6. Open items

| Status | Item |
|---|---|
| open | Pin the Openface-Mixer commit hash before any code is adapted |
| open | Decide whether the parser is adapted or reimplemented |
